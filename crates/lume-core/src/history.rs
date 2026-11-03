//! Modello degli edit e dello storico undo/redo, ispirato ai concetti di
//! helix-core (nessuna dipendenza): un `ChangeSet` descrive un edit come
//! sequenza di operazioni su tutto il documento, in CHAR INDEX.

use std::sync::atomic::{AtomicU64, Ordering};

use ropey::Rope;

use crate::SelectionSet;
use crate::selection::{Assoc, Range};

/// Operazione elementare di un `ChangeSet`. Le lunghezze sono in char.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Retain(usize),
    Delete(usize),
    Insert(String),
}

/// Descrizione completa di un edit: consuma un documento di `len_before`
/// char e ne produce uno di `len_after` char.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    ops: Vec<Op>,
    len_before: usize,
    len_after: usize,
}

impl ChangeSet {
    /// Costruttore generale: sostituisce `from..to` con `text`.
    /// Gli estremi fuori range vengono clampati (con debug_assert).
    pub fn replace_range(from: usize, to: usize, text: &str, doc_len: usize) -> Self {
        Self::replace_range_owned(from, to, text.to_owned(), doc_len)
    }

    /// Variante per preparazioni worker: trasferisce il buffer all'edit.
    pub fn replace_range_owned(from: usize, to: usize, text: String, doc_len: usize) -> Self {
        debug_assert!(from <= to && to <= doc_len, "range edit fuori dai limiti");
        let from = from.min(doc_len);
        let to = to.clamp(from, doc_len);
        let ins_len = text.chars().count();

        let mut ops = Vec::with_capacity(4);
        if from > 0 {
            ops.push(Op::Retain(from));
        }
        if to > from {
            ops.push(Op::Delete(to - from));
        }
        if !text.is_empty() {
            ops.push(Op::Insert(text));
        }
        if doc_len > to {
            ops.push(Op::Retain(doc_len - to));
        }
        ChangeSet {
            ops,
            len_before: doc_len,
            len_after: doc_len - (to - from) + ins_len,
        }
    }

    /// **Molte** sostituzioni sparse in un solo edit.
    ///
    /// Costruttore **e** validatore insieme: `None` se un intervallo è
    /// rovesciato, se due si sovrappongono o se uno esce dal documento. Non
    /// si clampa come [`ChangeSet::replace_range`] — lì il chiamante è
    /// l'editor e sa dov'è il cursore; qui gli intervalli arrivano da un
    /// language server o da una ricerca, cioè da fuori, e un intervallo
    /// sbagliato va **rifiutato**, non aggiustato in silenzio.
    ///
    /// Serve perché `Document::apply` prende **una** `ChangeSet` e non esiste
    /// una `compose`: senza questo, rinominare sette occorrenze sarebbe sette
    /// revisioni, e annullare vorrebbe sette Ctrl+Z. È la differenza fra una
    /// funzione usabile e una trappola.
    ///
    /// `edits` non deve essere ordinato: si ordina qui, ed è lì che si scopre
    /// una sovrapposizione.
    pub fn replace_many(edits: &[(usize, usize, String)], doc_len: usize) -> Option<Self> {
        let mut ordinati: Vec<&(usize, usize, String)> = edits.iter().collect();
        ordinati.sort_by_key(|(from, to, _)| (*from, *to));

        let mut ops = Vec::with_capacity(edits.len() * 3 + 1);
        let mut cursore = 0usize;
        let mut len_after = doc_len;
        for (from, to, text) in ordinati {
            let (from, to) = (*from, *to);
            // rovesciato, fuori dal documento, o sovrapposto al precedente
            if from > to || to > doc_len || from < cursore {
                return None;
            }
            if from > cursore {
                ops.push(Op::Retain(from - cursore));
            }
            if to > from {
                ops.push(Op::Delete(to - from));
            }
            if !text.is_empty() {
                ops.push(Op::Insert(text.clone()));
            }
            len_after = len_after + text.chars().count() - (to - from);
            cursore = to;
        }
        if doc_len > cursore {
            ops.push(Op::Retain(doc_len - cursore));
        }
        Some(ChangeSet {
            ops,
            len_before: doc_len,
            len_after,
        })
    }

    /// Inserimento di `text` alla posizione `pos`.
    pub fn insert_at(pos: usize, text: &str, doc_len: usize) -> Self {
        let pos = pos.min(doc_len);
        Self::replace_range(pos, pos, text, doc_len)
    }

