//! Raccolta finale dei worker JJ ammessi, posseduta dall'app.
//!
//! `admission.rs` e `load_admission.rs` conservano il JoinHandle del worker
//! dentro la future condivisa dell'invocazione. Quando l'app lascia cadere
//! Pending, producer e ogni Collector, nel percorso ordinario di uscita
//! resterebbe almeno un owner: questo registro. Il collector conserva la
//! stessa future condivisa, quindi lo stesso JoinHandle, e l'uscita lo attende
//! fuori dal thread UI dentro `spawn_blocking`.
//!
//! - Registrazione solo dopo `spawn_blocking` riuscito, prima del wake.
//! - Nessuna attesa, join o nuovo thread nel Drop di Pending/Permit/Collector.
//! - Nessun lavoro da inattivo: gli entry già racchiusi vengono scartati solo
//!   quando si registra un nuovo worker o si interroga l'uscita.
//! - Una raccolta già conclusa non viene riattesa: `collected()` è pubblicato
//!   dopo l'await autentico del JoinHandle.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use super::{admission, load_admission};

/// Worker registrati dall'app: uno per invocazione ammessa.
pub(crate) struct FinalCollection {
    retained: Mutex<Vec<Retained>>,
    joined: AtomicUsize,
}

/// Il ticket di un worker ancora vivo; la variante distingue il tipo di
/// Collector senza duplicare i due registri.
#[derive(Clone)]
pub(crate) enum Retained {
    Load(load_admission::Collector),
    Action(admission::Collector),
}

impl Retained {
    async fn collect(&self) {
        match self {
            Self::Load(collector) => collector.collect().await,
            Self::Action(collector) => collector.collect().await,
        }
    }

    fn collected(&self) -> bool {
        match self {
            Self::Load(collector) => collector.collected(),
            Self::Action(collector) => collector.collected(),
        }
    }
}

impl FinalCollection {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            retained: Mutex::new(Vec::new()),
            joined: AtomicUsize::new(0),
        })
    }

    pub(crate) fn retain_load(&self, collector: load_admission::Collector) {
        self.retain(Retained::Load(collector));
    }

    pub(crate) fn retain_action(&self, collector: admission::Collector) {
        self.retain(Retained::Action(collector));
    }

    fn retain(&self, entry: Retained) {
        let mut entries = self
            .retained
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Solo ticket già chiusi: nessuna attesa qui, né lavoro da inattivo.
        entries.retain(|entry| !entry.collected());
        entries.push(entry);
    }

    /// Vero se almeno un worker ammesso non è ancora stato raccolto.
    pub(crate) fn has_pending(&self) -> bool {
        let mut entries = self
            .retained
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|entry| !entry.collected());
        !entries.is_empty()
    }

    /// Attende i worker registrati. Da chiamare su un thread non-UI
    /// (`spawn_blocking`): `block_on` resta qui, mai nel thread dell'interfaccia.
    /// Restituisce falso se una raccolta non arriva a `collected()`.
    pub(crate) fn collect_pending(&self) -> bool {
        let entries = {
            let mut retained = self
                .retained
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            retained.retain(|entry| !entry.collected());
            retained.clone()
        };
        if entries.is_empty() {
            return true;
        }
        let mut success = true;
        for entry in &entries {
            futures::executor::block_on(entry.collect());
            success &= entry.collected();
        }
        if success {
            self.joined.fetch_add(entries.len(), Ordering::AcqRel);
        }
        let mut retained = self
            .retained
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        retained.retain(|entry| !entry.collected());
        success
    }

    /// Worker effettivamente attesi fino alla completion, per la prova nativa.
    #[cfg(test)]
    pub(crate) fn joined(&self) -> usize {
        self.joined.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::jj_actions::load_admission::{Pending, Permit};
    use std::time::Duration;

    fn ticket(
        collection: &Arc<FinalCollection>,
        started: &std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> (Pending, Permit) {
        let (pending, permit) = Pending::with_final(7, collection.clone());
        assert!(permit.admit());
        let started = started.clone();
        let handle = permit
            .start(move || {
                let _ = started.send(());
                release
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|_| "barriera finale scaduta".to_owned())?;
                Ok(lume_jj::JjSnapshot {
                    entries: Vec::new(),
                    status: "worker finale".to_owned(),
                })
            })
            .unwrap();
        drop(handle);
        (pending, permit)
    }

    #[test]
    fn ultimo_owner_lasciato_il_worker_resta_nel_registro_dell_app()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let collection = FinalCollection::new();
        let (started, ready) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let (pending, permit) = {
            let _context = runtime.enter();
            ticket(&collection, &started, wait)
        };
        ready.recv_timeout(Duration::from_secs(10))?;
        // Ultimo owner: nessun Pending/Permit/Handle resta in mano al test.
        drop(pending);
        drop(permit);
        assert!(collection.has_pending());
        runtime.shutdown_timeout(Duration::ZERO);
        // Il registro conserva lo stesso JoinHandle anche senza runtime.
        assert!(collection.has_pending());
        release.send(())?;
        assert!(collection.collect_pending());
        assert_eq!(collection.joined(), 1);
        assert!(!collection.has_pending());
        // Un secondo giro non riattende né riconta il worker concluso.
        assert!(collection.collect_pending());
        assert_eq!(collection.joined(), 1);
        Ok(())
    }

    #[test]
    fn raccolta_vuota_non_avvia_attese() {
        let collection = FinalCollection::new();
        assert!(!collection.has_pending());
        assert!(collection.collect_pending());
        assert_eq!(collection.joined(), 0);
    }
}
