use crate::{CodeLink, Note, NoteError, NoteId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedNote {
    pub note: Note,
    pub original_bytes: Vec<u8>,
    pub had_front_matter: bool,
    pub read_only_error: Option<NoteError>,
}

#[derive(Deserialize, Serialize)]
struct HeaderWire {
    format: u32,
    id: String,
    title: String,
    updated_unix_ms: u64,
    paper: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<LinkWire>,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize, Serialize)]
struct LinkWire {
    workspace: String,
    path: String,
    from: usize,
    to: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

pub fn parse(path: &Path, bytes: &[u8]) -> Result<ParsedNote, NoteError> {
    let markdown = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    let has_front_matter_opener = markdown.starts_with("+++\n");
    let front_matter = delimited_front_matter(&markdown);
    let had_front_matter = has_front_matter_opener;
    let body = front_matter.map_or(markdown.as_str(), |(_, body)| body);
    let parsed_header = front_matter.map(|(header, _)| toml::from_str::<HeaderWire>(header));
    let mut read_only_error = if has_front_matter_opener && front_matter.is_none() {
        Some(NoteError::InvalidFrontMatter(
            "missing exact closing delimiter".to_owned(),
        ))
    } else {
        parsed_header
            .as_ref()
            .and_then(|header| header.as_ref().err())
            .map(|error| NoteError::InvalidFrontMatter(error.to_string()))
    };
    let title = title_from_markdown(body)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Senza titolo".to_owned());
    let mut note = Note::empty(NoteId::new());
    note.header.title = title;
    note.markdown = if read_only_error.is_some() {
        markdown.clone()
    } else {
        body.to_owned()
    };
    note.path = Some(PathBuf::from(path));
    if let Some(Ok(header)) = parsed_header {
        let links = header
            .links
            .into_iter()
            .map(|link| {
                let mut code_link = CodeLink::try_new(
                    link.workspace,
                    link.path,
                    link.from,
                    link.to,
                    link.symbol.as_deref(),
                )?;
                code_link.extra = link.extra;
                Ok::<CodeLink, NoteError>(code_link)
            })
            .collect::<Result<Vec<_>, _>>();
        match links {
            Ok(links) => {
                note.header.format = header.format;
                note.header.id = NoteId::from(header.id);
                note.header.title = header.title;
                note.header.updated_unix_ms = header.updated_unix_ms;
                note.header.paper = header.paper;
                note.header.links = links;
                note.header.extra = header.extra;
            }
            Err(error) => {
                note.markdown = markdown.clone();
                read_only_error = Some(NoteError::InvalidFrontMatter(error.to_string()));
            }
        }
    }

    Ok(ParsedNote {
        note,
        original_bytes: bytes.to_vec(),
        had_front_matter,
        read_only_error,
    })
}

/// Il primo titolo ATX fuori dai blocchi di codice recintati.
#[must_use]
pub fn title_from_markdown(markdown: &str) -> Option<String> {
    let mut fence: Option<char> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some('`')
        } else if trimmed.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        match (fence, marker) {
            (None, Some(open)) => fence = Some(open),
            (Some(open), Some(close)) if open == close => fence = None,
            (None, None) => {
                if let Some(title) = atx_heading_title(line) {
                    return Some(title.to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

fn atx_heading_title(line: &str) -> Option<&str> {
    let marker_count = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&marker_count) || line.as_bytes().get(marker_count) != Some(&b' ') {
        return None;
    }
    line.get(marker_count + 1..)
        .map(str::trim)
        .filter(|title| !title.is_empty())
}

pub fn serialize(note: &Note) -> Result<Vec<u8>, NoteError> {
    let header = HeaderWire {
        format: note.header.format,
        id: note.header.id.as_str().to_owned(),
        title: note.header.title.clone(),
        updated_unix_ms: note.header.updated_unix_ms,
        paper: note.header.paper,
        links: note
            .header
            .links
            .iter()
            .map(|link| LinkWire {
                workspace: link.workspace.to_string_lossy().into_owned(),
                path: link.path.to_string_lossy().replace('\\', "/"),
                from: link.from,
                to: link.to,
                symbol: link.symbol.clone(),
                extra: link.extra.clone(),
            })
            .collect(),
        extra: note.header.extra.clone(),
    };
    let front_matter =
        toml::to_string(&header).map_err(|error| NoteError::Serialize(error.to_string()))?;
    Ok(format!("+++\n{front_matter}+++\n{}", note.markdown).into_bytes())
}

fn delimited_front_matter(markdown: &str) -> Option<(&str, &str)> {
    let rest = markdown.strip_prefix("+++\n")?;
    let end = rest.find("\n+++\n")?;
    let header = rest.get(..end)?;
    let body = rest.get(end + "\n+++\n".len()..)?;
    Some((header, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse_ok(path: &Path, bytes: &[u8]) -> ParsedNote {
        match parse(path, bytes) {
            Ok(parsed) => parsed,
            Err(error) => panic!("parse failed: {error}"),
        }
    }

    fn serialize_ok(note: &Note) -> Vec<u8> {
        match serialize(note) {
            Ok(bytes) => bytes,
            Err(error) => panic!("serialization failed: {error}"),
        }
    }

    #[test]
    fn markdown_normale_senza_front_matter_e_importabile() {
        let parsed = parse_ok(Path::new("idea.md"), b"# Idea\n\nTesto **forte**.\n");
        assert_eq!(parsed.note.header.title, "Idea");
        assert_eq!(parsed.note.markdown, "# Idea\n\nTesto **forte**.\n");
        assert!(!parsed.had_front_matter);
    }

    #[test]
    fn il_titolo_e_il_primo_titolo_fuori_dai_blocchi_di_codice() {
        assert_eq!(
            title_from_markdown("intro\n# Idea\n## Sotto").as_deref(),
            Some("Idea")
        );
        assert_eq!(
            title_from_markdown("```sh\n# commento\n```\n## Vero\n").as_deref(),
            Some("Vero")
        );
        assert_eq!(title_from_markdown("~~~\n# no\n~~~\n"), None);
        assert_eq!(title_from_markdown("#senza-spazio\n"), None);
        assert_eq!(title_from_markdown("# \n"), None);
        assert_eq!(title_from_markdown(""), None);
    }

    #[test]
    fn parse_non_prende_un_commento_di_codice_per_titolo() {
        let parsed = parse_ok(Path::new("nota.md"), b"```\n# commento\n```\n");
        assert_eq!(parsed.note.header.title, "nota");
    }

    #[test]
    fn front_matter_malformato_non_diventa_un_file_riscrivibile() {
        let parsed = parse_ok(Path::new("rotta.md"), b"+++\nid = [\n+++\ncorpo");
        assert!(parsed.read_only_error.is_some());
        assert_eq!(parsed.original_bytes, b"+++\nid = [\n+++\ncorpo");
    }

    #[test]
    fn link_non_valido_restituisce_la_nota_originale_in_sola_lettura() {
        let invalid_links = [
            "from = 0\nto = 7",
            "path = \"../outside.rs\"\nfrom = 4\nto = 7",
            "path = \"C:/absolute.rs\"\nfrom = 4\nto = 7",
            "path = \"C:drive-relative.rs\"\nfrom = 4\nto = 7",
            "path = \"\\\\\\\\server\\\\share\\\\file.rs\"\nfrom = 4\nto = 7",
        ];

        for invalid_link in invalid_links {
            let source = format!(
                "+++\nformat = 1\nid = \"n1\"\ntitle = \"Rotta\"\nupdated_unix_ms = 42\npaper = false\n\n[[links]]\nworkspace = \"C:/repo\"\npath = \"src/valid.rs\"\nfrom = 1\nto = 2\n\n[[links]]\nworkspace = \"C:/repo\"\n{invalid_link}\n+++\nCorpo leggibile\n"
            );

            let parsed = parse_ok(Path::new("rotta.md"), source.as_bytes());
            assert!(parsed.had_front_matter);
            assert!(parsed.read_only_error.is_some(), "accepted {invalid_link}");
            assert_eq!(parsed.original_bytes, source.as_bytes());
            assert_eq!(parsed.note.markdown.as_bytes(), source.as_bytes());
        }
    }

    #[test]
    fn apertura_senza_chiusura_esatta_resta_front_matter_malformato() {
        let malformed_sources: &[&[u8]] = &[
            b"+++\nformat = 1\nid = \"n1\"\ncorpo senza chiusura\n",
            b"+++\nformat = 1\nid = \"n1",
            b"+++\nformat = 1\nid = \"n1\"\n+++",
        ];

        for source in malformed_sources {
            let parsed = parse_ok(Path::new("incompleta.md"), source);
            assert!(parsed.had_front_matter);
            assert!(parsed.read_only_error.is_some());
            assert_eq!(parsed.original_bytes, *source);
            assert_eq!(parsed.note.markdown.as_bytes(), *source);
        }
    }

    #[test]
    fn front_matter_e_corpo_hanno_un_round_trip_esatto() {
        let source = b"+++\nformat = 1\nid = \"n1\"\ntitle = \"Rotta\"\nupdated_unix_ms = 42\npaper = false\n\n[[links]]\nworkspace = \"C:/repo\"\npath = \"src/main.rs\"\nfrom = 4\nto = 7\nsymbol = \"main\"\n+++\n\nCorpo **forte**.\n";
        let parsed = parse_ok(Path::new("rotta-n1.md"), source);

        assert_eq!(serialize_ok(&parsed.note), source);
    }

    #[test]
    fn crlf_viene_normalizzato_prima_della_modifica() {
        let source = b"+++\r\nformat = 1\r\nid = \"n1\"\r\ntitle = \"Rotta\"\r\nupdated_unix_ms = 42\r\npaper = false\r\n+++\r\n\r\nCorpo\r\n";
        let parsed = parse_ok(Path::new("rotta.md"), source);

        assert_eq!(parsed.note.markdown, "\nCorpo\n");
        assert!(parsed.had_front_matter);
        assert_eq!(
            serialize_ok(&parsed.note),
            b"+++\nformat = 1\nid = \"n1\"\ntitle = \"Rotta\"\nupdated_unix_ms = 42\npaper = false\n+++\n\nCorpo\n"
        );
    }

    #[test]
    fn il_primo_heading_atx_vince_anche_con_titoli_duplicati() {
        let parsed = parse_ok(
            Path::new("fallback.md"),
            b"## Titolo ripetuto\n\nTesto\n\n# Titolo successivo\n",
        );

        assert_eq!(parsed.note.header.title, "Titolo ripetuto");
    }

    #[test]
    fn metadati_sconosciuti_sopravvivono_al_salvataggio() {
        let source = b"+++\nformat = 2\nid = \"n1\"\ntitle = \"Futuro\"\nupdated_unix_ms = 42\npaper = false\ncolor = \"ambra\"\n+++\nCorpo\n";
        let parsed = parse_ok(Path::new("futuro.md"), source);
        let serialized = match String::from_utf8(serialize_ok(&parsed.note)) {
            Ok(serialized) => serialized,
            Err(error) => panic!("serialized note was not UTF-8: {error}"),
        };

        assert!(serialized.contains("format = 2"));
        assert!(serialized.contains("color = \"ambra\""));
    }

    #[test]
    fn metadati_sconosciuti_di_ogni_link_sopravvivono_al_salvataggio() {
        let source = b"+++\nformat = 2\nid = \"n1\"\ntitle = \"Futuro\"\nupdated_unix_ms = 42\npaper = false\n\n[[links]]\nworkspace = \"C:/repo\"\npath = \"src/main.rs\"\nfrom = 4\nto = 7\nkind = \"symbol\"\ntags = [\"rust\", \"entry\"]\nposition = { column = 3, affinity = \"after\" }\n\n[links.presentation]\ncolor = \"ambra\"\ncompact = true\n+++\nCorpo\n";
        let parsed = parse_ok(Path::new("futuro.md"), source);
        let Some(original_link) = parsed.note.header.links.first() else {
            panic!("the source link was not parsed");
        };
        let expected_extra = original_link.extra.clone();

        let serialized = serialize_ok(&parsed.note);
        let reparsed = parse_ok(Path::new("futuro.md"), &serialized);
        let Some(reparsed_link) = reparsed.note.header.links.first() else {
            panic!("the serialized link was not reparsed");
        };

        assert_eq!(reparsed_link.extra, expected_extra);
        assert_eq!(reparsed_link.workspace, original_link.workspace);
        assert_eq!(reparsed_link.path, original_link.path);
        assert_eq!(
            (reparsed_link.from, reparsed_link.to),
            (original_link.from, original_link.to)
        );
    }

    #[test]
    fn markdown_sconosciuto_e_html_restano_testo_sorgente() {
        let source = b":::estensione\n<div data-x=\"1\">testo</div>\n{{sintassi-sconosciuta}}\n";
        let parsed = parse_ok(Path::new("sorgente.md"), source);

        assert_eq!(parsed.note.markdown.as_bytes(), source);
        assert_eq!(parsed.original_bytes, source);
    }

    #[test]
    fn senza_heading_il_titolo_deriva_dal_nome_file() {
        let parsed = parse_ok(Path::new("mia-rotta.md"), b"Solo corpo\n");
        assert_eq!(parsed.note.header.title, "mia-rotta");

        let untitled = parse_ok(Path::new(""), b"Solo corpo\n");
        assert_eq!(untitled.note.header.title, "Senza titolo");
    }
}