    /// Cancellazione del range `from..to`.
    pub fn delete_range(from: usize, to: usize, doc_len: usize) -> Self {
        Self::replace_range(from, to, "", doc_len)
    }

    pub fn len_before(&self) -> usize {
        self.len_before
    }

    pub fn len_after(&self) -> usize {
        self.len_after
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// Applica il changeset alla rope (posizioni in char).
    pub fn apply(&self, rope: &mut Rope) {
        debug_assert_eq!(
            rope.len_chars(),
            self.len_before,
            "changeset non allineato al documento"
        );
        let mut pos = 0usize;
        for op in &self.ops {
            match op {
                Op::Retain(n) => pos += n,
                Op::Delete(n) => {
                    let end = (pos + n).min(rope.len_chars());
                    rope.remove(pos..end);
                }
                Op::Insert(s) => {
                    rope.insert(pos.min(rope.len_chars()), s);
                    pos += s.chars().count();
                }
            }
        }
        debug_assert_eq!(rope.len_chars(), self.len_after);
    }

    /// Costruisce il changeset inverso rispetto al documento `original`
    /// (lo stato PRIMA dell'applicazione di `self`).
    pub fn invert(&self, original: &Rope) -> ChangeSet {
        let mut ops = Vec::with_capacity(self.ops.len());
        let mut pos = 0usize; // posizione nel documento originale
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    ops.push(Op::Retain(*n));
                    pos += n;
                }
                Op::Delete(n) => {
                    // Per annullare una cancellazione serve il testo rimosso.
                    let end = (pos + n).min(original.len_chars());
                    ops.push(Op::Insert(original.slice(pos..end).to_string()));
                    pos += n;
                }
                Op::Insert(s) => ops.push(Op::Delete(s.chars().count())),
            }
        }
        ChangeSet {
            ops,
            len_before: self.len_after,
            len_after: self.len_before,
        }
    }

    /// Rimappa una posizione (char) attraverso l'edit. Con `Assoc::Before`
    /// la posizione resta prima di un inserimento nello stesso punto, con
    /// `Assoc::After` lo segue. Posizioni oltre la fine vengono clampate.
    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize {
        let mut old_pos = 0usize;
        let mut new_pos = 0usize;
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    if old_pos + n > pos {
                        return new_pos + (pos - old_pos);
                    }
                    old_pos += n;
                    new_pos += n;
                }
                Op::Delete(n) => {
                    // Posizione dentro il testo cancellato: collassa all'inizio.
                    if old_pos + n > pos {
                        return new_pos;
                    }
                    old_pos += n;
                }
                Op::Insert(s) => {
                    if old_pos == pos && assoc == Assoc::Before {
                        return new_pos;
                    }
                    new_pos += s.chars().count();
                }
            }
        }
        new_pos.min(self.len_after)
    }

    /// Se il changeset è un puro inserimento (nessuna Delete, una sola
    /// Insert) ritorna `(posizione, testo)`.
    pub fn as_simple_insert(&self) -> Option<(usize, &str)> {
        let mut pos = 0usize;
        let mut found: Option<(usize, &str)> = None;
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    if found.is_none() {
                        pos += n;
                    }
                }
                Op::Insert(s) => {
                    if found.is_some() {
                        return None;
                    }
                    found = Some((pos, s.as_str()));
                }
                Op::Delete(_) => return None,
            }
        }
        found
    }

    /// Se il changeset è una pura cancellazione (nessuna Insert, una sola
    /// Delete) ritorna `(posizione, lunghezza in char)`.
    pub fn as_simple_delete(&self) -> Option<(usize, usize)> {
        let mut pos = 0usize;
        let mut found: Option<(usize, usize)> = None;
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    if found.is_none() {
                        pos += n;
                    }
                }
                Op::Delete(n) => {
                    if found.is_some() {
                        return None;
                    }
                    found = Some((pos, *n));
                }
                Op::Insert(_) => return None,
            }
        }
        found
    }

    /// Span modificato in char: `(start, old_end, new_end)`, dove `start` è
    /// il primo char toccato, `old_end`/`new_end` la fine della modifica nel
    /// documento prima/dopo. `None` se il changeset non modifica nulla.
    pub fn change_span(&self) -> Option<(usize, usize, usize)> {
        let mut old_pos = 0usize;
        let mut new_pos = 0usize;
        let mut start: Option<usize> = None;
        let mut old_end = 0usize;
        let mut new_end = 0usize;
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    old_pos += n;
                    new_pos += n;
                }
                Op::Delete(n) => {
                    // Prima della prima op non-retain old_pos == new_pos.
                    start.get_or_insert(old_pos);
                    old_pos += n;
                    old_end = old_pos;
                    new_end = new_pos;
                }
                Op::Insert(s) => {
                    start.get_or_insert(old_pos);
                    new_pos += s.chars().count();
                    old_end = old_end.max(old_pos);
                    new_end = new_pos;
                }
            }
        }
        start.map(|s| (s, old_end.max(s), new_end.max(s)))
    }
}

