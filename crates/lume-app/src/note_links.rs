//! Logica per collegare righe di codice agli Appunti.

use lume_core::Range;
use lume_notes::{CodeLink, LoadedNote, Note, NoteStore};
use ropey::Rope;

/// Numero di righe visibili, senza contare la riga vuota dopo l'ultimo LF.
#[must_use]
pub fn righe_del_testo(rope: &Rope) -> usize {
    let trailing_newline = rope
        .len_chars()
        .checked_sub(1)
        .is_some_and(|last| rope.char(last) == '\n');
    if trailing_newline {
        rope.len_lines().saturating_sub(1).max(1)
    } else {
        rope.len_lines()
    }
}

/// Righe (a base 1, estremi inclusi) toccate dalla selezione.
#[must_use]
pub fn righe_della_selezione(rope: &Rope, selection: Range) -> (usize, usize) {
    let len = rope.len_chars();
    let last_visible = righe_del_testo(rope).saturating_sub(1);
    let start = selection.min().min(len);
    let end = selection.max().min(len);
    let first = rope.char_to_line(start).min(last_visible);
    let mut last = rope.char_to_line(end).min(last_visible);
    if end > start && last > first && rope.line_to_char(last) == end {
        last = last.saturating_sub(1);
    }
    (first + 1, last + 1)
}

/// Intervallo di caratteri `[inizio, fine)` che copre le righe a base 1.
#[must_use]
pub fn caratteri_delle_righe(rope: &Rope, da: usize, a: usize) -> (usize, usize) {
    let righe_visibili = righe_del_testo(rope);
    if rope.len_chars() == 0 {
        return (0, 0);
    }
    let (prima, ultima) = lume_notes::clamp_lines(da, a, righe_visibili);
    let righe = rope.len_lines();
    let inizio = rope.line_to_char(prima.min(righe.saturating_sub(1)));
    let fine = if ultima + 1 < righe {
        rope.line_to_char(ultima + 1)
    } else {
        rope.len_chars()
    };
    (inizio, fine)
}

/// Rimuove il link indicato, restituendo un messaggio adatto alla barra.
pub fn rimuovi_collegamento(nota: &mut Note, indice: usize) -> Result<String, String> {
    nota.remove_link(indice)
        .map(|_| "Collegamento rimosso".to_owned())
        .ok_or_else(|| "Il collegamento non esiste più".to_owned())
}

/// Aggiunge un link, rifiutando i duplicati.
pub fn aggiungi_collegamento(note: &mut Note, link: CodeLink) -> Result<String, String> {
    if note.add_link(link) {
        let title = if note.header.title.is_empty() {
            "Senza titolo"
        } else {
            note.header.title.as_str()
        };
        Ok(format!("Collegato a «{title}»"))
    } else {
        Err("Questo collegamento c'è già nella nota".to_owned())
    }
}

/// Crea un appunto già collegato e lo scrive.
///
/// Restituisce anche l'avviso della cartella non sincronizzata: la nota è
/// già sul disco, l'avviso va in status bar.
pub fn create_linked(
    store: &NoteStore,
    link: CodeLink,
) -> Result<(LoadedNote, Option<String>), String> {
    let (mut loaded, created_sync) = store
        .create_reporting("Senza titolo")
        .map_err(|error| error.to_string())?;
    loaded.note.add_link(link);
    let (saved, saved_sync) = store
        .save_reporting(&loaded)
        .map_err(|error| error.to_string())?;
    let avviso = created_sync
        .or(saved_sync)
        .map(|error| avviso_creazione(&saved.note.header.title, &error));
    Ok((saved, avviso))
}

