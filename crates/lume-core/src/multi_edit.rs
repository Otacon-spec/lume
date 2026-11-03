//! Edit atomici su tutte le selezioni di un documento.
use crate::{AppliedEdit, Document};

#[derive(Debug, Clone, Copy)]
pub enum MultiEdit<'a> {
    Replace(&'a str),
    Enter,
    Tab,
    Backspace,
    Delete,
    Cut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MultiEditError {
    #[error("le modifiche non sono applicabili alle selezioni correnti")]
    InvalidChangeSet,
}

pub fn apply(
    doc: &mut Document,
    edit: MultiEdit<'_>,
) -> Result<Option<AppliedEdit>, MultiEditError> {
    use crate::{Assoc, ChangeSet, Range, SelectionSet, Transaction, movement};
    use std::borrow::Cow;
    if matches!(edit, MultiEdit::Replace("")) {
        return Ok(None);
    }
    let actions: Vec<_> = doc
        .selections()
        .ranges()
        .iter()
        .map(|range| {
            let from = range.min();
            let to = range.max();
            match edit {
                MultiEdit::Replace(text) => (from, to, Cow::Borrowed(text)),
                MultiEdit::Tab => (from, to, Cow::Borrowed("    ")),
                MultiEdit::Enter => (
                    from,
                    to,
                    Cow::Owned(movement::auto_indent_for_newline(doc.rope(), from)),
                ),
                MultiEdit::Backspace if range.is_empty() => (
                    movement::prev_grapheme(doc.rope(), from),
                    to,
                    Cow::Borrowed(""),
                ),
                MultiEdit::Delete if range.is_empty() => (
                    from,
                    movement::next_grapheme(doc.rope(), to),
                    Cow::Borrowed(""),
                ),
                MultiEdit::Backspace | MultiEdit::Delete | MultiEdit::Cut => {
                    (from, to, Cow::Borrowed(""))
                }
            }
        })
        .collect();
    // L'espansione a grapheme può invertire l'ordine degli inizi.
    let mut pending: Vec<_> = actions
        .iter()
        .filter(|(from, to, text)| !doc.rope().slice(*from..*to).chars().eq(text.chars()))
        .map(|(from, to, text)| (*from, *to, text.to_string()))
        .collect();
    pending.sort_by_key(|(from, to, _)| (*from, *to));
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (from, to, text) in pending {
        if text.is_empty()
            && let Some((_, previous_to, previous_text)) = edits.last_mut()
            && previous_text.is_empty()
            && *previous_to >= from
        {
            *previous_to = (*previous_to).max(to);
            continue;
        }
        edits.push((from, to, text));
    }
    let changes = ChangeSet::replace_many(&edits, doc.rope().len_chars())
        .ok_or(MultiEditError::InvalidChangeSet)?;
    let after: Vec<_> = actions
        .iter()
        .map(|(from, to, text)| {
            let mapped = changes.map_pos(*from, Assoc::After);
            let end = if from == to {
                mapped
            } else {
                mapped + text.chars().count()
            };
            Range::point(end)
        })
        .collect();
    let after = SelectionSet::try_new(after, doc.selections().primary_index(), changes.len_after())
        .map_err(|_| MultiEditError::InvalidChangeSet)?;
    if changes.change_span().is_none() {
        if doc.selections() != &after {
            doc.set_selections(after);
        }
        return Ok(None);
    }
    Ok(Some(
        doc.apply_with_selections(Transaction::new(changes), after),
    ))
}

/// Clipboard comune a menu e widget; i caret vuoti non aggiungono righe.
pub fn selected_text(doc: &Document) -> Option<String> {
    let mut text = String::new();
    let mut found = false;
    for range in doc.selections().ranges().iter().filter(|r| !r.is_empty()) {
        if found {
            text.push('\n');
        }
        found = true;
        for chunk in doc.rope().slice(range.min()..range.max()).chunks() {
            text.push_str(chunk);
        }
    }
    found.then_some(text)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{DocId, Range, SelectionSet};

    fn documento(text: &str, ranges: Vec<Range>, primary: usize) -> Document {
        let mut doc = Document::from_str(text, None, DocId(1));
        doc.set_selections(SelectionSet::try_new(ranges, primary, doc.rope().len_chars()).unwrap());
        doc
    }

    #[test]
    fn multi_budget_un_mib_1024_caret_dieci_edit_e_undo() {
        let text = "x".repeat(1024 * 1024);
        let mut doc = documento(
            &text,
            (0..1024).map(|index| Range::point(index * 1024)).collect(),
            1023,
        );
        let before = doc.selections().clone();
        let mut edits = Vec::new();
        let mut undos = Vec::new();
        for _ in 0..10 {
            let start = std::time::Instant::now();
            apply(&mut doc, MultiEdit::Replace("a")).unwrap().unwrap();
            edits.push(start.elapsed());
            assert!(doc.history_bytes() <= crate::document::DOCUMENT_HISTORY_BUDGET);
        }
        let retained = doc.history_bytes();
        for _ in 0..10 {
            let start = std::time::Instant::now();
            doc.undo().unwrap();
            undos.push(start.elapsed());
        }
        assert_eq!(doc.rope().to_string(), text);
        assert_eq!(doc.selections(), &before);
        assert_eq!(SelectionSet::single(Range::point(0)).owned_bytes(), 0);
        edits.sort();
        undos.sort();
        eprintln!(
            "MULTICURSORE_BUDGET:modo=debug,test_core,byte=1048576,caret=1024,edit_mediana_us={},edit_max_us={},undo_mediana_us={},undo_max_us={},history_bytes={retained}",
            edits.get(5).unwrap().as_micros(),
            edits.get(9).unwrap().as_micros(),
            undos.get(5).unwrap().as_micros(),
            undos.get(9).unwrap().as_micros()
        );
    }

    #[test]
    fn multi_edit_atomico_unicode_noop_clipboard() {
        let mut doc = documento("a😀b e\u{301}c", vec![Range::point(2), Range::point(6)], 1);
        let before = doc.selections().clone();
        assert!(apply(&mut doc, MultiEdit::Backspace).unwrap().is_some());
        assert_eq!(doc.rope().to_string(), "ab c");
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(1), Range::point(3)]
        );
        assert_eq!(doc.version(), 1);
        doc.undo().unwrap();
        assert_eq!(doc.rope().to_string(), "a😀b e\u{301}c");
        assert_eq!(doc.selections(), &before);
        let mut doc = documento(
            "abc def ghi",
            vec![Range::new(0, 3), Range::point(4), Range::new(8, 11)],
            0,
        );
        assert_eq!(selected_text(&doc).as_deref(), Some("abc\nghi"));
        let before = doc.selections().clone();
        assert!(apply(&mut doc, MultiEdit::Replace("")).unwrap().is_none());
        assert_eq!(doc.version(), 0);
        assert_eq!(doc.selections(), &before);
        assert!(!doc.is_dirty());
        apply(&mut doc, MultiEdit::Cut).unwrap().unwrap();
        assert_eq!(doc.rope().to_string(), " def ");
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(0), Range::point(1), Range::point(5)]
        );
        assert_eq!(selected_text(&doc), None);
    }

    #[test]
    fn multi_cancellazioni_sovrapposte() {
        let mut doc = documento("abcd", vec![Range::point(2), Range::new(1, 3)], 0);
        apply(&mut doc, MultiEdit::Backspace).unwrap().unwrap();
        assert_eq!(doc.rope().to_string(), "ad");
        let mut doc = documento("e\u{301}z", vec![Range::point(1), Range::point(2)], 1);
        apply(&mut doc, MultiEdit::Backspace).unwrap().unwrap();
        assert_eq!(doc.rope().to_string(), "z");
        assert_eq!(doc.selections().ranges(), &[Range::point(0)]);
        doc.undo().unwrap();
        assert_eq!(doc.rope().to_string(), "e\u{301}z");
        assert_eq!(doc.selections().primary_index(), 1);
    }

    #[test]
    fn multi_cancellazioni_risalgono_al_grapheme_prima_di_un_range_precedente() {
        let mut doc = documento(
            "e\u{301}\u{301}\u{301}\u{301}z",
            vec![Range::point(1), Range::new(2, 3), Range::point(5)],
            2,
        );
        apply(&mut doc, MultiEdit::Backspace).unwrap().unwrap();
        assert_eq!(doc.rope().to_string(), "z");
        assert_eq!(doc.selections().ranges(), &[Range::point(0)]);
    }

    #[test]
    fn multi_indentazione_e_sostituzioni_adiacenti() {
        let mut doc = documento("  a\n\tb", vec![Range::point(3), Range::point(6)], 0);
        apply(&mut doc, MultiEdit::Enter).unwrap().unwrap();
        assert_eq!(doc.rope().to_string(), "  a\n  \n\tb\n\t");
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(6), Range::point(11)]
        );
        doc.undo().unwrap();
        assert_eq!(doc.rope().to_string(), "  a\n\tb");
        let mut doc = documento("abcdef", vec![Range::new(0, 3), Range::new(3, 6)], 1);
        apply(&mut doc, MultiEdit::Replace("é😀")).unwrap().unwrap();
        assert_eq!(doc.rope().to_string(), "é😀é😀");
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(2), Range::point(4)]
        );
        assert_eq!(doc.selection(), Range::point(4));
        doc.undo().unwrap();
        assert_eq!(doc.rope().to_string(), "abcdef");
    }

    #[test]
    fn multi_bordi_tab_e_sostituzione_identica_non_sporcano_inutilmente() {
        let mut doc = documento("abc", vec![Range::point(0), Range::point(3)], 0);
        apply(&mut doc, MultiEdit::Tab).unwrap().unwrap();
        assert_eq!(doc.rope().to_string(), "    abc    ");
        doc.undo().unwrap();
        doc.set_selection(Range::point(3));
        let version = doc.version();
        assert!(apply(&mut doc, MultiEdit::Delete).unwrap().is_none());
        assert_eq!(doc.version(), version);
        doc.set_selection(Range::new(0, 3));
        assert!(
            apply(&mut doc, MultiEdit::Replace("abc"))
                .unwrap()
                .is_none()
        );
        assert_eq!(doc.selection(), Range::point(3));
        assert_eq!(doc.version(), version);
        assert!(!doc.is_dirty());
    }
}
