#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::{Value, json};

    fn info() -> ServerInfo {
        ServerInfo {
            version: "0.2.1".to_owned(),
            project_root: r"C:\progetto".to_owned(),
        }
    }

    fn risposta(outcome: Outcome) -> Value {
        match outcome {
            Outcome::Reply(testo) | Outcome::Initialized { reply: testo, .. } => {
                serde_json::from_str(&testo).unwrap()
            }
            altro => panic!("attesa una risposta, non {altro:?}"),
        }
    }

    fn pronta() -> Session {
        let mut sessione = Session::new();
        let _ = sessione.handle(
            br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#,
            &info(),
            Availability::default(),
        );
        let _ = sessione.handle(
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &info(),
            Availability::default(),
        );
        sessione
    }

    #[test]
    fn initialize_dichiara_list_changed_e_la_radice() {
        let mut sessione = Session::new();
        let esito = sessione.handle(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"claude-code","version":"2"}}}"#,
            &info(),
            Availability::default(),
        );
        let Outcome::Initialized { client, .. } = &esito else {
            panic!("atteso Initialized, non {esito:?}")
        };
        assert_eq!(client, "claude-code");
        let valore = risposta(esito);
        assert_eq!(valore["jsonrpc"], "2.0");
        assert_eq!(valore["id"], 1);
        assert_eq!(valore["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            valore["result"]["capabilities"]["tools"]["listChanged"],
            true
        );
        assert_eq!(valore["result"]["serverInfo"]["name"], "Lume");
        assert_eq!(
            valore["result"]["serverInfo"]["project_root"],
            r"C:\progetto"
        );
        assert!(!sessione.is_initialized());
        assert_eq!(
            sessione.handle(
                br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &info(),
                Availability::default(),
            ),
            Outcome::Nothing
        );
        assert!(sessione.is_initialized());
    }

    #[test]
    fn la_versione_si_negozia_o_si_propone_la_nostra() {
        let versione = |richiesta: &str| {
            let riga = format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{richiesta}"}}}}"#
            );
            risposta(Session::new().handle(riga.as_bytes(), &info(), Availability::default()))
                ["result"]["protocolVersion"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(versione("2024-11-05"), "2024-11-05");
        assert_eq!(versione("2025-03-26"), "2025-03-26");
        assert_eq!(versione("2099-01-01"), PROTOCOL_VERSION);
    }

    #[test]
    fn un_secondo_initialize_e_rifiutato() {
        let mut sessione = pronta();
        let valore = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{}}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(valore["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn le_operazioni_normali_attendono_la_notifica_initialized() {
        let mut sessione = Session::new();
        let _ = sessione.handle(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            &info(),
            Availability::default(),
        );
        let prima = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(prima["error"]["code"], INVALID_REQUEST);

        assert_eq!(
            sessione.handle(
                br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &info(),
                Availability::default(),
            ),
            Outcome::Nothing
        );
        let dopo = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
            &info(),
            Availability::default(),
        ));
        assert!(dopo["result"]["tools"].is_array());
    }

    #[test]
    fn prima_di_initialize_passa_solo_ping() {
        let mut sessione = Session::new();
        let ping = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(ping["result"], json!({}));
        let lista = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(lista["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn un_metodo_sconosciuto_e_32601() {
        let valore = risposta(pronta().handle(
            br#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(valore["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(valore["id"], 3);
    }

    #[test]
    fn un_tool_sconosciuto_o_spento_e_32602() {
        let mut sessione = pronta();
        let sconosciuto = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"format_document","arguments":{}}}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(sconosciuto["error"]["code"], INVALID_PARAMS);
        let spento = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"git_status"}}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(
            spento["error"]["code"], INVALID_PARAMS,
            "Git spento: il tool non c'è"
        );
        let comando = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"run_command","arguments":{"program":"cmd.exe"}}}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(
            comando["error"]["code"], INVALID_PARAMS,
            "comandi arbitrari non confinati non sono invocabili"
        );
    }

    #[test]
    fn id_stringa_e_campi_extra_sono_accettati() {
        let valore = risposta(pronta().handle(
            br#"{"jsonrpc":"2.0","id":"abc","method":"tools/list","params":{"cursor":null},"_meta":{"x":1}}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(valore["id"], "abc");
        assert_eq!(
            valore["result"]["tools"].as_array().unwrap().len(),
            tools_list(Availability::default())["tools"]
                .as_array()
                .unwrap()
                .len()
        );
    }

    #[test]
    fn json_rotto_o_forma_sbagliata_danno_errori_con_id_nullo() {
        let mut sessione = pronta();
        let rotto = risposta(sessione.handle(b"{non json", &info(), Availability::default()));
        assert_eq!(rotto["error"]["code"], PARSE_ERROR);
        assert!(rotto["id"].is_null());
        let lotto =
            risposta(sessione.handle(br#"[{"jsonrpc":"2.0"}]"#, &info(), Availability::default()));
        assert_eq!(lotto["error"]["code"], INVALID_REQUEST);
        let senza_versione = risposta(sessione.handle(
            br#"{"id":1,"method":"ping"}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(senza_versione["error"]["code"], INVALID_REQUEST);
        let id_decimale = risposta(sessione.handle(
            br#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#,
            &info(),
            Availability::default(),
        ));
        assert_eq!(id_decimale["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn le_notifiche_non_hanno_risposta() {
        let mut sessione = pronta();
        for riga in [
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.as_slice(),
            br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#
                .as_slice(),
            br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":{"bad":true}}}"#.as_slice(),
            br#"{"jsonrpc":"2.0","method":"qualcosa/di/nuovo"}"#.as_slice(),
        ] {
            assert_eq!(
                sessione.handle(riga, &info(), Availability::default()),
                Outcome::Nothing
            );
        }
    }

    #[test]
    fn cancelled_richiama_l_id_della_chiamata_da_annullare() {
        let mut session = pronta();
        assert_eq!(
            session.handle(
                br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3,"reason":"stop"}}"#,
                &info(),
                Availability::default()
            ),
            Outcome::Cancelled(McpId::Num(3))
        );
        assert_eq!(
            session.handle(
                br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"abc"}}"#,
                &info(),
                Availability::default()
            ),
            Outcome::Cancelled(McpId::Str("abc".to_owned()))
        );
        let mut early = Session::new();
        assert_eq!(
            early.handle(
                br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}"#,
                &info(),
                Availability::default()
            ),
            Outcome::Nothing
        );
    }

    #[test]
    fn instructions_spiegano_modalita_conferme_e_confini() {
        let value = risposta(Session::new().handle(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            &info(),
            Availability::default(),
        ));
        let instructions = value["result"]["instructions"].as_str().unwrap();
        for term in ["modificare", "modalità", "conferma", ".git", r"C:\progetto"] {
            assert!(instructions.contains(term), "manca {term}: {instructions}");
        }
        assert_eq!(SERVER_BUSY, -32000);
    }

    #[test]
    fn la_riga_di_tetto_e_un_errore_jsonrpc_esplicito_senza_id() {
        let testo = busy_line();
        assert!(!testo.contains('\n'), "una riga sola: {testo}");
        let valore: Value = serde_json::from_str(&testo).unwrap();
        assert_eq!(valore["jsonrpc"], "2.0");
        assert!(valore["id"].is_null());
        assert_eq!(valore["error"]["code"], SERVER_BUSY);
        assert_eq!(valore["error"]["message"], BUSY_MESSAGE);
        assert!(
            BUSY_MESSAGE.contains("occupate"),
            "il messaggio dice perché: {BUSY_MESSAGE}"
        );
    }

    #[test]
    fn write_file_e_un_tool_attivo_e_produce_una_chiamata() {
        let result = pronta().handle(
            br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"write_file","arguments":{"path":"a.txt","content":"x"}}}"#,
            &info(),
            Availability::default(),
        );
        assert!(matches!(
            result,
            Outcome::Call {
                tool: ToolName::WriteFile,
                ..
            }
        ));
    }

    #[test]
    fn tools_call_produce_una_chiamata_con_gli_argomenti() {
        let esito = pronta().handle(
            br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"a.txt"}}}"#,
            &info(),
            Availability::default(),
        );
        let Outcome::Call {
            id,
            tool,
            arguments,
        } = esito
        else {
            panic!("attesa una chiamata, non {esito:?}")
        };
        assert_eq!(id, McpId::Num(9));
        assert_eq!(tool, ToolName::ReadFile);
        assert_eq!(arguments["path"], "a.txt");
        let senza_argomenti = pronta().handle(
            br#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"list_files"}}"#,
            &info(),
            Availability::default(),
        );
        assert!(matches!(senza_argomenti, Outcome::Call { arguments, .. } if arguments.is_empty()));
    }

    #[test]
    fn il_nome_del_client_e_ripulito_prima_di_arrivare_all_interfaccia() {
        assert_eq!(sanitize_client_name("claude-code"), "claude-code");
        assert_eq!(sanitize_client_name("a\u{1b}[31mb\ncd"), "a[31mbcd");
        assert_eq!(sanitize_client_name(&"x".repeat(100)).len(), 32);
        assert_eq!(sanitize_client_name("   "), "sconosciuto");
        assert_eq!(sanitize_client_name("città"), "citt");
    }

    #[test]
    fn tool_result_segnala_gli_errori_nel_risultato() {
        let buono = tool_result(Ok(json!({"a": 1})));
        assert_eq!(buono["isError"], false);
        assert_eq!(buono["content"][0]["type"], "text");
        let testo: Value =
            serde_json::from_str(buono["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(testo, json!({"a": 1}));
        let cattivo = tool_result(Err(ToolError::new("fuori dalla radice")));
        assert_eq!(cattivo["isError"], true);
        assert_eq!(cattivo["content"][0]["text"], "fuori dalla radice");
    }

    #[test]
    fn la_notifica_list_changed_non_ha_id() {
        let valore: Value = serde_json::from_str(&list_changed_notification()).unwrap();
        assert_eq!(valore["method"], "notifications/tools/list_changed");
        assert!(valore.get("id").is_none());
    }
}
use crate::catalog::{Args, Availability, ToolError, ToolName, tools_list};
use crate::id::McpId;
use lume_lsp::{JsonRpcVersion, RpcError};
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const SERVER_BUSY: i64 = -32000;

/// Messaggio unico del rifiuto per tetto di client: lo usano il server nella
/// riga di tetto e il ponte nel proprio errore d'uscita.
pub const BUSY_MESSAGE: &str =
    "tutte le connessioni MCP di questa istanza sono occupate: chiudi un client e riprova";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub version: String,
    pub project_root: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Reply(String),
    Initialized {
        reply: String,
        client: String,
    },
    Call {
        id: McpId,
        tool: ToolName,
        arguments: crate::catalog::Args,
    },
    Cancelled(McpId),
    Nothing,
}

