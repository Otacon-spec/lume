//! Registro dei documenti aperti, con deduplicazione per path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::document::{DocId, Document};

/// Budget complessivo degli storici: un terzo dei 150 MiB di private bytes.
pub const WORKSPACE_HISTORY_BUDGET: usize = 48 * 1024 * 1024;

/// Insieme dei documenti aperti nell'editor.
#[derive(Debug, Default)]
pub struct Workspace {
    docs: HashMap<DocId, Document>,
    by_path: HashMap<PathBuf, DocId>,
    next_id: u64,
}

/// Rifiuto del cambio di legame nel modello; nessun effetto sul filesystem.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RebindError {
    #[error("il documento non è aperto")]
    UnknownDoc,
    #[error("il file è già aperto in un'altra scheda")]
    PathTaken(DocId),
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apre un documento. Se `path` è già aperto ritorna l'id esistente
    /// (dedup); `path == None` crea sempre un buffer "untitled" nuovo.
    pub fn open(&mut self, path: Option<PathBuf>, text: &str) -> DocId {
        if let Some(p) = &path
            && let Some(id) = self.by_path.get(p)
        {
            return *id;
        }
        self.open_prepared(path, crate::PreparedText::prepare(text))
    }

    /// Inserisce il contenuto già preparato dal worker, mantenendo il dedup.
    pub fn open_prepared(&mut self, path: Option<PathBuf>, text: crate::PreparedText) -> DocId {
        if let Some(p) = &path
            && let Some(id) = self.by_path.get(p)
        {
            return *id;
        }
        let id = DocId(self.next_id);
        self.next_id += 1;
        if let Some(p) = &path {
            self.by_path.insert(p.clone(), id);
        }
        self.docs
            .insert(id, Document::from_prepared(text, path, id));
        id
    }

    pub fn get(&self, id: DocId) -> Option<&Document> {
        self.docs.get(&id)
    }

    pub fn get_mut(&mut self, id: DocId) -> Option<&mut Document> {
        self.docs.get_mut(&id)
    }

    /// Id del documento aperto per il path dato, se presente.
    pub fn get_by_path(&self, path: &Path) -> Option<DocId> {
        self.by_path.get(path).copied()
    }

    /// Aggiorna documento e indice insieme. Il proprio path è un no-op;
    /// quello di un altro documento è rifiutato prima di qualsiasi mutazione.
    /// Confronto dei Path del modello, non identità filesystem/canonicalizzazione
    /// o autorizzazione a scrivere. Il consumer deve prima completare il writer.
    pub fn rebind_path(&mut self, id: DocId, path: PathBuf) -> Result<(), RebindError> {
        if let Some(&other) = self.by_path.get(&path) {
            return if other == id {
                Ok(())
            } else {
                Err(RebindError::PathTaken(other))
            };
        }
        let Some(doc) = self.docs.get_mut(&id) else {
            return Err(RebindError::UnknownDoc);
        };
        if let Some(old) = doc.path() {
            self.by_path.remove(old);
        }
        doc.set_path(path.clone());
        self.by_path.insert(path, id);
        Ok(())
    }

    /// Chiude un documento e lo ritorna (per eventuale prompt di save).
    pub fn close(&mut self, id: DocId) -> Option<Document> {
        let doc = self.docs.remove(&id)?;
        if let Some(p) = doc.path() {
            self.by_path.remove(p);
        }
        Some(doc)
    }

    pub fn ids(&self) -> impl Iterator<Item = DocId> + '_ {
        self.docs.keys().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Document> {
        self.docs.values()
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Riduce gli storici fino al budget globale. Ogni documento conserva
    /// comunque la sua ultima operazione undo; una singola revisione enorme è
    /// dunque un'eccezione esplicita al tetto, non viene spezzata.
    pub fn trim_histories_to_budget(&mut self) -> bool {
        self.trim_histories_to(WORKSPACE_HISTORY_BUDGET)
    }

    /// Riduce gli storici fino al tetto dato, espellendo la revisione globale
    /// più vecchia e conservando l'ultima operazione di ogni documento.
    pub fn trim_histories_to(&mut self, budget: usize) -> bool {
        let mut trimmed = false;
        while self.history_bytes() > budget {
            let Some(id) = self
                .docs
                .iter()
                .filter_map(|(id, document)| {
                    document
                        .oldest_evictable_history_sequence()
                        .map(|sequence| (*id, sequence))
                })
                .min_by_key(|(_, sequence)| *sequence)
                .map(|(id, _)| id)
            else {
                break;
            };
            let removed = self
                .docs
                .get_mut(&id)
                .is_some_and(Document::discard_oldest_history_revision);
            debug_assert!(removed);
            trimmed = true;
        }
        trimmed
    }

    pub fn history_bytes(&self) -> usize {
        self.docs.values().map(Document::history_bytes).sum()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn prepared_open_keeps_dedup_and_existing_buffer() {
        let mut workspace = Workspace::new();
        let path = PathBuf::from("preparato.txt");
        let id = workspace.open_prepared(
            Some(path.clone()),
            crate::PreparedText::prepare("\u{feff}λ\r\n"),
        );
        let doc = workspace.get(id).unwrap();
        assert_eq!(doc.save_payload(), "\u{feff}λ\r\n".as_bytes());
        let same = workspace.open_prepared(Some(path), crate::PreparedText::prepare("diverso"));
        assert_eq!(same, id);
        assert_eq!(workspace.get(id).unwrap().rope().to_string(), "λ\n");
    }
    use crate::document::DOCUMENT_HISTORY_BUDGET;
    use crate::history::{ChangeSet, Transaction};

    const BUDGET_PRIVATE_BYTES: usize = 150 * 1024 * 1024;

    fn documento_con_storico(
        ws: &mut Workspace,
        nome: &str,
        blocchi: usize,
        lunghezza: usize,
    ) -> DocId {
        let id = ws.open(Some(PathBuf::from(nome)), &"x".repeat(blocchi * lunghezza));
        for _ in 0..blocchi {
            let doc = ws.get_mut(id).unwrap();
            let len = doc.rope().len_chars();
            doc.apply(Transaction::new(ChangeSet::delete_range(0, lunghezza, len)));
        }
        id
    }

    #[test]
    fn i_tetti_dell_undo_stanno_nel_budget_di_private_bytes() {
        const {
            assert!(WORKSPACE_HISTORY_BUDGET * 3 <= BUDGET_PRIVATE_BYTES);
            assert!(DOCUMENT_HISTORY_BUDGET * 2 <= WORKSPACE_HISTORY_BUDGET);
        }
    }

    #[test]
    fn sotto_il_tetto_non_si_toglie_niente() {
        let mut ws = Workspace::new();
        let a = documento_con_storico(&mut ws, "a.txt", 4, 10_000);
        let prima = ws.get(a).unwrap().history_bytes();
        assert!(!ws.trim_histories_to_budget());
        assert!(!ws.trim_histories_to(prima));
        assert_eq!(ws.get(a).unwrap().history_bytes(), prima);
    }

    #[test]
    fn il_tetto_globale_espelle_prima_le_revisioni_piu_vecchie() {
        let mut ws = Workspace::new();
        let a = documento_con_storico(&mut ws, "a.txt", 4, 10_000);
        let b = documento_con_storico(&mut ws, "b.txt", 4, 10_000);
        let a_prima = ws.get(a).unwrap().history_bytes();
        let b_prima = ws.get(b).unwrap().history_bytes();
        let unita = a_prima / 4;
        assert!(
            unita >= 10_000,
            "premessa: ogni revisione trattiene almeno il testo cancellato"
        );
        let tetto = a_prima + b_prima - 3 * unita + unita / 2;

        assert!(ws.trim_histories_to(tetto));

        let a_dopo = ws.get(a).unwrap().history_bytes();
        let b_dopo = ws.get(b).unwrap().history_bytes();
        assert!(ws.history_bytes() <= tetto);
        assert_eq!(
            b_dopo, b_prima,
            "il documento editato per ultimo non perde niente"
        );
        assert!(a_dopo < b_dopo, "il più vecchio perde per primo");
        assert!(a_dopo >= unita / 2, "l'ultima operazione resta");
    }

    #[test]
    fn ogni_documento_conserva_l_ultima_operazione() {
        let mut ws = Workspace::new();
        let a = documento_con_storico(&mut ws, "a.txt", 3, 10_000);
        let b = documento_con_storico(&mut ws, "b.txt", 3, 10_000);
        let unita = ws.get(a).unwrap().history_bytes() / 3;

        assert!(ws.trim_histories_to(1));

        for id in [a, b] {
            let bytes = ws.get(id).unwrap().history_bytes();
            assert!(
                (unita / 2..unita * 2).contains(&bytes),
                "una revisione sola: {bytes} B (unità {unita} B)"
            );
        }
        assert!(
            !ws.trim_histories_to(1),
            "niente più da espellere: nessun ciclo infinito"
        );
    }

    #[test]
    fn open_dedup_per_path() {
        let mut ws = Workspace::new();
        let p = PathBuf::from("src/main.rs");
        let id1 = ws.open(Some(p.clone()), "fn main() {}");
        let id2 = ws.open(Some(p.clone()), "contenuto ignorato");
        assert_eq!(id1, id2);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws.get_by_path(&p), Some(id1));
        // il contenuto resta quello della prima apertura
        assert_eq!(ws.get(id1).unwrap().rope().to_string(), "fn main() {}");
    }

    #[test]
    fn untitled_sempre_nuovo() {
        let mut ws = Workspace::new();
        let a = ws.open(None, "");
        let b = ws.open(None, "");
        assert_ne!(a, b);
        assert_eq!(ws.len(), 2);
    }

    #[test]
    fn close_rimuove_anche_il_path() {
        let mut ws = Workspace::new();
        let p = PathBuf::from("a.txt");
        let id = ws.open(Some(p.clone()), "x");
        let doc = ws.close(id).unwrap();
        assert_eq!(doc.rope().to_string(), "x");
        assert!(ws.get(id).is_none());
        assert_eq!(ws.get_by_path(&p), None);
        // riaprire dopo la chiusura crea un id nuovo
        let id2 = ws.open(Some(p), "y");
        assert_ne!(id, id2);
    }

    #[test]
    fn rebind_sposta_il_path_e_l_indice() {
        let mut ws = Workspace::new();
        let vecchio = PathBuf::from("a.txt");
        let nuovo = PathBuf::from("b.txt");
        let id = ws.open(Some(vecchio.clone()), "x");

        ws.rebind_path(id, nuovo.clone()).unwrap();

        assert_eq!(ws.get(id).unwrap().path(), Some(nuovo.as_path()));
        assert_eq!(ws.get_by_path(&nuovo), Some(id));
        assert_eq!(
            ws.get_by_path(&vecchio),
            None,
            "il nome vecchio non punta più a niente"
        );
        // e il vecchio nome è di nuovo libero
        let altro = ws.open(Some(vecchio), "y");
        assert_ne!(altro, id);
    }

    #[test]
    fn rebind_da_untitled_rende_il_documento_raggiungibile_per_path() {
        let mut ws = Workspace::new();
        let id = ws.open(None, "x");
        let p = PathBuf::from("nuovo.txt");

        ws.rebind_path(id, p.clone()).unwrap();

        assert_eq!(ws.get_by_path(&p), Some(id));
        assert_eq!(
            ws.open(Some(p.clone()), "ignorato"),
            id,
            "la dedup ora lo vede"
        );
        // chiudere toglie anche il nome nuovo
        ws.close(id).unwrap();
        assert_eq!(ws.get_by_path(&p), None);
    }

    #[test]
    fn rebind_su_un_path_di_un_altro_documento_e_rifiutato_e_non_cambia_niente() {
        let mut ws = Workspace::new();
        let occupato = PathBuf::from("a.txt");
        let a = ws.open(Some(occupato.clone()), "a");
        let senza = ws.open(None, "b");

        assert_eq!(
            ws.rebind_path(senza, occupato.clone()),
            Err(RebindError::PathTaken(a))
        );

        assert!(ws.get(senza).unwrap().path().is_none(), "resta senza nome");
        assert_eq!(
            ws.get_by_path(&occupato),
            Some(a),
            "e il proprietario non cambia"
        );
    }

    #[test]
    fn rebind_sul_proprio_path_non_fa_niente_e_riesce() {
        let mut ws = Workspace::new();
        let p = PathBuf::from("a.txt");
        let id = ws.open(Some(p.clone()), "x");

        assert_eq!(ws.rebind_path(id, p.clone()), Ok(()));

        assert_eq!(ws.get_by_path(&p), Some(id));
        assert_eq!(ws.len(), 1);
    }

    #[test]
    fn rebind_di_un_id_sconosciuto_e_un_errore() {
        let mut ws = Workspace::new();
        assert_eq!(
            ws.rebind_path(DocId(42), PathBuf::from("a.txt")),
            Err(RebindError::UnknownDoc)
        );
        assert_eq!(ws.get_by_path(std::path::Path::new("a.txt")), None);
    }
}
