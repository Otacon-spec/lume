//! L'id di una richiesta JSON-RPC come lo manda un client MCP.
//!
//! Non si riusa `lume_lsp::RequestId` (`pub i64`): JSON-RPC 2.0 ammette anche
//! le stringhe, e un client che le usa verrebbe rifiutato alla prima richiesta.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpId {
    Num(i64),
    Str(String),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::McpId;
    use serde_json::json;

    /// Un client con id stringa verrebbe rifiutato da `RequestId(i64)` di
    /// lume-lsp: è il motivo per cui questo tipo esiste.
    #[test]
    fn accetta_numeri_e_stringhe_e_li_rimanda_uguali() {
        let numero: McpId = serde_json::from_value(json!(7)).unwrap();
        let testo: McpId = serde_json::from_value(json!("abc-1")).unwrap();
        assert_eq!(numero, McpId::Num(7));
        assert_eq!(testo, McpId::Str("abc-1".to_owned()));
        assert_eq!(serde_json::to_value(&numero).unwrap(), json!(7));
        assert_eq!(serde_json::to_value(&testo).unwrap(), json!("abc-1"));
    }

    #[test]
    fn rifiuta_quel_che_json_rpc_non_ammette_come_id() {
        for cattivo in [
            json!(1.5),
            json!(true),
            json!(null),
            json!({"a": 1}),
            json!([1]),
        ] {
            assert!(
                serde_json::from_value::<McpId>(cattivo.clone()).is_err(),
                "{cattivo}"
            );
        }
    }
}
