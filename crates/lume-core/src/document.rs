//! Documento aperto: rope + selezione + storico undo/redo + metadati di
//! salvataggio (fine riga originale, BOM, versioni).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use crate::SelectionSet;
use crate::history::{ChangeSet, EditKind, History, Revision, Transaction};
use crate::line_ending::{LineEnding, normalize, strip_bom};
use crate::selection::{Assoc, Range};

/// Budget locale dello storico: un terzo del tetto globale riservato all'undo.
pub const DOCUMENT_HISTORY_BUDGET: usize = 16 * 1024 * 1024;

/// Identificatore di un documento nel workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub u64);

/// Contenuto disco preparato fuori UI; clone condivide il Rope.
#[derive(Clone)]
pub struct PreparedText {
    rope: Rope,
    line_ending: LineEnding,
    bom: bool,
    coalescible_insert: bool,
}
impl PreparedText {
    pub fn prepare(raw: &str) -> Self {
        let (text, bom) = strip_bom(raw);
        let (normalized, line_ending) = normalize(text);
        Self {
            rope: Rope::from_str(&normalized),
            line_ending,
            bom,
            coalescible_insert: !normalized.contains('\n')
                && normalized.graphemes(true).take(2).count() == 1,
        }
    }
}
impl fmt::Debug for PreparedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedText")
            .field("caratteri", &self.rope.len_chars())
            .finish_non_exhaustive()
    }
}

/// Reload completo legato alla fotografia originale, senza payload nel Debug.
pub struct PreparedReload {
    id: DocId,
    identity: Arc<()>,
    version: u64,
    state_id: u64,
    text: PreparedText,
    inverted: ChangeSet,
    edit: AppliedEdit,
    kind: EditKind,
}
impl fmt::Debug for PreparedReload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedReload")
            .field("id", &self.id)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// Risultato dell'applicazione di un edit, con le informazioni necessarie a
/// un consumer incrementale (es. tree-sitter `InputEdit`): offset in BYTE e
/// point `(riga, colonna in byte)`, calcolati prima e dopo l'edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
    /// Prima riga toccata dall'edit (nel documento prima e dopo coincide).
    pub first_changed_line: usize,
    /// Variazione del numero di righe.
    pub line_delta: isize,
    pub new_version: u64,
}

/// Buffer di testo dell'editor. Tutte le posizioni sono in char index.
#[derive(Debug)]
pub struct Document {
    id: DocId,
    rope: Rope,
    path: Option<PathBuf>,
    line_ending: LineEnding,
    bom: bool,
    selection: SelectionSet,
    /// Identità dell'istanza e del legame: DocId riutilizzati o percorsi
    /// cambiati non accettano ricevute di salvataggio precedenti.
    save_identity: Arc<()>,
    /// Contatore monotono: cresce a ogni apply/undo/redo (per invalidazioni).
    version: u64,
    history: History,
    /// Versione al momento dell'ultimo salvataggio (informativa).
    saved_version: u64,
    /// Identità del CONTENUTO corrente: l'undo la riporta al valore dello
    /// stato precedente, rendendo `is_dirty` robusto all'undo oltre il save.
    state_id: u64,
    /// Identità del contenuto salvato.
    saved_state_id: u64,
    /// Ultimo edit applicato (apply/undo/redo): permette ai consumer di
    /// invalidare solo il range toccato invece dell'intero documento.
    last_edit: Option<AppliedEdit>,
    history_trimmed: bool,
}

/// Fotografia immutabile e condivisa del contenuto da salvare.
///
/// La clone della rope non materializza l'intero file sul thread UI; payload
/// e I/O possono essere elaborati dal worker. Il Debug non stampa il testo.
#[derive(Clone)]
pub struct SaveSnapshot {
    id: DocId,
    identity: Arc<()>,
    version: u64,
    state_id: u64,
    rope: Rope,
    line_ending: LineEnding,
    bom: bool,
}

impl fmt::Debug for SaveSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveSnapshot")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("caratteri", &self.rope.len_chars())
            .finish_non_exhaustive()
    }
}

