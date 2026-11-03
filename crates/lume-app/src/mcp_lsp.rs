use std::collections::BTreeMap;
use std::path::Path;

use lsp_types::{Hover, HoverContents, MarkedString, Position, Range, Uri};
use lume_lsp::LspRequest;
use lume_mcp::AppQuery;
use ropey::Rope;
use serde_json::{Value, json};

pub const MAX_RESULTS: usize = 200;
pub const MAX_SYMBOLS: usize = 1000;
pub const MAX_HOVER_CHARS: usize = 16_384;
pub const MAX_NEW_NAME_CHARS: usize = 256;

#[derive(Debug, Default)]
pub struct Waits {
    by_request: BTreeMap<(String, u64), u64>,
}

impl Waits {
    pub fn insert(&mut self, server: &str, request: u64, mcp_id: u64) {
        self.by_request.insert((server.to_owned(), request), mcp_id);
    }

    pub fn take(&mut self, server: &str, request: u64) -> Option<u64> {
        self.by_request.remove(&(server.to_owned(), request))
    }

    pub fn drain_server(&mut self, server: &str) -> Vec<u64> {
        let keys: Vec<_> = self
            .by_request
            .keys()
            .filter(|(owner, _)| owner == server)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| self.by_request.remove(&key))
            .collect()
    }

    pub fn drain_all(&mut self) -> Vec<(String, u64, u64)> {
        std::mem::take(&mut self.by_request)
            .into_iter()
            .map(|((server, request), mcp_id)| (server, request, mcp_id))
            .collect()
    }

    pub fn remove_mcp(&mut self, mcp_id: u64) -> Vec<(String, u64)> {
        let keys: Vec<_> = self
            .by_request
            .iter()
            .filter(|(_, owner)| **owner == mcp_id)
            .map(|(key, _)| key.clone())
            .collect();
        keys.into_iter()
            .filter(|key| self.by_request.remove(key).is_some())
            .collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_request.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.by_request.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hover,
    Definition,
    References,
    Symbols,
}

pub type Query<'a> = (Kind, &'a Path, Option<(u32, u32)>);

impl Kind {
    pub fn of(query: &AppQuery) -> Option<Query<'_>> {
        match query {
            AppQuery::Hover {
                relative,
                line,
                column,
            } => Some((Self::Hover, relative, Some((*line, *column)))),
            AppQuery::Definition {
                relative,
                line,
                column,
            } => Some((Self::Definition, relative, Some((*line, *column)))),
            AppQuery::References {
                relative,
                line,
                column,
            } => Some((Self::References, relative, Some((*line, *column)))),
            AppQuery::DocumentSymbols { relative } => Some((Self::Symbols, relative, None)),
            _ => None,
        }
    }

    pub const fn tool(self) -> &'static str {
        match self {
            Self::Hover => "get_hover",
            Self::Definition => "goto_definition",
            Self::References => "find_usages",
            Self::Symbols => "get_document_symbols",
        }
    }

    pub fn supported_by(self, lsp: &crate::lsp_mode::LspWorkspace) -> bool {
        match self {
            Self::Hover => lsp.can_hover(),
            Self::Definition => lsp.can_define(),
            Self::References => lsp.can_find_usages(),
            Self::Symbols => lsp.can_symbols(),
        }
    }

    pub fn request(self, uri: Uri, position: Position) -> LspRequest {
        match self {
            Self::Hover => LspRequest::Hover { uri, position },
            Self::Definition => LspRequest::Definition { uri, position },
            Self::References => LspRequest::References { uri, position },
            Self::Symbols => LspRequest::DocumentSymbols { uri },
        }
    }
}

/// Converte riga/colonna 1-based dell'agente in UTF-16 LSP senza saturare
/// posizioni fuori testo o tagliare una coppia surrogata.
pub fn position_from_query(rope: &Rope, line: u32, column: u32) -> Result<Position, String> {
    if line == 0 {
        return Err("line parte da 1".to_owned());
    }
    if column == 0 {
        return Err("column parte da 1".to_owned());
    }
    let line_index = usize::try_from(line - 1).map_err(|_| format!("riga {line} fuori testo"))?;
    let Some(text_line) = rope.get_line(line_index) else {
        return Err(format!("riga {line} fuori testo"));
    };
    let mut chars = text_line.chars().collect::<Vec<_>>();
    if chars.last() == Some(&'\n') {
        chars.pop();
    }
    if chars.last() == Some(&'\r') {
        chars.pop();
    }
    let target = column - 1;
    let mut utf16 = 0_u32;
    for ch in chars {
        if utf16 == target {
            return Ok(Position::new(line - 1, target));
        }
        let next = utf16.saturating_add(ch.len_utf16() as u32);
        if target < next {
            return Err(format!("column {column} cade in una coppia UTF-16"));
        }
        utf16 = next;
    }
    if utf16 == target {
        Ok(Position::new(line - 1, target))
    } else {
        Err(format!("column {column} oltre la fine della riga {line}"))
    }
}