/// Edit atomico dal punto di vista del chiamante: il changeset più la
/// selezione desiderata dopo l'applicazione (se `None` la selezione corrente
/// viene rimappata attraverso l'edit).
#[derive(Debug, Clone)]
pub struct Transaction {
    pub changes: ChangeSet,
    pub selection_after: Option<Range>,
}

impl Transaction {
    pub fn new(changes: ChangeSet) -> Self {
        Self {
            changes,
            selection_after: None,
        }
    }

    pub fn with_selection(changes: ChangeSet, selection_after: Range) -> Self {
        Self {
            changes,
            selection_after: Some(selection_after),
        }
    }
}

/// Voce dello storico: applicare `inverted` riporta il documento allo stato
/// identificato da `state_after`, con selezione `selection_before`.
#[derive(Debug, Clone)]
pub struct Revision {
    /// Changeset da applicare per tornare indietro (per lo stack undo è
    /// l'inverso dell'edit; per lo stack redo è l'edit in avanti).
    pub inverted: ChangeSet,
    /// Selezione da ripristinare dopo l'applicazione.
    pub selection_before: SelectionSet,
    /// Identità di contenuto del documento risultante (per `is_dirty`).
    pub state_after: u64,
}

impl Revision {
    /// Memoria direttamente posseduta dalla revisione. È un conteggio
    /// conservativo delle capacità, non una stima della RAM dell'allocator.
    fn owned_bytes(&self) -> usize {
        self.selection_before.owned_bytes()
            + self.inverted.ops.capacity() * std::mem::size_of::<Op>()
            + self
                .inverted
                .ops
                .iter()
                .map(|op| match op {
                    Op::Insert(text) => text.capacity(),
                    Op::Retain(_) | Op::Delete(_) => 0,
                })
                .sum::<usize>()
    }
}

#[derive(Debug, Clone)]
struct StoredRevision {
    revision: Revision,
    /// Sequenza globale: permette al workspace di eliminare davvero la voce
    /// eliminabile più vecchia, anche su documenti diversi.
    sequence: u64,
}

static NEXT_REVISION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Natura di un edit ai fini del coalescing della digitazione.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Inserimento di un singolo grapheme (non newline): può fondersi con
    /// il gruppo undo aperto se adiacente.
    CoalescibleInsert,
    /// Qualsiasi altro edit (delete, paste multi-char, newline...): chiude
    /// il gruppo e costituisce un passo undo a sé.
    Break,
}

/// Storico undo/redo con coalescing della digitazione: inserimenti
/// consecutivi di un singolo grapheme in posizioni adiacenti si fondono in
/// un'unica revisione finché il gruppo resta aperto. Il gruppo si chiude su
/// movimento del cursore, pausa (`commit_group`), newline o delete.
#[derive(Debug, Default)]
pub struct History {
    undo_stack: Vec<StoredRevision>,
    redo_stack: Vec<StoredRevision>,
    /// `true` se l'ultima revisione dello stack undo accetta coalescing.
    open: bool,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un nuovo edit. Se `kind` è coalescibile e il gruppo è
    /// aperto, l'edit si fonde con la revisione in cima allo stack.
    pub fn commit(&mut self, rev: Revision, kind: EditKind) {
        let kind = if rev.selection_before.ranges().len() > 1 {
            EditKind::Break
        } else {
            kind
        };
        self.redo_stack.clear();

        if kind == EditKind::CoalescibleInsert
            && self.open
            && let Some(last) = self.undo_stack.last_mut()
        {
            // Entrambe le revisioni annullano un inserimento, quindi sono
            // delete semplici: si fondono se la nuova inizia esattamente
            // dove finisce la precedente (digitazione adiacente).
            if let (Some((f1, l1)), Some((f2, l2))) = (
                last.revision.inverted.as_simple_delete(),
                rev.inverted.as_simple_delete(),
            ) && f1 + l1 == f2
            {
                // La revisione fusa deve applicarsi al documento più
                // recente: la ricostruiamo con la lunghezza aggiornata.
                last.revision.inverted =
                    ChangeSet::delete_range(f1, f2 + l2, rev.inverted.len_before());
                return;
            }
        }

        self.open = kind == EditKind::CoalescibleInsert;
        self.undo_stack.push(StoredRevision {
            revision: rev,
            sequence: NEXT_REVISION_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        });
    }

