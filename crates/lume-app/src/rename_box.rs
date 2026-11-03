//! Lo stato di «Rinomina simbolo». Il riquadro che lo mostra è quello
//! condiviso di [`crate::prompt`]: due comandi che chiedono la stessa cosa
//! nello stesso momento del gesto non hanno ragione di avere due aspetti.
//!
//! Rinomina **in linea** (input sul testo al caret) resta fuori finché il
//! widget editor non espone la geometria del caret a `view()` senza facciata.

use lsp_types::Position;
use lume_core::DocId;

/// I messaggi del riquadro sono quelli del guscio: qui non ce ne sono di
/// propri, e inventarne un doppione sarebbe un secondo posto da mappare.
pub use crate::prompt::PromptMessage as RenameMessage;

/// Cosa si sta rinominando, e su quale testo.
///
/// `version` è la fotografia della revisione al momento della richiesta: se
/// al ritorno non torna, la risposta parla di un testo che non c'è più.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameBox {
    pub doc: DocId,
    pub position: Position,
    pub version: u64,
    pub original: String,
    pub name: String,
    /// Una richiesta è già partita e si aspetta: il tasto non si ripreme.
    pub in_corso: bool,
}

impl RenameBox {
    pub fn new(doc: DocId, position: Position, version: u64, original: String) -> Self {
        Self {
            doc,
            position,
            version,
            name: original.clone(),
            original,
            in_corso: false,
        }
    }

    /// Un nome nuovo che è uguale al vecchio, o vuoto, non è una rinomina.
    pub fn is_applicable(&self) -> bool {
        !self.in_corso && !self.name.trim().is_empty() && self.name != self.original
    }

    pub fn prompt(&self) -> crate::prompt::Prompt<'_> {
        crate::prompt::Prompt {
            title: format!("Rinomina «{}»", self.original),
            placeholder: "nuovo nome",
            value: &self.name,
            hint: if self.in_corso {
                "Il server sta preparando le modifiche…".to_owned()
            } else {
                "Nessun file viene scritto: le schede restano da salvare.".to_owned()
            },
            ready: self.is_applicable(),
            confirm_label: "Rinomina",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::RenameBox;
    use lsp_types::Position;
    use lume_core::DocId;

    /// Il tasto non si accende per niente: un nome vuoto, o uguale a quello
    /// di prima, farebbe partire un giro sul server per non cambiare nulla —
    /// e su un simbolo molto usato quel giro costa secondi.
    #[test]
    fn un_nome_uguale_o_vuoto_non_e_una_rinomina() {
        let mut riquadro = RenameBox::new(DocId(0), Position::new(0, 0), 3, "mian".to_owned());
        assert!(!riquadro.is_applicable(), "parte uguale a se stesso");
        riquadro.name = "   ".to_owned();
        assert!(!riquadro.is_applicable());
        riquadro.name = "main".to_owned();
        assert!(riquadro.is_applicable());
        riquadro.in_corso = true;
        assert!(
            !riquadro.is_applicable(),
            "e non si ripreme mentre il server lavora"
        );
    }
}