fn hover_text(hover: &Hover) -> Option<String> {
    fn marked_text(marked: &MarkedString) -> String {
        match marked {
            MarkedString::String(text) => text.clone(),
            MarkedString::LanguageString(code) => {
                format!("```{}\n{}\n```", code.language, code.value)
            }
        }
    }
    let pieces: Vec<String> = match &hover.contents {
        HoverContents::Scalar(marked) => vec![marked_text(marked)],
        HoverContents::Array(marked) => marked.iter().map(marked_text).collect(),
        HoverContents::Markup(markup) => vec![markup.value.clone()],
    };
    let text = pieces
        .iter()
        .map(|piece| piece.trim())
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

pub fn hover_json(hover: Option<&Hover>) -> Value {
    let Some(text) = hover.and_then(hover_text) else {
        return json!({"found": false});
    };
    let truncated = text.chars().count() > MAX_HOVER_CHARS;
    let contents: String = text.chars().take(MAX_HOVER_CHARS).collect();
    json!({"found": true, "contents": contents, "truncated": truncated})
}

fn range_json(range: &Range) -> Value {
    json!({
        "line": u64::from(range.start.line) + 1,
        "column": u64::from(range.start.character) + 1,
        "end_line": u64::from(range.end.line) + 1,
        "end_column": u64::from(range.end.character) + 1,
    })
}

pub fn location_json(root: &Path, uri: &Uri, range: &Range) -> Value {
    let mut value = range_json(range);
    let relative = crate::lsp_uri::uri_to_path(uri)
        .and_then(|path| crate::mcp_mode::relative_to_root(root, &path));
    if let Some(object) = value.as_object_mut() {
        match relative {
            Some(relative) => {
                object.insert(
                    "path".to_owned(),
                    json!(lume_mcp::confine::to_slash(&relative)),
                );
            }
            None => {
                object.insert("outside_root".to_owned(), json!(true));
                object.insert("uri".to_owned(), json!(uri.as_str()));
            }
        }
    }
    value
}

pub fn definitions_json(root: &Path, targets: Vec<(Uri, Range)>) -> Value {
    let definitions: Vec<Value> = targets
        .iter()
        .take(MAX_RESULTS)
        .map(|(uri, range)| location_json(root, uri, range))
        .collect();
    json!({"definitions": definitions})
}

pub fn references_json(root: &Path, locations: Option<Vec<lsp_types::Location>>) -> Value {
    let locations = locations.unwrap_or_default();
    let total = locations.len();
    let references: Vec<Value> = locations
        .iter()
        .take(MAX_RESULTS)
        .map(|location| location_json(root, &location.uri, &location.range))
        .collect();
    json!({"references": references, "total": total, "truncated": total > MAX_RESULTS})
}

pub const fn kind_name(kind: lsp_types::SymbolKind) -> &'static str {
    use lsp_types::SymbolKind;
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE => "module",
        SymbolKind::NAMESPACE => "namespace",
        SymbolKind::PACKAGE => "package",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY => "property",
        SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "function",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::STRING => "string",
        SymbolKind::NUMBER => "number",
        SymbolKind::BOOLEAN => "boolean",
        SymbolKind::ARRAY => "array",
        SymbolKind::OBJECT => "object",
        SymbolKind::KEY => "key",
        SymbolKind::NULL => "null",
        SymbolKind::ENUM_MEMBER => "enum_member",
        SymbolKind::STRUCT => "struct",
        SymbolKind::EVENT => "event",
        SymbolKind::OPERATOR => "operator",
        SymbolKind::TYPE_PARAMETER => "type_parameter",
        _ => "other",
    }
}