impl SaveSnapshot {
    /// Normalizzazione, inverse undo e coordinate vengono preparate sul worker.
    pub fn prepare_reload(self, text: PreparedText) -> PreparedReload {
        let old_len = self.rope.len_chars();
        let new_len = text.rope.len_chars();
        let (old_end_byte, old_end_point) = byte_point(&self.rope, old_len);
        let (new_end_byte, new_end_point) = byte_point(&text.rope, new_len);
        let edit = AppliedEdit {
            start_byte: 0,
            old_end_byte,
            new_end_byte,
            start_point: (0, 0),
            old_end_point,
            new_end_point,
            first_changed_line: 0,
            line_delta: text.rope.len_lines() as isize - self.rope.len_lines() as isize,
            new_version: self.version + 1,
        };
        let inverted = ChangeSet::replace_range_owned(0, new_len, self.rope.to_string(), new_len);
        let kind = if old_len == 0 && text.coalescible_insert {
            EditKind::CoalescibleInsert
        } else {
            EditKind::Break
        };
        PreparedReload {
            id: self.id,
            identity: self.identity,
            version: self.version,
            state_id: self.state_id,
            text,
            inverted,
            edit,
            kind,
        }
    }

    pub fn id(&self) -> DocId {
        self.id
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Rope condivisa immutabile, per preparazioni sul worker senza payload.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Materializza solo la fotografia, conservando BOM e fine riga.
    /// Da chiamare fuori dal thread dell'interfaccia per file grandi.
    pub fn payload(&self) -> Vec<u8> {
        save_payload(&self.rope, self.line_ending, self.bom)
    }

    /// Corpo normalizzato, senza BOM: per store con intestazione propria.
    /// Anche questa materializzazione va eseguita sul worker.
    pub fn normalized_text(&self) -> String {
        self.rope.to_string()
    }
}

impl Document {
    /// Crea un documento dal testo grezzo: strippa il BOM e normalizza i
    /// fine riga a `\n`, ricordando entrambi per il salvataggio.
    pub fn from_str(text: &str, path: Option<PathBuf>, id: DocId) -> Self {
        Self::from_prepared(PreparedText::prepare(text), path, id)
    }

    pub fn from_prepared(text: PreparedText, path: Option<PathBuf>, id: DocId) -> Self {
        Document {
            id,
            rope: text.rope,
            path,
            line_ending: text.line_ending,
            bom: text.bom,
            selection: SelectionSet::single(Range::point(0)),
            save_identity: Arc::new(()),
            version: 0,
            history: History::new(),
            saved_version: 0,
            state_id: 0,
            saved_state_id: 0,
            last_edit: None,
            history_trimmed: false,
        }
    }

