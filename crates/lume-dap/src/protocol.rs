//! Messaggi del Debug Adapter Protocol.
//!
//! Solo ciò che la v1 usa ha un tipo; il resto resta `serde_json::Value` e non
//! si interpreta. A differenza di JSON-RPC, DAP numera ogni messaggio con
//! `seq` e lega la risposta alla richiesta con `request_seq`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Un messaggio letto dall'adattatore.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response(Response),
    Event(Event),
    /// Richiesta inversa (`runInTerminal`, `startDebugging`, ...).
    Request {
        seq: i64,
        command: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub request_seq: i64,
    pub success: bool,
    pub command: String,
    pub message: Option<String>,
    pub body: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub event: String,
    pub body: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("messaggio DAP non JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("messaggio DAP senza campo `{0}`")]
    MissingField(&'static str),
    #[error("tipo di messaggio DAP sconosciuto: {0}")]
    UnknownType(String),
}

fn text_field(value: &Value, name: &'static str) -> Result<String, DecodeError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(DecodeError::MissingField(name))
}

fn int_field(value: &Value, name: &'static str) -> Result<i64, DecodeError> {
    value
        .get(name)
        .and_then(Value::as_i64)
        .ok_or(DecodeError::MissingField(name))
}

/// Decodifica il corpo di un frame.
pub fn decode(frame: &[u8]) -> Result<Incoming, DecodeError> {
    let value: Value = serde_json::from_slice(frame)?;
    let kind = text_field(&value, "type")?;
    match kind.as_str() {
        "response" => Ok(Incoming::Response(Response {
            request_seq: int_field(&value, "request_seq")?,
            success: value
                .get("success")
                .and_then(Value::as_bool)
                .ok_or(DecodeError::MissingField("success"))?,
            command: text_field(&value, "command")?,
            message: value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned),
            body: value.get("body").cloned().unwrap_or(Value::Null),
        })),
        "event" => Ok(Incoming::Event(Event {
            event: text_field(&value, "event")?,
            body: value.get("body").cloned().unwrap_or(Value::Null),
        })),
        "request" => Ok(Incoming::Request {
            seq: int_field(&value, "seq")?,
            command: text_field(&value, "command")?,
        }),
        other => Err(DecodeError::UnknownType(other.to_owned())),
    }
}

/// Le capability dell'adattatore che governano quali controlli compaiono.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Capabilities {
    pub supports_configuration_done_request: bool,
    pub supports_conditional_breakpoints: bool,
    pub supports_hit_conditional_breakpoints: bool,
    pub supports_log_points: bool,
    pub supports_terminate_request: bool,
    pub support_terminate_debuggee: bool,
    pub supports_evaluate_for_hovers: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_reference: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub line: i64,
    #[serde(default)]
    pub column: i64,
    #[serde(default)]
    pub instruction_pointer_reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub name: String,
    pub variables_reference: i64,
    #[serde(default)]
    pub presentation_hint: Option<String>,
    #[serde(default)]
    pub expensive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(default, rename = "type")]
    pub type_name: Option<String>,
    #[serde(default)]
    pub variables_reference: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

/// Un punto di interruzione chiesto dal client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBreakpoint {
    pub line: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

/// Lo stato di un punto di interruzione secondo l'adattatore.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub source: Option<Source>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoppedEvent {
    pub reason: String,
    #[serde(default)]
    pub thread_id: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub all_threads_stopped: bool,
    #[serde(default)]
    pub hit_breakpoint_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEvent {
    #[serde(default)]
    pub category: Option<String>,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitedEvent {
    pub exit_code: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointEvent {
    pub reason: String,
    pub breakpoint: Breakpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateResult {
    pub result: String,
    #[serde(default, rename = "type")]
    pub type_name: Option<String>,
    #[serde(default)]
    pub variables_reference: i64,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn decodifica_risposta_evento_e_richiesta_inversa() {
        let risposta = decode(
            br#"{"seq":3,"type":"response","request_seq":1,"success":false,"command":"evaluate","message":"no"}"#,
        )
        .unwrap();
        assert_eq!(
            risposta,
            Incoming::Response(Response {
                request_seq: 1,
                success: false,
                command: "evaluate".into(),
                message: Some("no".into()),
                body: Value::Null,
            })
        );
        let evento = decode(
            br#"{"seq":4,"type":"event","event":"stopped","body":{"reason":"step","threadId":1}}"#,
        )
        .unwrap();
        let Incoming::Event(evento) = evento else {
            panic!("atteso un evento");
        };
        let stopped: StoppedEvent = serde_json::from_value(evento.body).unwrap();
        assert_eq!(stopped.reason, "step");
        assert_eq!(stopped.thread_id, Some(1));
        let inversa =
            decode(br#"{"seq":5,"type":"request","command":"runInTerminal","arguments":{}}"#)
                .unwrap();
        assert_eq!(
            inversa,
            Incoming::Request {
                seq: 5,
                command: "runInTerminal".into()
            }
        );
    }

    #[test]
    fn messaggi_malformati_sono_errori_espliciti() {
        assert!(matches!(
            decode(br#"{"type":"response","command":"x"}"#),
            Err(DecodeError::MissingField("request_seq"))
        ));
        assert!(matches!(
            decode(br#"{"type":"altro"}"#),
            Err(DecodeError::UnknownType(_))
        ));
        assert!(matches!(decode(b"non json"), Err(DecodeError::Json(_))));
    }
}
