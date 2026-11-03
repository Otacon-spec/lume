//! Risultato immutabile di una query, condivisibile senza parser o Rope.

use std::{ops::Range, sync::Arc};

use crate::{LanguageId, TokenClass};

/// Una regione ripiegabile: la riga `start_line` resta visibile, le righe
/// `start_line + 1 ..= end_line` sono nascoste. Semichiuso, non vuoto.
///
/// Invariante: `end_line >= start_line`, e la regione è non vuota solo se
/// `end_line > start_line`. Il produttore scarta le regioni vuote; il tipo
/// non può imporlo senza rompere la costruzione letterale nei test, ma nessun
/// consumatore deve costruire una regione che lo violi.
///
/// È la stessa convenzione del Cestino e del giro GUI: il testo non cambia
/// mai, cambia solo la presentazione.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    pub start_line: usize,
    pub end_line: usize,
}

/// Il chiamante deve verificare anche identità del documento ed epoca radice.
/// La revisione identifica il testo usato dal produttore, non viene inferita
/// dall'albero. Creare questo risultato sul worker della medesima revisione.
#[derive(Clone, Debug)]
pub struct HighlightSnapshot {
    pub(crate) lang: LanguageId,
    pub(crate) revision: u64,
    pub(crate) covered: Range<usize>,
    pub(crate) tokens: Arc<[(Range<usize>, TokenClass)]>,
    /// Le regioni ripiegabili del documento, calcolate sul worker. Vuote se
    /// la lingua non ha una query di folding o se il documento non ha
    /// blocchi multilinea.
    pub(crate) folding: Arc<[FoldRegion]>,
    /// Il produttore ha rifiutato il risultato (troppi blocchi, memoria):
    /// i blocchi restano aperti e il chiamante deve avvisare.
    pub(crate) folding_rejected: bool,
}

impl HighlightSnapshot {
    pub fn folding(&self) -> &[FoldRegion] {
        &self.folding
    }

    pub fn folding_rejected(&self) -> bool {
        self.folding_rejected
    }
}

impl HighlightSnapshot {
    pub fn language(&self) -> LanguageId {
        self.lang
    }

    /// Revisione del testo da cui proviene la proiezione.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Identità della proiezione per invalidare cache UI senza copiare token.
    pub fn same_projection(&self, other: &Self) -> bool {
        self.lang == other.lang
            && self.revision == other.revision
            && self.covered == other.covered
            && Arc::ptr_eq(&self.tokens, &other.tokens)
            && Arc::ptr_eq(&self.folding, &other.folding)
            && self.folding_rejected == other.folding_rejected
    }

    pub fn tokens(&self) -> &[(Range<usize>, TokenClass)] {
        &self.tokens
    }

    /// Nessuna query, copia o lettura del testo nel percorso di rendering.
    pub fn covers(&self, lang: LanguageId, revision: u64, requested: Range<usize>) -> bool {
        self.lang == lang
            && self.revision == revision
            && requested.start <= requested.end
            && self.covered.start <= requested.start
            && requested.end <= self.covered.end
    }
}
