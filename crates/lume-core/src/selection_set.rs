//! Insieme di selezioni del documento, con un principale riconoscibile.
use crate::{Assoc, ChangeSet, Range};

pub const MAX_SELECTIONS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SelectionError {
    #[error("serve almeno una selezione")]
    Empty,
    #[error("selezione principale non valida")]
    InvalidPrimary,
    #[error("limite di 1024 selezioni raggiunto")]
    Limit,
}

/// Caso singolo inline: i documenti senza multicursore non allocano range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionSet(Storage);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Storage {
    Single(Range),
    Multiple { ranges: Vec<Range>, primary: usize },
}

impl SelectionSet {
    pub fn single(range: Range) -> Self {
        Self(Storage::Single(range))
    }

    pub fn try_new(ranges: Vec<Range>, primary: usize, len: usize) -> Result<Self, SelectionError> {
        if ranges.is_empty() {
            return Err(SelectionError::Empty);
        }
        let main = ranges
            .get(primary)
            .ok_or(SelectionError::InvalidPrimary)?
            .clamped(len);
        let result = Self::normalized(ranges.into_iter().map(|r| r.clamped(len)).collect(), main);
        if result.ranges().len() > MAX_SELECTIONS {
            return Err(SelectionError::Limit);
        }
        Ok(result)
    }

    fn normalized(mut ranges: Vec<Range>, main: Range) -> Self {
        if let [range] = ranges.as_slice() {
            return Self::single(*range);
        }
        ranges.sort_by_key(|range| (range.min(), range.max()));
        let mut merged: Vec<(Range, bool)> = Vec::with_capacity(ranges.len());
        for range in ranges {
            let is_main = range == main;
            if let Some((previous, was_main)) = merged.last_mut() {
                let touches =
                    previous.max() == range.min() && (previous.is_empty() || range.is_empty());
                if previous.max() > range.min() || touches {
                    let reverse = if is_main {
                        range.anchor > range.head
                    } else {
                        previous.anchor > previous.head
                    };
                    let from = previous.min().min(range.min());
                    let to = previous.max().max(range.max());
                    *previous = if reverse {
                        Range::new(to, from)
                    } else {
                        Range::new(from, to)
                    };
                    *was_main |= is_main;
                    continue;
                }
            }
            merged.push((range, is_main));
        }
        match merged.as_slice() {
            [] => Self::single(main), // i chiamanti interni conservano almeno un range
            [(range, _)] => Self::single(*range),
            _ => {
                let primary = merged.iter().position(|(_, main)| *main).unwrap_or(0);
                Self(Storage::Multiple {
                    ranges: merged.into_iter().map(|(r, _)| r).collect(),
                    primary,
                })
            }
        }
    }

