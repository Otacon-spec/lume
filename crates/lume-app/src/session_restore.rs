use std::collections::HashMap;
use std::path::PathBuf;

use lume_core::DocId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingRestoredTab {
    pub(crate) path: PathBuf,
    pub(crate) failure: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct PendingRestoredTabs {
    by_doc: HashMap<DocId, PendingRestoredTab>,
}

impl PendingRestoredTabs {
    pub(crate) fn insert(&mut self, id: DocId, path: PathBuf) {
        self.by_doc.insert(
            id,
            PendingRestoredTab {
                path,
                failure: None,
            },
        );
    }

    pub(crate) fn get(&self, id: DocId) -> Option<&PendingRestoredTab> {
        self.by_doc.get(&id)
    }

    pub(crate) fn contains(&self, id: DocId) -> bool {
        self.by_doc.contains_key(&id)
    }

    pub(crate) fn set_failure(&mut self, id: DocId, failure: String) {
        if let Some(tab) = self.by_doc.get_mut(&id) {
            tab.failure = Some(failure);
        }
    }

    pub(crate) fn take(&mut self, id: DocId) -> Option<PendingRestoredTab> {
        self.by_doc.remove(&id)
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.by_doc.len()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn solo_la_scheda_attiva_esce_dalla_coda() {
        let mut pending = PendingRestoredTabs::default();
        for n in 1..=10 {
            pending.insert(DocId(n), PathBuf::from(format!("file-{n:02}.rs")));
        }

        assert_eq!(pending.pending_len(), 10);
        assert_eq!(
            pending.get(DocId(1)).map(|tab| tab.path.as_path()),
            Some(std::path::Path::new("file-01.rs"))
        );
        assert!(pending.take(DocId(1)).is_some());
        assert_eq!(pending.pending_len(), 9);
        assert!(pending.take(DocId(1)).is_none(), "attivazione ripetuta");
        assert!(pending.take(DocId(2)).is_some());
        assert_eq!(pending.pending_len(), 8);
    }

    #[test]
    fn un_errore_resta_sulla_scheda_finche_non_viene_rimossa() {
        let mut pending = PendingRestoredTabs::default();
        pending.insert(DocId(7), PathBuf::from("missing.rs"));
        pending.set_failure(DocId(7), "file assente".to_owned());

        assert_eq!(
            pending.get(DocId(7)).unwrap().failure.as_deref(),
            Some("file assente")
        );
        assert!(pending.take(DocId(7)).is_some());
        assert_eq!(pending.pending_len(), 0);
    }
}