    pub fn id(&self) -> DocId {
        self.id
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Solo Workspace aggiorna anche l'indice per percorso. Non scrive file.
    /// Un legame diverso revoca le fotografie precedenti, anche tornando poi
    /// al percorso originario. Il caller applica l'ack della scrittura riuscita
    /// prima del rebind; testo, undo e stato saved/dirty non cambiano qui.
    pub(crate) fn set_path(&mut self, path: PathBuf) {
        if self.path.as_deref() != Some(path.as_path()) {
            self.save_identity = Arc::new(());
            self.path = Some(path);
        }
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn bom(&self) -> bool {
        self.bom
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Ultimo edit applicato (apply/undo/redo), se ce n'è stato uno.
    pub fn last_edit(&self) -> Option<&AppliedEdit> {
        self.last_edit.as_ref()
    }

    pub fn selection(&self) -> Range {
        self.selection.primary()
    }

    pub fn selections(&self) -> &SelectionSet {
        &self.selection
    }
    pub fn set_selections(&mut self, selection: SelectionSet) {
        self.selection = selection.clamped(self.rope.len_chars());
        self.history.commit_group();
    }
    /// Applica un gesto che definisce tutte le selezioni risultanti.
    pub fn apply_with_selections(&mut self, tx: Transaction, after: SelectionSet) -> AppliedEdit {
        self.apply_inner(tx, Some(after))
    }

    /// Sposta la selezione. Un movimento esplicito del cursore chiude il
    /// gruppo di coalescing dell'undo.
    pub fn set_selection(&mut self, selection: Range) {
        self.selection = SelectionSet::single(selection.clamped(self.rope.len_chars()));
        self.history.commit_group();
    }

    /// Chiude il gruppo undo corrente (da invocare p.es. su pausa di
    /// digitazione o salvataggio).
    pub fn commit_undo_group(&mut self) {
        self.history.commit_group();
    }

    /// Applica una transazione, registrandola nello storico.
    pub fn apply(&mut self, tx: Transaction) -> AppliedEdit {
        self.apply_inner(tx, None)
    }

    fn apply_inner(&mut self, tx: Transaction, after: Option<SelectionSet>) -> AppliedEdit {
        debug_assert_eq!(
            tx.changes.len_before(),
            self.rope.len_chars(),
            "transazione costruita su una versione diversa del documento"
        );
        let selection_before = self.selection.clone();
        let selection_after = after.map_or_else(
            || selection_before.map_with_primary(&tx.changes, Assoc::After, tx.selection_after),
            |set| set.clamped(tx.changes.len_after()),
        );
        let state_before = self.state_id;
        let inverted = tx.changes.invert(&self.rope);

        // Un inserimento di un singolo grapheme (non newline) è coalescibile
        // con la digitazione precedente.
        let kind = match tx.changes.as_simple_insert() {
            Some((_, s))
                if selection_before.ranges().len() == 1
                    && selection_after.ranges().len() == 1
                    && !s.contains('\n')
                    && s.graphemes(true).count() == 1 =>
            {
                EditKind::CoalescibleInsert
            }
            _ => EditKind::Break,
        };

        let edit = self.apply_changes(&tx.changes);
        self.state_id = self.version;
        self.history.commit(
            Revision {
                inverted,
                selection_before,
                state_after: state_before,
            },
            kind,
        );
        self.history_trimmed = self.history.trim_to_budget(DOCUMENT_HISTORY_BUDGET);
        self.selection = selection_after;
        edit
    }

    /// Annulla l'ultimo gruppo di edit. `None` se lo storico è vuoto.
    pub fn undo(&mut self) -> Option<AppliedEdit> {
        let rev = self
            .history
            .undo(&self.rope, &self.selection, self.state_id)?;
        Some(self.restore(rev))
    }

    /// Riapplica l'ultimo gruppo annullato. `None` se non c'è nulla da rifare.
    pub fn redo(&mut self) -> Option<AppliedEdit> {
        let rev = self
            .history
            .redo(&self.rope, &self.selection, self.state_id)?;
        Some(self.restore(rev))
    }

    /// Ritorna e azzera il segnale di riduzione dello storico, per un avviso
    /// UI non modale e accorpabile dal chiamante.
    pub fn take_history_trimmed(&mut self) -> bool {
        std::mem::take(&mut self.history_trimmed)
    }

    pub fn history_bytes(&self) -> usize {
        self.history.retained_bytes()
    }

    pub(crate) fn discard_oldest_history_revision(&mut self) -> bool {
        let trimmed = self.history.discard_oldest_evictable();
        self.history_trimmed |= trimmed;
        trimmed
    }

    pub(crate) fn oldest_evictable_history_sequence(&self) -> Option<u64> {
        self.history.oldest_evictable_sequence()
    }

    /// Applica una revisione dello storico ripristinando selezione e
    /// identità di contenuto.
    fn restore(&mut self, rev: Revision) -> AppliedEdit {
        let edit = self.apply_changes(&rev.inverted);
        self.state_id = rev.state_after;
        self.selection = rev.selection_before.clamped(self.rope.len_chars());
        edit
    }

    /// Applica il changeset alla rope calcolando gli offset byte/point
    /// PRIMA (start, old_end) e DOPO (new_end) l'applicazione.
    fn apply_changes(&mut self, changes: &ChangeSet) -> AppliedEdit {
        let (start, old_end, new_end) = changes.change_span().unwrap_or((
            changes.len_before(),
            changes.len_before(),
            changes.len_after(),
        ));

        let old_lines = self.rope.len_lines();
        let (start_byte, start_point) = byte_point(&self.rope, start);
        let (old_end_byte, old_end_point) = byte_point(&self.rope, old_end);

        changes.apply(&mut self.rope);
        self.version += 1;

        let (new_end_byte, new_end_point) = byte_point(&self.rope, new_end);

        let edit = AppliedEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_point,
            old_end_point,
            new_end_point,
            first_changed_line: start_point.0,
            line_delta: self.rope.len_lines() as isize - old_lines as isize,
            new_version: self.version,
        };
        self.last_edit = Some(edit);
        edit
    }

    /// `true` se il contenuto differisce dall'ultimo salvataggio. Robusto
    /// all'undo oltre il punto di save: confronta identità di contenuto,
    /// non contatori monotoni.
    pub fn is_dirty(&self) -> bool {
        self.state_id != self.saved_state_id
    }

    /// Registra che il contenuto corrente è stato salvato su disco.
    /// Per scritture asincrone usare invece mark_snapshot_saved.
    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
        self.saved_state_id = self.state_id;
    }

    /// Fotografa il contenuto e separa i gruppi undo prima/dopo il save.
    /// Nessun I/O o buffer di byte completo viene creato qui.
    pub fn snapshot_for_save(&mut self) -> SaveSnapshot {
        self.history.commit_group();
        self.read_snapshot()
    }

    /// Fotografia senza modificare il gruppo undo, per un reader worker.
    pub fn read_snapshot(&self) -> SaveSnapshot {
        SaveSnapshot {
            id: self.id,
            identity: self.save_identity.clone(),
            version: self.version,
            state_id: self.state_id,
            rope: self.rope.clone(),
            line_ending: self.line_ending,
            bom: self.bom,
        }
    }

    /// La fotografia appartiene ancora a questa istanza e revisione esatta?
    /// Non conferma salvataggi né modifica history/dirty.
    pub fn matches_snapshot(&self, snapshot: &SaveSnapshot) -> bool {
        snapshot.id == self.id
            && Arc::ptr_eq(&snapshot.identity, &self.save_identity)
            && snapshot.version == self.version
    }

    /// La fotografia può ancora ricevere un acknowledgment in questa istanza?
    /// Non modifica testo, dirty o gruppi undo e ammette edit successivi.
    /// È una verifica puntuale: non concede autorità filesystem, non riserva
    /// il percorso e non garantisce che una conferma futura sia ancora valida.
    pub fn can_mark_snapshot_saved(&self, snapshot: &SaveSnapshot) -> bool {
        snapshot.id == self.id
            && Arc::ptr_eq(&snapshot.identity, &self.save_identity)
            && snapshot.version >= self.saved_version
            && snapshot.version <= self.version
    }

    /// Registra solo il contenuto della fotografia effettivamente scritto.
    /// Le modifiche successive restano sporche; undo al save torna pulito.
    /// Una conferma di un'altra istanza o precedente a save/reload già
    /// registrati viene rifiutata. Il chiamante deve comunque serializzare
    /// le scritture e verificare percorso/epoca: questo non protegge il disco.
    pub fn mark_snapshot_saved(&mut self, snapshot: &SaveSnapshot) -> bool {
        if !self.can_mark_snapshot_saved(snapshot) {
            return false;
        }
        self.saved_version = snapshot.version;
        self.saved_state_id = snapshot.state_id;
        true
    }

    /// Sostituisce l'intero contenuto con `text` (grezzo da disco): strippa
    /// il BOM, normalizza i fine riga e applica il tutto come edit regolare
    /// (quindi UNDOABILE — l'undo dopo un reload esterno ripristina il
    /// contenuto precedente e rende il documento sporco), poi marca il
    /// contenuto come salvato. Id, tab e storico restano intatti.
    pub fn reload_from_str(&mut self, text: &str) -> AppliedEdit {
        let (text, bom) = strip_bom(text);
        let (normalized, line_ending) = normalize(text);
        self.bom = bom;
        self.line_ending = line_ending;
        self.history.commit_group();
        let len = self.rope.len_chars();
        let after = self.selection.clamped(normalized.chars().count());
        let edit = self.apply_with_selections(
            Transaction::new(ChangeSet::replace_range(0, len, &normalized, len)),
            after,
        );
        self.mark_saved();
        edit
    }

    /// Applique un résultat préparé sans normalisation, reconstruction Rope
    /// ou copie complète pour l'undo. Refuse un résultat devenu obsolète.
    pub fn reload_prepared(&mut self, prepared: PreparedReload) -> Option<AppliedEdit> {
        if self.id != prepared.id
            || !Arc::ptr_eq(&self.save_identity, &prepared.identity)
            || self.version != prepared.version
            || self.state_id != prepared.state_id
        {
            return None;
        }
        self.history.commit_group();
        let selection_before = self.selection.clone();
        let state_before = self.state_id;
        self.bom = prepared.text.bom;
        self.line_ending = prepared.text.line_ending;
        self.rope = prepared.text.rope;
        self.version = prepared.edit.new_version;
        self.state_id = self.version;
        self.history.commit(
            Revision {
                inverted: prepared.inverted,
                selection_before: selection_before.clone(),
                state_after: state_before,
            },
            prepared.kind,
        );
        self.history_trimmed = self.history.trim_to_budget(DOCUMENT_HISTORY_BUDGET);
        self.selection = selection_before.clamped(self.rope.len_chars());
        self.last_edit = Some(prepared.edit);
        self.mark_saved();
        Some(prepared.edit)
    }

    /// Byte da scrivere su disco: riapplica il fine riga originale e il BOM.
    pub fn save_payload(&self) -> Vec<u8> {
        save_payload(&self.rope, self.line_ending, self.bom)
    }
}

fn save_payload(rope: &Rope, line_ending: LineEnding, bom: bool) -> Vec<u8> {
    let restored = line_ending.restore(&rope.to_string());
    let mut out = Vec::with_capacity(restored.len() + 3);
    if bom {
        out.extend_from_slice(b"\xEF\xBB\xBF");
    }
    out.extend_from_slice(restored.as_bytes());
    out
}

/// Offset in byte e point `(riga, colonna in byte)` di una posizione char.
fn byte_point(rope: &Rope, char_pos: usize) -> (usize, (usize, usize)) {
    let char_pos = char_pos.min(rope.len_chars());
    let byte = rope.char_to_byte(char_pos);
    let row = rope.byte_to_line(byte);
    let col = byte - rope.line_to_byte(row);
    (byte, (row, col))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn doc(text: &str) -> Document {
        Document::from_str(text, None, DocId(1))
    }

    #[test]
    fn prepared_reload_matches_legacy_format_history_and_edit() {
        for (old, new) in [
            ("α\nβ", "\u{feff}λ\r\n🙂\r\n"),
            ("vecchio", ""),
            ("", "a\rb\r\nc\n"),
            ("", ""),
        ] {
            let mut prepared = doc(old);
            let mut legacy = doc(old);
            for d in [&mut prepared, &mut legacy] {
                type_str(d, 0, "!");
                d.set_selection(Range::new(1, d.rope().len_chars()));
            }
            let identity = prepared.save_identity.clone();
            let snapshot = prepared.read_snapshot();
            let raw = new.to_owned();
            let result =
                std::thread::spawn(move || snapshot.prepare_reload(PreparedText::prepare(&raw)))
                    .join()
                    .unwrap();
            assert_eq!(
                prepared.reload_prepared(result),
                Some(legacy.reload_from_str(new))
            );
            assert!(Arc::ptr_eq(&identity, &prepared.save_identity));
            assert_eq!(prepared.save_payload(), legacy.save_payload());
            assert_eq!(prepared.selection(), legacy.selection());
            assert_eq!(prepared.is_dirty(), legacy.is_dirty());
            for redo in [false, true, false, false] {
                let (a, b) = if redo {
                    (prepared.redo(), legacy.redo())
                } else {
                    (prepared.undo(), legacy.undo())
                };
                assert_eq!(a, b);
                assert_eq!(prepared.rope().to_string(), legacy.rope().to_string());
                assert_eq!(prepared.is_dirty(), legacy.is_dirty());
                assert_eq!(prepared.selection(), legacy.selection());
            }
        }
    }

    #[test]
    fn prepared_reload_rejects_edit_rebind_and_foreign_identity() {
        for kind in 0..3 {
            let mut d = doc("origine");
            let prepared = d
                .read_snapshot()
                .prepare_reload(PreparedText::prepare("\u{feff}nuovo\r\n"));
            match kind {
                0 => {
                    type_str(&mut d, 0, "edit");
                }
                1 => d.set_path(PathBuf::from("altro.txt")),
                _ => d = doc("origine"),
            }
            let before = (d.save_payload(), d.version(), d.selection(), d.is_dirty());
            assert!(d.reload_prepared(prepared).is_none());
            assert_eq!(
                before,
                (d.save_payload(), d.version(), d.selection(), d.is_dirty())
            );
        }
    }

    #[test]
    fn prepared_open_preserves_format_and_redacts_debug() {
        let prepared = PreparedText::prepare("\u{feff}contenuto-segreto\r\nλ");
        assert!(!format!("{prepared:?}").contains("contenuto-segreto"));
        let d = Document::from_prepared(prepared, None, DocId(9));
        assert_eq!(d.rope().to_string(), "contenuto-segreto\nλ");
        assert_eq!(
            d.save_payload(),
            "\u{feff}contenuto-segreto\r\nλ".as_bytes()
        );
        assert!(!d.is_dirty());
    }

    #[test]
    fn prepared_reload_into_empty_keeps_legacy_undo_grouping() {
        for raw in ["", "x", "a\u{301}", "🙂", "a\n"] {
            let mut prepared = doc("");
            let mut legacy = doc("");
            let result = prepared
                .read_snapshot()
                .prepare_reload(PreparedText::prepare(raw));
            assert_eq!(
                prepared.reload_prepared(result),
                Some(legacy.reload_from_str(raw))
            );
            for d in [&mut prepared, &mut legacy] {
                type_str(d, d.rope().len_chars(), "y");
            }
            assert_eq!(prepared.undo(), legacy.undo());
            assert_eq!(prepared.rope().to_string(), legacy.rope().to_string());
            assert_eq!(prepared.is_dirty(), legacy.is_dirty());
        }
    }

    #[test]
    fn il_preflight_rifiuta_versioni_future_o_id_estranei_senza_mutazioni() {
        let mut d = doc("prima");
        let mut snapshot = d.snapshot_for_save();
        assert!(d.can_mark_snapshot_saved(&snapshot));
        snapshot.version = d.version() + 1;
        assert!(!d.can_mark_snapshot_saved(&snapshot));
        assert!(!d.mark_snapshot_saved(&snapshot));
        snapshot.version = d.version();
        snapshot.id = DocId(2);
        assert!(!d.can_mark_snapshot_saved(&snapshot));
        assert!(!d.mark_snapshot_saved(&snapshot));
        assert_eq!(d.saved_version, 0);
        assert_eq!(d.saved_state_id, 0);
        assert_eq!(d.rope().to_string(), "prima");
        assert!(!d.is_dirty());
    }

    /// Simula la digitazione: insert con caret spostato in coda al testo.
    fn type_str(d: &mut Document, pos: usize, text: &str) -> AppliedEdit {
        let len = d.rope().len_chars();
        d.apply(Transaction::with_selection(
            ChangeSet::insert_at(pos, text, len),
            Range::point(pos + text.chars().count()),
        ))
    }

    fn delete(d: &mut Document, from: usize, to: usize) -> AppliedEdit {
        let len = d.rope().len_chars();
        d.apply(Transaction::with_selection(
            ChangeSet::delete_range(from, to, len),
            Range::point(from),
        ))
    }

    #[test]
    fn insert_delete_undo_redo_roundtrip() {
        let mut d = doc("hello world");
        type_str(&mut d, 5, "!!!");
        assert_eq!(d.rope().to_string(), "hello!!! world");
        delete(&mut d, 0, 5);
        assert_eq!(d.rope().to_string(), "!!! world");

        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "hello!!! world");
        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "hello world");
        assert!(d.undo().is_none());