#[derive(Debug, Default)]
pub struct Session {
    initialize_requested: bool,
    initialized: bool,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn handle(
        &mut self,
        line: &[u8],
        info: &ServerInfo,
        availability: Availability,
    ) -> Outcome {
        let value: Value = match serde_json::from_slice(line) {
            Ok(value) => value,
            Err(_) => return Outcome::Reply(reply_error(None, PARSE_ERROR, "Parse error")),
        };
        let Some(object) = value.as_object() else {
            return Outcome::Reply(reply_error(None, INVALID_REQUEST, "Invalid Request"));
        };
        let id = match object.get("id") {
            Some(Value::Number(number)) => number.as_i64().map(McpId::Num),
            Some(Value::String(text)) => Some(McpId::Str(text.clone())),
            Some(_) => {
                return Outcome::Reply(reply_error(None, INVALID_REQUEST, "Invalid Request"));
            }
            None => None,
        };
        let has_id = object.contains_key("id");
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return if has_id {
                Outcome::Reply(reply_error(id.as_ref(), INVALID_REQUEST, "Invalid Request"))
            } else {
                Outcome::Nothing
            };
        };
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || (has_id && id.is_none())
        {
            return Outcome::Reply(reply_error(id.as_ref(), INVALID_REQUEST, "Invalid Request"));
        }
        if !has_id {
            if method == "notifications/initialized" && self.initialize_requested {
                self.initialized = true;
            }
            if method == "notifications/cancelled" && self.initialized {
                let request_id = object
                    .get("params")
                    .and_then(Value::as_object)
                    .and_then(|params| params.get("requestId"));
                let id = match request_id {
                    Some(Value::Number(number)) => number.as_i64().map(McpId::Num),
                    Some(Value::String(text)) => Some(McpId::Str(text.clone())),
                    _ => None,
                };
                if let Some(id) = id {
                    return Outcome::Cancelled(id);
                }
            }
            return Outcome::Nothing;
        }
        let Some(id) = id else {
            return Outcome::Reply(reply_error(None, INVALID_REQUEST, "Invalid Request"));
        };
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Outcome::Reply(reply_error(Some(&id), INVALID_PARAMS, "Invalid params"));
        }
        match method {
            "initialize" => {
                if self.initialize_requested {
                    return Outcome::Reply(reply_error(
                        Some(&id),
                        INVALID_REQUEST,
                        "Already initialized",
                    ));
                }
                let requested = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL_VERSION);
                let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
                    requested
                } else {
                    PROTOCOL_VERSION
                };
                let client = params
                    .get("clientInfo")
                    .and_then(|v| v.get("name"))
                    .and_then(Value::as_str)
                    .map(sanitize_client_name)
                    .unwrap_or_else(|| "sconosciuto".to_owned());
                self.initialize_requested = true;
                let result = json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": true}},
                    "serverInfo": {"name": "Lume", "version": info.version, "project_root": info.project_root},
                    "instructions": format!("Alcuni tool possono modificare il progetto o eseguire comandi. La modalità Non brave / Auto / Brave decide quando chiedere conferma in Lume. Le scritture fuori dalla radice e sotto .git sono sempre vietate. Radice: {}.", info.project_root)
                });
                Outcome::Initialized {
                    reply: reply_ok(&id, result),
                    client,
                }
            }
            "ping" => Outcome::Reply(reply_ok(&id, json!({}))),
            _ if !self.initialized => Outcome::Reply(reply_error(
                Some(&id),
                INVALID_REQUEST,
                "sessione non inizializzata: manda prima initialize",
            )),
            "tools/list" => Outcome::Reply(reply_ok(&id, tools_list(availability))),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    return Outcome::Reply(reply_error(
                        Some(&id),
                        INVALID_PARAMS,
                        "Missing tool name",
                    ));
                };
                let Some(tool) =
                    ToolName::from_name(name).filter(|tool| tool.is_available(availability))
                else {
                    return Outcome::Reply(reply_error(
                        Some(&id),
                        INVALID_PARAMS,
                        "Unknown or unavailable tool",
                    ));
                };
                let arguments = match params.get("arguments") {
                    None | Some(Value::Null) => Args::new(),
                    Some(Value::Object(map)) => map.clone(),
                    Some(_) => {
                        return Outcome::Reply(reply_error(
                            Some(&id),
                            INVALID_PARAMS,
                            "Tool arguments must be an object",
                        ));
                    }
                };
                Outcome::Call {
                    id,
                    tool,
                    arguments,
                }
            }
            _ => Outcome::Reply(reply_error(Some(&id), METHOD_NOT_FOUND, "Method not found")),
        }
    }
}