    /// Chiude il gruppo di coalescing corrente (movimento cursore, pausa
    /// di digitazione, salvataggio...).
    pub fn commit_group(&mut self) {
        self.open = false;
    }

    /// Estrae la revisione da annullare. `rope` è il documento CORRENTE
    /// (prima dell'applicazione), necessario per costruire la voce di redo;
    /// `selection` e `state` sono lo stato corrente del documento.
    pub fn undo(&mut self, rope: &Rope, selection: &SelectionSet, state: u64) -> Option<Revision> {
        self.open = false;
        let stored = self.undo_stack.pop()?;
        self.redo_stack.push(StoredRevision {
            revision: Revision {
                inverted: stored.revision.inverted.invert(rope),
                selection_before: selection.clone(),
                state_after: state,
            },
            sequence: stored.sequence,
        });
        Some(stored.revision)
    }

    /// Estrae la revisione da riapplicare (simmetrico a `undo`).
    pub fn redo(&mut self, rope: &Rope, selection: &SelectionSet, state: u64) -> Option<Revision> {
        self.open = false;
        let stored = self.redo_stack.pop()?;
        self.undo_stack.push(StoredRevision {
            revision: Revision {
                inverted: stored.revision.inverted.invert(rope),
                selection_before: selection.clone(),
                state_after: state,
            },
            sequence: stored.sequence,
        });
        Some(stored.revision)
    }

    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo_stack.len()
    }

    pub fn retained_bytes(&self) -> usize {
        self.undo_stack.capacity() * std::mem::size_of::<StoredRevision>()
            + self.redo_stack.capacity() * std::mem::size_of::<StoredRevision>()
            + self
                .undo_stack
                .iter()
                .chain(&self.redo_stack)
                .map(|stored| stored.revision.owned_bytes())
                .sum::<usize>()
    }

    /// Elimina una revisione intera, dalla coda cronologica più vecchia. Non
    /// elimina mai l'unica operazione undo rimasta: una paste enorme resta
    /// quindi un'eccezione dichiarata al budget, non una perdita silenziosa.
    pub fn discard_oldest_evictable(&mut self) -> bool {
        let undo = (self.undo_stack.len() > 1)
            .then(|| self.undo_stack.first().map(|r| r.sequence))
            .flatten();
        let redo = self.redo_stack.first().map(|r| r.sequence);
        match (undo, redo) {
            (None, None) => false,
            (Some(_), None) => {
                self.undo_stack.remove(0);
                self.undo_stack.shrink_to_fit();
                true
            }
            (None, Some(_)) => {
                self.redo_stack.remove(0);
                self.redo_stack.shrink_to_fit();
                true
            }
            (Some(undo), Some(redo)) if undo <= redo => {
                self.undo_stack.remove(0);
                self.undo_stack.shrink_to_fit();
                true
            }
            (Some(_), Some(_)) => {
                self.redo_stack.remove(0);
                self.redo_stack.shrink_to_fit();
                true
            }
        }
    }

    pub fn oldest_evictable_sequence(&self) -> Option<u64> {
        let undo = (self.undo_stack.len() > 1)
            .then(|| self.undo_stack.first().map(|stored| stored.sequence))
            .flatten();
        let redo = self.redo_stack.first().map(|stored| stored.sequence);
        match (undo, redo) {
            (Some(undo), Some(redo)) => Some(undo.min(redo)),
            (Some(sequence), None) | (None, Some(sequence)) => Some(sequence),
            (None, None) => None,
        }
    }

    pub fn trim_to_budget(&mut self, budget: usize) -> bool {
        let mut trimmed = false;
        while self.retained_bytes() > budget && self.discard_oldest_evictable() {
            trimmed = true;
        }
        trimmed
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn owned_replacement_moves_the_insert_buffer() {
        let text = "λ🙂testo".to_owned();
        let pointer = text.as_ptr();
        let expected = ChangeSet::replace_range(0, 3, &text, 3);
        let actual = ChangeSet::replace_range_owned(0, 3, text, 3);
        assert_eq!(actual, expected);
        assert!(
            actual
                .ops()
                .iter()
                .any(|op| matches!(op, Op::Insert(text) if text.as_ptr() == pointer))
        );
    }

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    fn revision_with_text(text: &str) -> Revision {
        Revision {
            inverted: ChangeSet::insert_at(0, text, 0),
            selection_before: SelectionSet::single(Range::point(0)),
            state_after: 0,
        }
    }

    #[test]
    fn multi_budget_include_i_range_posseduti_dalla_revisione() {
        let mut rev = revision_with_text("");
        let base = rev.owned_bytes();
        assert_eq!(rev.selection_before.owned_bytes(), 0);
        rev.selection_before =
            SelectionSet::try_new((0..1024).map(Range::point).collect(), 0, 1024).unwrap();
        assert!(rev.owned_bytes() >= base + 1024 * std::mem::size_of::<Range>());
    }

    #[test]
    fn il_budget_elimina_dal_lato_meno_recente_ma_tiene_l_ultimo_undo() {
        let mut history = History::new();
        history.commit(
            revision_with_text("prima revisione abbastanza lunga"),
            EditKind::Break,
        );
        history.commit(
            revision_with_text("seconda revisione abbastanza lunga"),
            EditKind::Break,
        );
        assert!(history.trim_to_budget(1));
        assert_eq!(history.undo_depth(), 1);
        assert!(
            history.retained_bytes() > 1,
            "l'ultima operazione e' l'eccezione dichiarata"
        );
    }

    /// Molte sostituzioni sono **una** revisione: annullare una rinomina di
    /// sette occorrenze non deve volere sette Ctrl+Z.
    #[test]
    fn molte_sostituzioni_sono_un_edit_solo_e_si_annullano_insieme() {
        let mut r = rope("foo + foo * foo");
        let edits = [
            (0, 3, "bar".to_owned()),
            (6, 9, "bar".to_owned()),
            (12, 15, "bar".to_owned()),
        ];
        let cs = ChangeSet::replace_many(&edits, r.len_chars()).unwrap();
        let inv = cs.invert(&r);
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "bar + bar * bar");
        inv.apply(&mut r);
        assert_eq!(r.to_string(), "foo + foo * foo", "una sola invertita");
    }

    /// Gli intervalli arrivano da fuori — un language server, una ricerca —
    /// e uno sbagliato va **rifiutato**, non aggiustato in silenzio: applicare
    /// mezzo edit a un file di qualcun altro è il modo peggiore di fallire.
    #[test]
    fn gli_intervalli_che_non_tornano_si_rifiutano_invece_di_clampare() {
        let doc_len = rope("abcdefghij").len_chars();
        // sovrapposti
        assert!(
            ChangeSet::replace_many(&[(0, 5, "x".into()), (3, 7, "y".into())], doc_len).is_none()
        );
        // rovesciato
        assert!(ChangeSet::replace_many(&[(7, 3, "x".into())], doc_len).is_none());
        // fuori dal documento
        assert!(ChangeSet::replace_many(&[(8, 42, "x".into())], doc_len).is_none());
        // adiacenti (non sovrapposti) invece vanno
        assert!(
            ChangeSet::replace_many(&[(0, 3, "x".into()), (3, 5, "y".into())], doc_len).is_some()
        );
    }

    /// Non serve che arrivino ordinati: li ordina il costruttore, ed è lì che
    /// si scopre la sovrapposizione.
    #[test]
    fn l_ordine_degli_intervalli_non_conta() {
        let mut r = rope("uno due tre");
        let disordinati = [(8, 11, "TRE".to_owned()), (0, 3, "UNO".to_owned())];
        let cs = ChangeSet::replace_many(&disordinati, r.len_chars()).unwrap();
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "UNO due TRE");
    }

    /// Un edit vuoto è legittimo: è il documento tale e quale.
    #[test]
    fn nessuna_sostituzione_lascia_il_documento_intatto() {
        let mut r = rope("intatto");
        let cs = ChangeSet::replace_many(&[], r.len_chars()).unwrap();
        assert_eq!(cs.len_after(), r.len_chars());
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "intatto");
    }

    /// I char non sono byte: un'emoji occupa un char e quattro byte, e
    /// sbagliare qui darebbe indici giusti sull'ASCII e sballati sul resto.
    #[test]
    fn gli_indici_sono_char_anche_con_le_emoji() {
        let mut r = rope("🙂 foo 🙂 foo");
        // "🙂 foo 🙂 foo" = [🙂][ ][f][o][o][ ][🙂][ ][f][o][o]
        let edits = [(2, 5, "bar".to_owned()), (8, 11, "bar".to_owned())];
        let cs = ChangeSet::replace_many(&edits, r.len_chars()).unwrap();
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "🙂 bar 🙂 bar");
    }

    #[test]
    fn insert_apply_e_invert_roundtrip() {
        let mut r = rope("hello world");
        let cs = ChangeSet::insert_at(5, ", brave", r.len_chars());
        let inv = cs.invert(&r);
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "hello, brave world");
        inv.apply(&mut r);
        assert_eq!(r.to_string(), "hello world");
    }

    #[test]
    fn delete_apply_e_invert_roundtrip() {
        let mut r = rope("hello world");
        let cs = ChangeSet::delete_range(5, 11, r.len_chars());
        let inv = cs.invert(&r);
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "hello");
        inv.apply(&mut r);
        assert_eq!(r.to_string(), "hello world");
    }

    #[test]
    fn replace_apply_e_invert_roundtrip() {
        let mut r = rope("abcdef");
        let cs = ChangeSet::replace_range(2, 4, "XYZ", r.len_chars());
        let inv = cs.invert(&r);
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "abXYZef");
        inv.apply(&mut r);
        assert_eq!(r.to_string(), "abcdef");
    }

    #[test]
    fn map_pos_attraverso_insert() {
        let cs = ChangeSet::insert_at(3, "xx", 10);
        assert_eq!(cs.map_pos(2, Assoc::After), 2);
        assert_eq!(cs.map_pos(3, Assoc::Before), 3);
        assert_eq!(cs.map_pos(3, Assoc::After), 5);
        assert_eq!(cs.map_pos(5, Assoc::After), 7);
        assert_eq!(cs.map_pos(10, Assoc::After), 12);
        // oltre la fine: clamp
        assert_eq!(cs.map_pos(99, Assoc::After), 12);
    }

    #[test]
    fn map_pos_attraverso_delete() {
        let cs = ChangeSet::delete_range(2, 5, 10);
        assert_eq!(cs.map_pos(1, Assoc::After), 1);
        assert_eq!(cs.map_pos(3, Assoc::After), 2); // dentro la cancellazione
        assert_eq!(cs.map_pos(5, Assoc::After), 2);
        assert_eq!(cs.map_pos(7, Assoc::After), 4);
    }

    #[test]
    fn edit_semplici_riconosciuti() {
        let ins = ChangeSet::insert_at(3, "ab", 10);
        assert_eq!(ins.as_simple_insert(), Some((3, "ab")));
        assert_eq!(ins.as_simple_delete(), None);

        let del = ChangeSet::delete_range(2, 6, 10);
        assert_eq!(del.as_simple_delete(), Some((2, 4)));
        assert_eq!(del.as_simple_insert(), None);

        let rep = ChangeSet::replace_range(2, 4, "x", 10);
        assert_eq!(rep.as_simple_insert(), None);
        assert_eq!(rep.as_simple_delete(), None);
    }

    #[test]
    fn change_span_su_replace() {
        let cs = ChangeSet::replace_range(3, 8, "XY", 12);
        assert_eq!(cs.change_span(), Some((3, 8, 5)));
        let noop = ChangeSet::delete_range(4, 4, 12);
        assert_eq!(noop.change_span(), None);
    }

    #[test]
    fn clamp_su_range_fuori_limite() {
        // In release il range viene clampato senza panic.
        let cs = ChangeSet::insert_at(50, "x", 5);
        let mut r = rope("abcde");
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "abcdex");
    }
}
