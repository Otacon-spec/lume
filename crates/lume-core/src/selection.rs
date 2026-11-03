//! Selezione: coppia anchor/head in CHAR INDEX (unità nativa di ropey).

use crate::history::ChangeSet;

/// Associatività di una posizione rispetto a un inserimento nello stesso
/// punto: `Before` resta prima del testo inserito, `After` lo segue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Before,
    After,
}

/// Selezione singola: `anchor` è l'estremo fisso, `head` quello mobile.
/// Se coincidono la selezione è un semplice caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub anchor: usize,
    pub head: usize,
}

impl Range {
    pub const fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    /// Caret (selezione vuota) alla posizione data.
    pub const fn point(pos: usize) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }

    /// Estremo minore della selezione.
    pub fn min(&self) -> usize {
        self.anchor.min(self.head)
    }

    /// Estremo maggiore della selezione.
    pub fn max(&self) -> usize {
        self.anchor.max(self.head)
    }

    /// `true` se la selezione è un caret (anchor == head).
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Rimappa la selezione attraverso un edit.
    pub fn map_through(&self, changes: &ChangeSet, assoc: Assoc) -> Range {
        Range {
            anchor: changes.map_pos(self.anchor, assoc),
            head: changes.map_pos(self.head, assoc),
        }
    }

    /// Limita entrambi gli estremi alla lunghezza del documento.
    pub fn clamped(self, len: usize) -> Range {
        Range {
            anchor: self.anchor.min(len),
            head: self.head.min(len),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn estremi_e_caret() {
        let r = Range::new(5, 2);
        assert_eq!(r.min(), 2);
        assert_eq!(r.max(), 5);
        assert!(!r.is_empty());
        assert!(Range::point(3).is_empty());
    }

    #[test]
    fn map_through_insert_prima_della_selezione() {
        // "hello world", selezione su "world" (6..11), insert di "XY" a 0.
        let cs = ChangeSet::insert_at(0, "XY", 11);
        let r = Range::new(6, 11).map_through(&cs, Assoc::After);
        assert_eq!(r, Range::new(8, 13));
    }

    #[test]
    fn map_through_insert_sul_caret() {
        let cs = ChangeSet::insert_at(4, "ab", 10);
        assert_eq!(
            Range::point(4).map_through(&cs, Assoc::Before),
            Range::point(4)
        );
        assert_eq!(
            Range::point(4).map_through(&cs, Assoc::After),
            Range::point(6)
        );
    }

    #[test]
    fn map_through_delete_che_ingloba() {
        // delete 2..6, caret a 4 collassa sull'inizio della cancellazione.
        let cs = ChangeSet::delete_range(2, 6, 10);
        assert_eq!(
            Range::point(4).map_through(&cs, Assoc::After),
            Range::point(2)
        );
        // posizione dopo la cancellazione trasla indietro.
        assert_eq!(
            Range::point(8).map_through(&cs, Assoc::After),
            Range::point(4)
        );
    }

    #[test]
    fn clamped() {
        assert_eq!(Range::new(3, 12).clamped(5), Range::new(3, 5));
    }
}