        assert!(d.redo().is_some());
        assert_eq!(d.rope().to_string(), "hello!!! world");
        assert!(d.redo().is_some());
        assert_eq!(d.rope().to_string(), "!!! world");
        assert!(d.redo().is_none());
    }

    #[test]
    fn coalescing_digitare_abc_e_un_solo_undo() {
        let mut d = doc("");
        type_str(&mut d, 0, "a");
        type_str(&mut d, 1, "b");
        type_str(&mut d, 2, "c");
        assert_eq!(d.rope().to_string(), "abc");
        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "");
        assert!(d.undo().is_none());
        // la selezione torna a quella precedente al gruppo
        assert_eq!(d.selection(), Range::point(0));
    }

    #[test]
    fn coalescing_interrotto_dal_movimento_cursore() {
        let mut d = doc("");
        type_str(&mut d, 0, "a");
        type_str(&mut d, 1, "b");
        d.set_selection(Range::point(0)); // movimento → chiude il gruppo
        type_str(&mut d, 0, "c");
        assert_eq!(d.rope().to_string(), "cab");
        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "ab");
        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "");
        assert!(d.undo().is_none());
    }

    #[test]
    fn coalescing_interrotto_da_newline_e_delete() {
        let mut d = doc("");
        type_str(&mut d, 0, "a");
        type_str(&mut d, 1, "\n"); // newline: gruppo a sé
        type_str(&mut d, 2, "b");
        assert_eq!(d.rope().to_string(), "a\nb");
        assert!(d.undo().is_some()); // "b"
        assert!(d.undo().is_some()); // "\n"
        assert_eq!(d.rope().to_string(), "a");

        // delete dopo insert: chiude il gruppo
        let mut d = doc("");
        type_str(&mut d, 0, "a");
        type_str(&mut d, 1, "b");
        delete(&mut d, 1, 2); // backspace su 'b'
        type_str(&mut d, 1, "c");
        assert_eq!(d.rope().to_string(), "ac");
        assert!(d.undo().is_some()); // "c"
        assert_eq!(d.rope().to_string(), "a");
        assert!(d.undo().is_some()); // il delete
        assert_eq!(d.rope().to_string(), "ab");
        assert!(d.undo().is_some()); // "ab"
        assert_eq!(d.rope().to_string(), "");
    }

    #[test]
    fn coalescing_interrotto_da_commit_group() {
        let mut d = doc("");
        type_str(&mut d, 0, "a");
        d.commit_undo_group(); // pausa di digitazione
        type_str(&mut d, 1, "b");
        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "a");
        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "");
    }

    #[test]
    fn crlf_normalizzato_e_ripristinato() {
        let originale = "a\r\nb\r\n";
        let mut d = Document::from_str(originale, None, DocId(7));
        assert_eq!(d.rope().to_string(), "a\nb\n");
        assert_eq!(d.line_ending(), LineEnding::CrLf);
        assert_eq!(d.save_payload(), originale.as_bytes());

        // dopo un edit il payload resta CRLF
        type_str(&mut d, 1, "X");
        assert_eq!(d.save_payload(), b"aX\r\nb\r\n");
    }

    #[test]
    fn bom_preservato() {
        let originale = "\u{feff}a\r\nb";
        let d = Document::from_str(originale, None, DocId(8));
        assert!(d.bom());
        assert_eq!(d.rope().to_string(), "a\nb");
        assert_eq!(d.save_payload(), originale.as_bytes());
    }

    #[test]
    fn selezione_rimappata_attraverso_edit() {
        let mut d = doc("hello world");
        d.set_selection(Range::new(6, 11)); // "world"
        // insert prima della selezione, senza selection_after esplicita
        let len = d.rope().len_chars();
        d.apply(Transaction::new(ChangeSet::insert_at(0, "XY", len)));
        assert_eq!(d.selection(), Range::new(8, 13));
        // delete che ingloba l'anchor: collassa sull'inizio del delete
        let len = d.rope().len_chars();
        d.apply(Transaction::new(ChangeSet::delete_range(6, 10, len)));
        assert_eq!(d.selection(), Range::new(6, 9));
    }

    #[test]
    fn is_dirty_robusto_a_undo_oltre_il_save() {
        let mut d = doc("x");
        assert!(!d.is_dirty());
        type_str(&mut d, 1, "a"); // stato A
        d.commit_undo_group();
        d.mark_saved();
        assert!(!d.is_dirty());
        type_str(&mut d, 2, "b"); // stato B
        assert!(d.is_dirty());
        d.undo(); // torna ad A = salvato
        assert!(!d.is_dirty());
        d.undo(); // oltre il save → sporco anche se "più indietro"
        assert!(d.is_dirty());
        d.redo(); // di nuovo A
        assert!(!d.is_dirty());
        d.redo(); // B
        assert!(d.is_dirty());
    }

    #[test]
    fn applied_edit_su_edit_multiriga() {
        // "hello\nworld\n": sostituisce i char 3..8 ("lo\nwo") con "X\nYZ\nQ"
        let mut d = doc("hello\nworld\n");
        let len = d.rope().len_chars();
        let e = d.apply(Transaction::new(ChangeSet::replace_range(
            3, 8, "X\nYZ\nQ", len,
        )));
        assert_eq!(d.rope().to_string(), "helX\nYZ\nQrld\n");
        assert_eq!(e.start_byte, 3);
        assert_eq!(e.old_end_byte, 8);
        assert_eq!(e.new_end_byte, 9);
        assert_eq!(e.start_point, (0, 3));
        assert_eq!(e.old_end_point, (1, 2));
        assert_eq!(e.new_end_point, (2, 1));
        assert_eq!(e.first_changed_line, 0);
        assert_eq!(e.line_delta, 1);
        assert_eq!(e.new_version, 1);
    }

    #[test]
    fn applied_edit_con_multibyte() {
        // "aé\nb": 'é' vale 2 byte → i point della riga 0 sono in byte
        let mut d = doc("a\u{e9}\nb");
        let len = d.rope().len_chars();
        let e = d.apply(Transaction::new(ChangeSet::insert_at(2, "\u{20ac}", len))); // €
        assert_eq!(d.rope().to_string(), "a\u{e9}\u{20ac}\nb");
        assert_eq!(e.start_byte, 3); // 'a'(1) + 'é'(2)
        assert_eq!(e.old_end_byte, 3);
        assert_eq!(e.new_end_byte, 6); // + '€'(3)
        assert_eq!(e.start_point, (0, 3));
        assert_eq!(e.new_end_point, (0, 6));
        assert_eq!(e.line_delta, 0);
    }

    /// LCG deterministico, nessuna dipendenza esterna.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: usize) -> usize {
            if n == 0 { 0 } else { self.next() as usize % n }
        }
    }

    #[test]
    fn property_200_edit_random_poi_undo_completo() {
        let originale = "Lume: nucleo di test.\nSeconda riga àèì.\n";
        let mut d = doc(originale);
        let mut rng = Lcg(0xC0FFEE);

        for i in 0..200 {
            let len = d.rope().len_chars();
            match rng.below(3) {
                0 | 1 if len > 0 => {
                    // delete di un piccolo range
                    let from = rng.below(len);
                    let to = (from + 1 + rng.below(5)).min(len);
                    delete(&mut d, from, to);
                }
                _ => {
                    // insert (a volte singolo char → esercita il coalescing)
                    let pos = rng.below(len + 1);
                    let n = 1 + rng.below(4);
                    let text: String = (0..n)
                        .map(|_| (b'a' + rng.below(26) as u8) as char)
                        .collect();
                    type_str(&mut d, pos, &text);
                }
            }
            if i % 7 == 0 {
                d.commit_undo_group();
            }
        }

        let finale = d.rope().to_string();
        while d.undo().is_some() {}
        assert_eq!(d.rope().to_string(), originale);

        // e il redo completo riporta allo stato finale
        while d.redo().is_some() {}
        assert_eq!(d.rope().to_string(), finale);
    }

    #[test]
    fn reload_preserva_history_e_identita() {
        let mut d = doc("uno\ndue\n");
        type_str(&mut d, 0, "X"); // edit locale, poi "salvato"
        d.commit_undo_group();
        d.mark_saved();
        assert!(!d.is_dirty());

        // il file cambia su disco (con CRLF: il reload aggiorna i metadati)
        d.reload_from_str("alfa\r\nbeta\r\n");
        assert_eq!(d.rope().to_string(), "alfa\nbeta\n");
        assert_eq!(d.line_ending(), LineEnding::CrLf);
        assert!(
            !d.is_dirty(),
            "dopo il reload il contenuto è quello su disco"
        );

        // l'undo del reload ripristina il contenuto precedente e sporca
        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "Xuno\ndue\n");
        assert!(d.is_dirty());

        // e prima ancora c'è l'edit pre-reload
        assert!(d.undo().is_some());
        assert_eq!(d.rope().to_string(), "uno\ndue\n");
    }

    #[test]
    fn version_monotona() {
        let mut d = doc("");
        let e1 = type_str(&mut d, 0, "a");
        let e2 = type_str(&mut d, 1, "b");
        assert!(e2.new_version > e1.new_version);
        d.undo();
        assert!(d.version() > e2.new_version);
    }
}