pub fn reply_ok(id: &McpId, result: Value) -> String {
    serde_json::to_string(&json!({"jsonrpc": JsonRpcVersion::V2, "id": id, "result": result}))
        .unwrap_or_else(|_| String::from("{}"))
}

pub fn reply_error(id: Option<&McpId>, code: i64, message: &str) -> String {
    let error = RpcError {
        code,
        message: message.to_owned(),
        data: None,
    };
    serde_json::to_string(&json!({"jsonrpc": JsonRpcVersion::V2, "id": id, "error": error}))
        .unwrap_or_else(|_| String::from("{}"))
}

/// Riga JSON-RPC con cui il server rifiuta una connessione **dopo** averne
/// verificato il preambolo, quando `MAX_INSTANCES` client sono già autenticati.
/// Il server non ha ancora letto nessuna richiesta (l'`id` è nullo) e non
/// scrive byte prima del preambolo: questa riga è l'unico segnale con cui il
/// ponte distingue «occupato» da una chiusura di trasporto.
pub fn busy_line() -> String {
    reply_error(None, SERVER_BUSY, BUSY_MESSAGE)
}

pub fn list_changed_notification() -> String {
    serde_json::to_string(
        &json!({"jsonrpc": JsonRpcVersion::V2, "method": "notifications/tools/list_changed"}),
    )
    .unwrap_or_else(|_| String::from("{}"))
}

pub fn tool_result(result: Result<Value, ToolError>) -> Value {
    let (value, is_error) = match result {
        Ok(value) => (
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            false,
        ),
        Err(error) => (error.to_string(), true),
    };
    json!({"content": [{"type": "text", "text": value}], "isError": is_error})
}

pub fn sanitize_client_name(raw: &str) -> String {
    let clean: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_graphic() || *ch == ' ')
        .take(32)
        .collect();
    let clean = clean.trim().to_owned();
    if clean.is_empty() {
        "sconosciuto".to_owned()
    } else {
        clean
    }
}
