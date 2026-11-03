//! Risposte del client alle richieste avviate dal server.
use crate::RpcRequest;
use serde_json::{Value, json};

pub const METHOD_NOT_FOUND: i64 = -32601;
#[derive(Debug, Clone, PartialEq)]
pub enum ServerReply {
    Result(Value),
    Error { code: i64, message: String },
}

pub fn reply_to_server_request(request: &RpcRequest) -> ServerReply {
    match request.method.as_str() {
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "window/showMessageRequest" => ServerReply::Result(Value::Null),
        "workspace/configuration" => {
            let count = request
                .params
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            ServerReply::Result(Value::Array(vec![Value::Null; count]))
        }
        "workspace/applyEdit" => ServerReply::Result(
            json!({"applied":false,"failureReason":"Lume non applica modifiche richieste dal server"}),
        ),
        "window/showDocument" => ServerReply::Result(json!({"success":false})),
        method => ServerReply::Error {
            code: METHOD_NOT_FOUND,
            message: format!("metodo non gestito dal client: {method}"),
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{METHOD_NOT_FOUND, ServerReply, reply_to_server_request};
    use crate::RpcRequest;
    use serde_json::{Value, json};

    fn richiesta(method: &str, params: Value) -> RpcRequest {
        serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .unwrap()
    }
    #[test]
    fn registrazione_e_creazione_del_gettone_ricevono_null() {
        for metodo in [
            "client/registerCapability",
            "client/unregisterCapability",
            "window/workDoneProgress/create",
            "window/showMessageRequest",
        ] {
            assert_eq!(
                reply_to_server_request(&richiesta(metodo, json!({}))),
                ServerReply::Result(Value::Null),
                "{metodo}"
            );
        }
    }
    #[test]
    fn la_configurazione_riceve_un_null_per_voce_richiesta() {
        let params = json!({"items":[{"section":"a"},{"section":"b"},{}]});
        assert_eq!(
            reply_to_server_request(&richiesta("workspace/configuration", params)),
            ServerReply::Result(json!([null, null, null]))
        );
        assert_eq!(
            reply_to_server_request(&richiesta("workspace/configuration", json!({}))),
            ServerReply::Result(json!([]))
        );
    }
    #[test]
    fn le_modifiche_e_i_documenti_chiesti_dal_server_si_rifiutano_con_garbo() {
        match reply_to_server_request(&richiesta("workspace/applyEdit", json!({"edit":{}}))) {
            ServerReply::Result(value) => assert_eq!(value["applied"], json!(false)),
            other => panic!("attesa risposta, visto {other:?}"),
        }
        assert_eq!(
            reply_to_server_request(&richiesta(
                "window/showDocument",
                json!({"uri":"file:///a"})
            )),
            ServerReply::Result(json!({"success":false}))
        );
    }
    #[test]
    fn un_metodo_sconosciuto_riceve_un_errore_non_il_silenzio() {
        match reply_to_server_request(&richiesta("metodo/che-non-esiste", json!({}))) {
            ServerReply::Error { code, message } => {
                assert_eq!(code, METHOD_NOT_FOUND);
                assert!(message.contains("metodo/che-non-esiste"));
            }
            other => panic!("atteso errore, visto {other:?}"),
        }
    }
}
