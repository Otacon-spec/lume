//! Presentazione limitata delle firme LSP nell'editor.

use lume_ui::editor::{HoverCard, HoverLine};

const MAX_DOC_CHARS: usize = 400;
const MAX_DOC_LINES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelpWait {
    pub server: String,
    pub request: u64,
    pub doc: lume_core::DocId,
    pub version: u64,
    pub anchor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelpShown {
    pub doc: lume_core::DocId,
    pub card: HoverCard,
}

pub fn card(help: &lsp_types::SignatureHelp, anchor: usize) -> Option<HoverCard> {
    let active_signature = help.active_signature.unwrap_or(0) as usize;
    let signature = help.signatures.get(active_signature)?;
    let mut lines = vec![HoverLine {
        text: bounded(&signature.label, lume_ui::editor::CARD_MAX_COLS),
        code: true,
    }];
    let active_parameter = signature
        .active_parameter
        .or(help.active_parameter)
        .and_then(|index| usize::try_from(index).ok());
    if let Some(parameter) =
        active_parameter.and_then(|index| signature.parameters.as_ref()?.get(index))
    {
        let label = match &parameter.label {
            lsp_types::ParameterLabel::Simple(label) => label.clone(),
            lsp_types::ParameterLabel::LabelOffsets([from, to]) => {
                let from = usize::try_from(*from).ok()?;
                let to = usize::try_from(*to).ok()?;
                let from = byte_offset_for_utf16(&signature.label, from)?;
                let to = byte_offset_for_utf16(&signature.label, to)?;
                signature.label.get(from..to)?.to_owned()
            }
        };
        lines.push(HoverLine {
            text: format!("▶ {label}"),
            code: false,
        });
        if let Some(documentation) = parameter.documentation.as_ref() {
            lines.push(HoverLine {
                text: documentation_text(documentation),
                code: false,
            });
        }
    }
    Some(HoverCard { anchor, lines })
}

fn byte_offset_for_utf16(text: &str, target: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        if units == target {
            return Some(byte);
        }
        units += character.len_utf16();
    }
    (units == target).then_some(text.len())
}

fn documentation_text(documentation: &lsp_types::Documentation) -> String {
    let text = match documentation {
        lsp_types::Documentation::String(text) => text.as_str(),
        lsp_types::Documentation::MarkupContent(markup) => markup.value.as_str(),
    };
    text.lines()
        .take(MAX_DOC_LINES)
        .map(|line| bounded(line, lume_ui::editor::CARD_MAX_COLS))
        .collect::<Vec<_>>()
        .join("\n")
}

fn bounded(text: &str, max: usize) -> String {
    lume_ui::editor::clip_text(text, max.min(MAX_DOC_CHARS))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::card;

    #[test]
    fn mostra_firma_e_parametro_attivo() {
        let help: lsp_types::SignatureHelp = serde_json::from_value(serde_json::json!({
            "signatures": [{
                "label": "scrivi(nome, valore)",
                "parameters": [
                    { "label": "nome" },
                    { "label": "valore", "documentation": "testo da scrivere" }
                ]
            }],
            "activeSignature": 0,
            "activeParameter": 1
        }))
        .expect("risposta LSP valida");
        let card = card(&help, 5).expect("firma presente");
        assert_eq!(card.anchor, 5);
        assert_eq!(card.lines[0].text, "scrivi(nome, valore)");
        assert_eq!(card.lines[1].text, "▶ valore");
        assert_eq!(card.lines[2].text, "testo da scrivere");
    }

    #[test]
    fn interpreta_gli_offset_lsp_in_unita_utf16() {
        let help: lsp_types::SignatureHelp = serde_json::from_value(serde_json::json!({
            "signatures": [{
                "label": "call(😀, value)",
                "parameters": [{ "label": [9, 14] }]
            }],
            "activeSignature": 0,
            "activeParameter": 0
        }))
        .expect("risposta LSP valida");
        let card = card(&help, 0).expect("firma presente");
        assert_eq!(card.lines[1].text, "▶ value");
    }
}
