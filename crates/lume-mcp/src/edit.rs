//! Motori puri per le modifiche richieste via MCP.

pub const MAX_WRITE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReplaceError {
    #[error("old_text non può essere vuoto")]
    EmptyOld,
    #[error("old_text non è presente nel documento")]
    NotFound,
    #[error("old_text compare {0} volte; specifica un contesto univoco o replace_all")]
    Ambiguous(usize),
}

/// Sostituisce occorrenze testuali non sovrapposte. Le coppie CRLF sono
/// equivalenti a LF per la ricerca; il testo nuovo riprende lo stile locale
/// delle fine-riga, senza riscrivere le parti non modificate.
pub fn apply_replace(
    text: &str,
    old: &str,
    new: &str,
    all: bool,
) -> Result<(String, usize), ReplaceError> {
    if old.is_empty() {
        return Err(ReplaceError::EmptyOld);
    }
    let old_normalized = old.replace("\r\n", "\n");
    let new_normalized = new.replace("\r\n", "\n");
    let (normalized_text, offsets) = normalize_with_offsets(text);
    let matches: Vec<_> = normalized_text.match_indices(&old_normalized).collect();
    if matches.is_empty() {
        return Err(ReplaceError::NotFound);
    }
    if !all && matches.len() != 1 {
        return Err(ReplaceError::Ambiguous(matches.len()));
    }

    let preferred_eol = preferred_eol(text);
    let spans: Vec<(usize, usize)> = matches
        .iter()
        .map(|(start, matched)| {
            let end = start + matched.len();
            Some((*offsets.get(*start)?, *offsets.get(end)?))
        })
        .collect::<Option<_>>()
        .ok_or(ReplaceError::NotFound)?;
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in &spans {
        let Some(original) = text.get(cursor..*start) else {
            return Err(ReplaceError::NotFound);
        };
        output.push_str(original);
        let Some(matched) = text.get(*start..*end) else {
            return Err(ReplaceError::NotFound);
        };
        let eol = if matched.contains("\r\n") {
            "\r\n"
        } else if matched.contains('\n') {
            "\n"
        } else {
            preferred_eol
        };
        output.push_str(&new_normalized.replace('\n', eol));
        cursor = *end;
    }
    let Some(tail) = text.get(cursor..) else {
        return Err(ReplaceError::NotFound);
    };
    output.push_str(tail);
    Ok((output, spans.len()))
}

pub fn check_size(content: &str) -> Result<(), String> {
    if content.len() > MAX_WRITE_BYTES {
        Err("il contenuto supera il limite di 1 MiB".to_owned())
    } else {
        Ok(())
    }
}

fn preferred_eol(text: &str) -> &'static str {
    let crlf = text.match_indices("\r\n").count();
    let lf = text.matches('\n').count().saturating_sub(crlf);
    if crlf > lf { "\r\n" } else { "\n" }
}

fn normalize_with_offsets(text: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(text.len());
    let mut offsets = vec![0];
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(rest) = text.get(cursor..) else {
            break;
        };
        if rest.starts_with("\r\n") {
            normalized.push('\n');
            cursor += 2;
            offsets.push(cursor);
            continue;
        }
        let Some(character) = rest.chars().next() else {
            break;
        };
        normalized.push(character);
        let bytes = character.len_utf8();
        for offset in 1..=bytes {
            offsets.push(cursor + offset);
        }
        cursor += bytes;
    }
    (normalized, offsets)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn sostituisce_una_sola_occorrenza() {
        let (new, count) =
            apply_replace("fn a() {}\nfn b() {}\n", "fn b()", "fn c()", false).unwrap();
        assert_eq!(new, "fn a() {}\nfn c() {}\n");
        assert_eq!(count, 1);
    }

    #[test]
    fn piu_occorrenze_senza_replace_all_sono_ambigue() {
        let error = apply_replace("x x x", "x", "y", false).unwrap_err();
        assert_eq!(error, ReplaceError::Ambiguous(3));
        assert!(error.to_string().contains('3'));
    }

    #[test]
    fn replace_all_sostituisce_tutte_le_occorrenze_non_sovrapposte() {
        assert_eq!(
            apply_replace("x x x", "x", "y", true).unwrap(),
            ("y y y".into(), 3)
        );
        assert_eq!(
            apply_replace("aaaa", "aa", "b", true).unwrap(),
            ("bb".into(), 2)
        );
    }

    #[test]
    fn testo_assente_e_old_vuoto_sono_errori() {
        assert_eq!(
            apply_replace("abc", "z", "y", false),
            Err(ReplaceError::NotFound)
        );
        assert_eq!(
            apply_replace("abc", "", "y", false),
            Err(ReplaceError::EmptyOld)
        );
        assert_eq!(
            apply_replace("abc", "", "y", true),
            Err(ReplaceError::EmptyOld)
        );
    }

    #[test]
    fn old_lf_corrisponde_a_un_file_crlf_e_new_ne_riprende_le_fine_riga() {
        let (new, count) =
            apply_replace("uno\r\ndue\r\ntre\r\n", "uno\ndue", "1\n2", false).unwrap();
        assert_eq!(new, "1\r\n2\r\ntre\r\n");
        assert_eq!(count, 1);
    }

    #[test]
    fn tetto_di_scrittura_un_mebibyte() {
        assert!(check_size(&"a".repeat(MAX_WRITE_BYTES)).is_ok());
        assert!(
            check_size(&"a".repeat(MAX_WRITE_BYTES + 1))
                .unwrap_err()
                .contains("1 MiB")
        );
    }
}
