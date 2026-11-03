//! Il collante fra la sessione di completamento (pura, `completion.rs`) e
//! l'editor: che cosa fa un comando al popup aperto, la riga di testo che
//! decide se aprirlo da solo, e la fotografia che si consegna al widget.

use lume_core::DocId;
use lume_ui::editor::{EditorCommand, EditorPopup, Motion, PopupRow};
use ropey::Rope;

use crate::completion::{CompletionSession, is_word_char};

/// Che cosa un comando fa a un popup già aperto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    /// Non lo tocca.
    Keep,
    /// Ha scritto o mosso il cursore dentro la parola: si rifà il filtro.
    Refilter,
    /// Qualunque altra cosa: si chiude.
    Close,
}

/// Classifica un comando **prima** che l'app lo applichi. Il default è
/// `Close`: una variante nuova di `EditorCommand` chiude il popup, che è il
/// lato sicuro dell'errore.
pub fn follow_of(comando: &EditorCommand) -> Follow {
    match comando {
        EditorCommand::Insert(testo) if !testo.is_empty() && testo.chars().all(is_word_char) => {
            Follow::Refilter
        }
        EditorCommand::Backspace | EditorCommand::Move(Motion::Left | Motion::Right, false) => {
            Follow::Refilter
        }
        EditorCommand::Save
        | EditorCommand::Copy
        | EditorCommand::Delete
        | EditorCommand::PopupUp
        | EditorCommand::PopupDown
        | EditorCommand::PopupAccept
        | EditorCommand::PopupDismiss
        | EditorCommand::TriggerComplete
        | EditorCommand::HoverAt(_) => Follow::Keep,
        _ => Follow::Close,
    }
}

/// Il testo della riga del cursore **fino al cursore**: è quel che
/// `should_open` guarda per decidere se il carattere appena scritto apre il
/// completamento. Un cursore fuori scala si stringe alla fine.
pub fn line_before(rope: &Rope, head: usize) -> String {
    let head = head.min(rope.len_chars());
    let riga = rope.char_to_line(head);
    rope.slice(rope.line_to_char(riga)..head).to_string()
}

/// La fotografia da consegnare al widget: il widget non tiene riferimenti
/// alla sessione, e `EditorOverlay` è `Copy` con riferimenti.
pub fn popup_of(sessione: &CompletionSession) -> EditorPopup {
    EditorPopup {
        anchor: sessione.anchor(),
        rows: sessione
            .rows()
            .into_iter()
            .map(|(label, detail)| PopupRow {
                label: label.to_owned(),
                detail: detail.map(str::to_owned),
            })
            .collect(),
        selected: sessione.selected(),
    }
}

/// La sessione e la sua fotografia, tenute allineate.
pub struct CompletionUi {
    pub session: CompletionSession,
    pub popup: EditorPopup,
}

impl CompletionUi {
    pub fn new(session: CompletionSession) -> Self {
        let popup = popup_of(&session);
        Self { session, popup }
    }

    /// Da chiamare dopo ogni cambio della sessione (filtro, selezione).
    pub fn sync(&mut self) {
        self.popup = popup_of(&self.session);
    }
}

/// La domanda di completamento in volo. Ne esiste una sola: chiederne
/// un'altra sostituisce questa, e ogni chiusura la azzera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionWait {
    /// Il server a cui si è chiesto: gli id di richiesta sono contatori per
    /// workspace e da soli collidono.
    pub server: String,
    pub request: u64,
    pub doc: DocId,
    /// Dove cominciava la parola quando si è chiesto.
    pub anchor: usize,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lsp_types::{CompletionItem, CompletionResponse};
    use lume_core::DocId;
    use lume_ui::editor::{EditorCommand, Motion, PageDirection};
    use ropey::Rope;

    use super::{Follow, follow_of, line_before, popup_of};
    use crate::completion::CompletionSession;

    fn insert(testo: &str) -> EditorCommand {
        EditorCommand::Insert(testo.to_owned())
    }

    #[test]
    fn scrivere_o_muoversi_dentro_la_parola_rifiltra() {
        for comando in [
            insert("a"),
            insert("_"),
            insert("é"),
            EditorCommand::Backspace,
            EditorCommand::Move(Motion::Left, false),
            EditorCommand::Move(Motion::Right, false),
        ] {
            assert_eq!(follow_of(&comando), Follow::Refilter, "{comando:?}");
        }
    }

    #[test]
    fn tutto_il_resto_chiude_il_popup() {
        for comando in [
            insert(" "),
            insert("."),
            insert("("),
            insert(""),
            insert("ab "),
            EditorCommand::Paste("x".to_owned()),
            EditorCommand::Cut,
            EditorCommand::Enter,
            EditorCommand::Tab,
            EditorCommand::Undo,
            EditorCommand::Redo,
            EditorCommand::SelectAll,
            EditorCommand::SetCursor {
                pos: 0,
                extend: false,
            },
            EditorCommand::Move(Motion::Up, false),
            EditorCommand::Move(Motion::Home, false),
            EditorCommand::Move(Motion::Left, true),
            EditorCommand::MovePage {
                direction: PageDirection::Down,
                lines: 10,
                extend: false,
            },
            EditorCommand::ContextMenu {
                pos: 0,
                screen_x: 0,
                screen_y: 0,
            },
        ] {
            assert_eq!(follow_of(&comando), Follow::Close, "{comando:?}");
        }
    }

    #[test]
    fn i_comandi_che_non_toccano_il_testo_lasciano_stare() {
        for comando in [
            EditorCommand::Save,
            EditorCommand::Copy,
            EditorCommand::Delete,
            EditorCommand::PopupUp,
            EditorCommand::PopupDown,
            EditorCommand::PopupAccept,
            EditorCommand::PopupDismiss,
            EditorCommand::TriggerComplete,
            EditorCommand::HoverAt(None),
            EditorCommand::HoverAt(Some(3)),
        ] {
            assert_eq!(follow_of(&comando), Follow::Keep, "{comando:?}");
        }
    }

    #[test]
    fn il_testo_della_riga_fino_al_cursore() {
        let rope = Rope::from_str("ab\ncd");
        assert_eq!(line_before(&rope, 4), "c");
        assert_eq!(line_before(&rope, 5), "cd");
        assert_eq!(line_before(&rope, 3), "", "appena dopo l'a capo");
        assert_eq!(line_before(&rope, 0), "");
        assert_eq!(line_before(&rope, 2), "ab");
        assert_eq!(
            line_before(&rope, 99),
            "cd",
            "un cursore fuori scala si stringe"
        );
        assert_eq!(line_before(&Rope::new(), 0), "");
    }

    #[test]
    fn il_popup_porta_ancora_righe_e_selezione_della_sessione() {
        let rope = Rope::from_str("x p");
        let voce = |label: &str, detail: Option<&str>| CompletionItem {
            label: label.to_owned(),
            detail: detail.map(str::to_owned),
            ..CompletionItem::default()
        };
        let mut sessione = CompletionSession::new(
            DocId(1),
            1,
            &rope,
            3,
            CompletionResponse::Array(vec![voce("push", Some("fn(T)")), voce("pop", None)]),
        )
        .unwrap();
        let popup = popup_of(&sessione);
        assert_eq!(popup.anchor, 2);
        assert_eq!(popup.selected, 0);
        let etichette: Vec<&str> = popup.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(etichette, ["pop", "push"], "in ordine di lista");
        assert_eq!(popup.rows[0].detail, None);
        assert_eq!(popup.rows[1].detail.as_deref(), Some("fn(T)"));

        sessione.select_next();
        assert_eq!(popup_of(&sessione).selected, 1);
    }
}