pub fn symbols_json(rows: &[crate::lsp_mode::OutlineRow]) -> Value {
    let symbols: Vec<Value> = rows
        .iter()
        .take(MAX_SYMBOLS)
        .map(|row| {
            let mut value = json!({
                "name": row.name,
                "kind": kind_name(row.kind),
                "depth": row.depth,
                "line": u64::from(row.selection.start.line) + 1,
                "column": u64::from(row.selection.start.character) + 1,
                "end_line": u64::from(row.range.end.line) + 1,
            });
            if let (Some(object), Some(detail)) = (value.as_object_mut(), row.detail.as_ref()) {
                object.insert("detail".to_owned(), json!(detail));
            }
            value
        })
        .collect();
    json!({"symbols": symbols, "total": rows.len(), "truncated": rows.len() > MAX_SYMBOLS})
}

/// Valida il nome prima di inviare una richiesta al language server.
pub fn check_new_name(new_name: &str) -> Result<(), String> {
    if new_name.is_empty() {
        return Err("new_name è vuoto".to_owned());
    }
    if new_name.chars().count() > MAX_NEW_NAME_CHARS {
        return Err(format!("new_name supera i {MAX_NEW_NAME_CHARS} caratteri"));
    }
    if new_name
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("new_name non può contenere spazi né caratteri di controllo".to_owned());
    }
    Ok(())
}