/// Il testo dell'avviso per una nota creata con la cartella non
/// sincronizzata.
pub fn avviso_creazione(title: &str, error: &impl std::fmt::Display) -> String {
    format!("Appunto «{title}» creato, ma la cartella non è stata sincronizzata sul disco: {error}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use lume_core::Range;
    use lume_notes::{NoteId, NoteStore};

    const TEXT: &str = "uno\ndue\ntre\n";

    #[test]
    fn line_count_does_not_include_the_trailing_empty_line() {
        assert_eq!(righe_del_testo(&Rope::from_str(TEXT)), 3);
        assert_eq!(righe_del_testo(&Rope::from_str("a\nb")), 2);
        assert_eq!(righe_del_testo(&Rope::from_str("")), 1);
        assert_eq!(righe_del_testo(&Rope::from_str("\n")), 1);
    }

    #[test]
    fn selection_lines_are_one_based_and_inclusive() {
        let rope = Rope::from_str(TEXT);
        assert_eq!(righe_della_selezione(&rope, Range::point(5)), (2, 2));
        assert_eq!(righe_della_selezione(&rope, Range::new(4, 7)), (2, 2));
        assert_eq!(righe_della_selezione(&rope, Range::new(4, 8)), (2, 2));
        assert_eq!(righe_della_selezione(&rope, Range::new(1, 10)), (1, 3));
        assert_eq!(righe_della_selezione(&rope, Range::new(10, 2)), (1, 3));
        assert_eq!(righe_della_selezione(&rope, Range::new(0, 12)), (1, 3));
        assert_eq!(righe_della_selezione(&rope, Range::new(0, 999)), (1, 3));
        assert_eq!(
            righe_della_selezione(&Rope::from_str(""), Range::point(0)),
            (1, 1)
        );
    }

    #[test]
    fn line_ranges_map_to_character_ranges_and_clamp_to_the_file() {
        let rope = Rope::from_str(TEXT);
        assert_eq!(caratteri_delle_righe(&rope, 2, 2), (4, 8));
        assert_eq!(caratteri_delle_righe(&rope, 2, 99), (4, 12));
        assert_eq!(caratteri_delle_righe(&Rope::new(), 1, 2), (0, 0));
    }

    fn code_link(from: usize, to: usize) -> CodeLink {
        CodeLink::try_new("/w/proj", "src/main.rs", from, to, None).unwrap()
    }

    #[test]
    fn add_link_reports_a_duplicate_without_adding_it() {
        let mut note = Note::empty(NoteId::from("n"));
        note.header.title = "Idea".into();
        assert_eq!(
            aggiungi_collegamento(&mut note, code_link(2, 3)).unwrap(),
            "Collegato a «Idea»"
        );
        assert_eq!(note.header.links.len(), 1);
        assert_eq!(
            aggiungi_collegamento(&mut note, code_link(2, 3)).unwrap_err(),
            "Questo collegamento c'è già nella nota"
        );
        assert_eq!(note.header.links.len(), 1);
        aggiungi_collegamento(&mut note, code_link(4, 4)).unwrap();
        assert_eq!(note.header.links.len(), 2);
    }

    #[test]
    fn remove_link_reports_stale_indices() {
        let mut note = Note::empty(NoteId::from("n"));
        note.add_link(code_link(2, 3));
        assert_eq!(
            rimuovi_collegamento(&mut note, 0).unwrap(),
            "Collegamento rimosso"
        );
        assert!(note.header.links.is_empty());
        assert!(
            rimuovi_collegamento(&mut note, 0)
                .unwrap_err()
                .contains("non esiste")
        );
    }

    #[test]
    fn new_note_is_published_with_its_link() {
        let dir = tempfile::tempdir().unwrap();
        let store = NoteStore::open(dir.path()).unwrap();
        let (loaded, avviso) = create_linked(&store, code_link(2, 3)).unwrap();
        assert!(avviso.is_none(), "{avviso:?}");
        assert_eq!(loaded.note.header.links.len(), 1);
        assert_eq!(
            store.load(loaded.path()).unwrap().note.header.links,
            loaded.note.header.links
        );
        assert_eq!(store.scan().unwrap().len(), 1);
    }
}