    pub fn ranges(&self) -> &[Range] {
        match &self.0 {
            Storage::Single(range) => std::slice::from_ref(range),
            Storage::Multiple { ranges, .. } => ranges,
        }
    }
    pub fn primary(&self) -> Range {
        match &self.0 {
            Storage::Single(range) => *range,
            Storage::Multiple { ranges, primary } => {
                ranges.get(*primary).copied().unwrap_or_default()
            }
        }
    }
    pub fn primary_index(&self) -> usize {
        match &self.0 {
            Storage::Single(_) => 0,
            Storage::Multiple { primary, .. } => *primary,
        }
    }
    pub fn map_through(&self, changes: &ChangeSet, assoc: Assoc) -> Self {
        self.map_with_primary(changes, assoc, None)
    }
    /// Sostituisce il principale prima di fondere collisioni: una secondaria
    /// non deve sparire solo perché il vecchio principale le passa sopra.
    pub(crate) fn map_with_primary(
        &self,
        changes: &ChangeSet,
        assoc: Assoc,
        after: Option<Range>,
    ) -> Self {
        let main = after
            .unwrap_or_else(|| self.primary().map_through(changes, assoc))
            .clamped(changes.len_after());
        match &self.0 {
            Storage::Single(_) => Self::single(main),
            Storage::Multiple { ranges, primary } => {
                let mapped = ranges
                    .iter()
                    .enumerate()
                    .map(|(index, range)| {
                        if index == *primary {
                            main
                        } else {
                            range
                                .map_through(changes, assoc)
                                .clamped(changes.len_after())
                        }
                    })
                    .collect();
                Self::normalized(mapped, main)
            }
        }
    }
    pub fn clamped(&self, len: usize) -> Self {
        match &self.0 {
            Storage::Single(range) => Self::single(range.clamped(len)),
            Storage::Multiple { ranges, .. } => Self::normalized(
                ranges.iter().map(|r| r.clamped(len)).collect(),
                self.primary().clamped(len),
            ),
        }
    }
    pub fn keep_primary(&self) -> Self {
        Self::single(self.primary())
    }
    pub(crate) fn owned_bytes(&self) -> usize {
        match &self.0 {
            Storage::Single(_) => 0,
            Storage::Multiple { ranges, .. } => ranges.capacity() * std::mem::size_of::<Range>(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn multi_normalizza_principale_e_limite() {
        let set = SelectionSet::try_new(
            vec![
                Range::new(8, 2),
                Range::new(6, 10),
                Range::point(10),
                Range::point(12),
            ],
            0,
            20,
        )
        .unwrap();
        assert_eq!(set.ranges(), &[Range::new(10, 2), Range::point(12)]);
        assert_eq!(set.primary_index(), 0);
        assert!(matches!(
            SelectionSet::try_new((0..1025).map(Range::point).collect(), 0, 1025),
            Err(SelectionError::Limit)
        ));
        assert_eq!(
            SelectionSet::try_new(vec![Range::point(3); 2048], 0, 10)
                .unwrap()
                .ranges(),
            &[Range::point(3)]
        );
    }

    #[test]
    fn multi_adiacenti_principale_invertito_e_mappa_unicode() {
        let set = SelectionSet::try_new(
            vec![Range::new(4, 2), Range::new(0, 2), Range::point(6)],
            0,
            6,
        )
        .unwrap();
        assert_eq!(
            set.ranges(),
            &[Range::new(0, 2), Range::new(4, 2), Range::point(6)]
        );
        assert_eq!(set.primary_index(), 1);
        let set = SelectionSet::try_new(vec![Range::point(1), Range::point(5)], 1, 6).unwrap();
        let mapped = set.map_through(&ChangeSet::insert_at(1, "é😀", 6), Assoc::After);
        assert_eq!(mapped.ranges(), &[Range::point(3), Range::point(7)]);
        assert_eq!(mapped.primary(), Range::point(7));
        assert_eq!(set.clamped(0).ranges(), &[Range::point(0)]);
        assert_eq!(set.keep_primary().ranges(), &[Range::point(5)]);
    }

    #[test]
    fn multi_undo_redo_e_reload_usano_le_selezioni_correnti() {
        use crate::{DocId, Document, PreparedText, Transaction};
        let mut doc = Document::from_str("abcdef", None, DocId(1));
        doc.set_selections(
            SelectionSet::try_new(vec![Range::point(1), Range::point(5)], 0, 6).unwrap(),
        );
        doc.apply(Transaction::with_selection(
            ChangeSet::insert_at(1, "X", 6),
            Range::point(2),
        ));
        assert_eq!(doc.rope().to_string(), "aXbcdef");
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(2), Range::point(6)]
        );
        doc.undo().unwrap();
        assert_eq!(doc.rope().to_string(), "abcdef");
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(1), Range::point(5)]
        );
        assert!(!doc.is_dirty());
        doc.redo().unwrap();
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(2), Range::point(6)]
        );
        let prepared = doc
            .read_snapshot()
            .prepare_reload(PreparedText::prepare("ab"));
        doc.set_selections(
            SelectionSet::try_new(vec![Range::point(1), Range::point(7)], 1, 7).unwrap(),
        );
        doc.reload_prepared(prepared).unwrap();
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(1), Range::point(2)]
        );
        assert_eq!(doc.selection(), Range::point(2));
        doc.undo().unwrap();
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(1), Range::point(7)]
        );
    }

    #[test]
    fn multi_principale_esplicito_non_cancella_secondaria_in_collisione() {
        use crate::{DocId, Document, Transaction};
        let mut doc = Document::from_str("abcdef", None, DocId(2));
        doc.set_selections(
            SelectionSet::try_new(vec![Range::point(1), Range::point(2)], 0, 6).unwrap(),
        );
        doc.apply(Transaction::with_selection(
            ChangeSet::delete_range(0, 3, 6),
            Range::point(2),
        ));
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(0), Range::point(2)]
        );
        assert_eq!(doc.selection(), Range::point(2));
        doc.undo().unwrap();
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(1), Range::point(2)]
        );
    }

    #[test]
    fn multi_gesti_distinti_non_si_fondono_nello_storico() {
        use crate::{DocId, Document, Transaction};
        let mut doc = Document::from_str("abcd", None, DocId(3));
        doc.set_selections(
            SelectionSet::try_new(vec![Range::point(1), Range::point(4)], 0, 4).unwrap(),
        );
        doc.apply(Transaction::new(ChangeSet::insert_at(1, "x", 4)));
        doc.apply(Transaction::new(ChangeSet::insert_at(2, "y", 5)));
        doc.undo().unwrap();
        assert_eq!(doc.rope().to_string(), "axbcd");
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(2), Range::point(5)]
        );
        doc.undo().unwrap();
        assert_eq!(doc.rope().to_string(), "abcd");
    }

    #[test]
    fn multi_selezioni_finali_e_reload_intero_ripristinano_insieme() {
        use crate::{DocId, Document, Transaction};
        let mut doc = Document::from_str("abcdef", None, DocId(4));
        let after = SelectionSet::try_new(vec![Range::point(2), Range::point(6)], 1, 7).unwrap();
        doc.apply_with_selections(
            Transaction::new(ChangeSet::insert_at(1, "X", 6)),
            after.clone(),
        );
        assert_eq!(doc.selections(), &after);
        doc.undo().unwrap();
        assert_eq!(doc.selections().ranges(), &[Range::point(0)]);
        doc.redo().unwrap();
        assert_eq!(doc.selections(), &after);
        doc.reload_from_str("abc");
        assert_eq!(
            doc.selections().ranges(),
            &[Range::point(2), Range::point(3)]
        );
        assert_eq!(doc.selection(), Range::point(3));
        assert!(!doc.is_dirty());
        doc.undo().unwrap();
        assert_eq!(doc.rope().to_string(), "aXbcdef");
        assert_eq!(doc.selections(), &after);
        assert!(doc.is_dirty());
    }

    #[test]
    fn multi_rifiuta_vuoto_e_principale_assente() {
        assert_eq!(
            SelectionSet::try_new(vec![], 0, 0),
            Err(SelectionError::Empty)
        );
        assert_eq!(
            SelectionSet::try_new(vec![Range::point(0)], 1, 0),
            Err(SelectionError::InvalidPrimary)
        );
    }
}