/// Crea la risposta di una rinomina: cambia i buffer, il disco resta
/// invariato. I percorsi ricevuti sono già relativi e confinati.
pub fn rename_json(files: &[(String, usize)]) -> Value {
    let occurrences: usize = files.iter().map(|(_, count)| *count).sum();
    let listed: Vec<Value> = files
        .iter()
        .take(MAX_RESULTS)
        .map(|(path, count)| json!({"path": path, "occurrences": count}))
        .collect();
    json!({
        "renamed": true,
        "occurrences": occurrences,
        "files": listed,
        "files_total": files.len(),
        "truncated": files.len() > MAX_RESULTS,
        "saved": false,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use lsp_types::{LanguageString, Location, MarkupContent, MarkupKind};
    use ropey::Rope;

    fn range(l0: u32, c0: u32, l1: u32, c1: u32) -> Range {
        Range {
            start: Position::new(l0, c0),
            end: Position::new(l1, c1),
        }
    }

    fn markup(testo: &str) -> Hover {
        Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: testo.to_owned(),
            }),
            range: None,
        }
    }

    #[test]
    fn la_posizione_pubblica_e_1_based_e_utf16() {
        let rope = Rope::from_str("a😀b\nsecondo\n");
        assert_eq!(
            position_from_query(&rope, 1, 1).unwrap(),
            Position::new(0, 0)
        );
        assert_eq!(
            position_from_query(&rope, 1, 4).unwrap(),
            Position::new(0, 3)
        );
        assert_eq!(
            position_from_query(&rope, 2, 3).unwrap(),
            Position::new(1, 2)
        );
        assert_eq!(
            position_from_query(&rope, 1, 5).unwrap(),
            Position::new(0, 4)
        );
    }

    #[test]
    fn rifiuta_riga_colonna_e_posizione_in_mezzo_all_emoji() {
        let rope = Rope::from_str("a😀b\nsecondo\n");
        assert!(position_from_query(&rope, 1, 3).is_err());
        assert!(position_from_query(&rope, 1, 99).is_err());
        assert!(position_from_query(&rope, 9, 1).is_err());
        assert!(position_from_query(&rope, 0, 1).is_err());
        assert!(position_from_query(&rope, 1, 0).is_err());
    }

    #[test]
    fn attese_sono_identificate_da_server_e_richiesta() {
        let mut waits = Waits::default();
        waits.insert("rust-analyzer", 1, 10);
        waits.insert("tombi", 1, 11);
        waits.insert("rust-analyzer", 2, 12);
        assert_eq!(waits.len(), 3);
        assert_eq!(waits.take("tombi", 1), Some(11));
        assert_eq!(waits.take("tombi", 1), None);
        assert_eq!(waits.drain_server("rust-analyzer"), [10, 12]);
        assert!(waits.is_empty());
    }

    #[test]
    fn ritira_tutte_le_attese_di_una_domanda_mcp_senza_toccare_le_altre() {
        let mut waits = Waits::default();
        waits.insert("rust-analyzer", 1, 10);
        waits.insert("tombi", 2, 10);
        waits.insert("rust-analyzer", 3, 11);
        assert_eq!(
            waits.remove_mcp(10),
            [("rust-analyzer".to_owned(), 1), ("tombi".to_owned(), 2)]
        );
        assert_eq!(waits.len(), 1);
        assert_eq!(waits.remove_mcp(10), []);
    }

    #[test]
    fn hover_supporta_le_forme_lsp_e_limita_la_dimensione() {
        let scalar = Hover {
            contents: HoverContents::Scalar(MarkedString::String("un intero".into())),
            range: None,
        };
        assert_eq!(hover_json(Some(&scalar))["contents"], "un intero");
        let array = Hover {
            contents: HoverContents::Array(vec![
                MarkedString::LanguageString(LanguageString {
                    language: "rust".into(),
                    value: "let x".into(),
                }),
                MarkedString::String(" documentazione ".into()),
            ]),
            range: None,
        };
        assert_eq!(
            hover_json(Some(&array))["contents"],
            "```rust\nlet x\n```\n\ndocumentazione"
        );
        assert_eq!(hover_json(None), json!({"found": false}));
        let long = markup(&"x".repeat(MAX_HOVER_CHARS + 100));
        let value = hover_json(Some(&long));
        assert_eq!(
            value["contents"].as_str().unwrap().chars().count(),
            MAX_HOVER_CHARS
        );
        assert_eq!(value["truncated"], true);
    }

    #[test]
    fn i_risultati_sono_relativi_e_fuori_radice_non_esportano_percorsi() {
        let root = std::env::temp_dir().join("lume-mcp-lsp-tests");
        let inside = crate::lsp_uri::path_to_uri(&root.join("src").join("lib.rs")).unwrap();
        let outside_path = std::env::temp_dir().join("another-project/lib.rs");
        let outside = crate::lsp_uri::path_to_uri(&outside_path).unwrap();
        let value = location_json(&root, &inside, &range(2, 3, 2, 8));
        assert_eq!(value["path"], "src/lib.rs");
        assert_eq!(value["line"], 3);
        assert_eq!(value["column"], 4);
        let external = location_json(&root, &outside, &range(0, 0, 0, 1));
        assert_eq!(external["outside_root"], true);
        assert!(external.get("path").is_none());
        assert_eq!(external["uri"], outside.as_str());
    }

    #[test]
    fn riferimenti_hanno_un_tetto_esplicito() {
        let root = std::env::temp_dir().join("lume-mcp-lsp-tests");
        let uri = crate::lsp_uri::path_to_uri(&root.join("src/lib.rs")).unwrap();
        let locations = (0..MAX_RESULTS + 4)
            .map(|line| Location {
                uri: uri.clone(),
                range: range(line as u32, 0, line as u32, 1),
            })
            .collect();
        let value = references_json(&root, Some(locations));
        assert_eq!(value["references"].as_array().unwrap().len(), MAX_RESULTS);
        assert_eq!(value["total"], MAX_RESULTS + 4);
        assert_eq!(value["truncated"], true);
    }

    #[test]
    fn il_nome_nuovo_e_validato_prima_della_richiesta_al_server() {
        for good in ["main", "r#tipo", "nome_è_valido", "X1"] {
            assert!(check_new_name(good).is_ok(), "{good:?}");
        }
        for bad in ["", " ", "due parole", "a\nb", "tab\t", "a\0b", "\u{a0}"] {
            assert!(check_new_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_new_name(&"a".repeat(MAX_NEW_NAME_CHARS)).is_ok());
        let error = check_new_name(&"a".repeat(MAX_NEW_NAME_CHARS + 1)).unwrap_err();
        assert!(error.contains("256"), "{error}");
    }

    #[test]
    fn la_risposta_rename_elenca_file_con_tetto_e_dichiara_non_salvato() {
        let value = rename_json(&[("src/a.rs".to_owned(), 3), ("main.rs".to_owned(), 1)]);
        assert_eq!(value["renamed"], true);
        assert_eq!(value["occurrences"], 4);
        assert_eq!(value["files_total"], 2);
        assert_eq!(value["truncated"], false);
        assert_eq!(value["saved"], false);
        assert_eq!(value["files"][0]["path"], "src/a.rs");
        assert_eq!(value["files"][0]["occurrences"], 3);

        let many: Vec<(String, usize)> = (0..250).map(|n| (format!("f{n}.rs"), 1)).collect();
        let value = rename_json(&many);
        assert_eq!(value["files"].as_array().unwrap().len(), MAX_RESULTS);
        assert_eq!(value["files_total"], 250);
        assert_eq!(value["occurrences"], 250);
        assert_eq!(value["truncated"], true);
    }
}
