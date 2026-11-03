//! Load JJ: il permit revoca soltanto il cold work; dopo ammissione il ticket
//! app conserva un owner condiviso del singolo worker e della sua risposta.
use futures::{
    FutureExt, Stream,
    future::{BoxFuture, Shared},
};
use lume_jj::JjSnapshot;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};
use std::{
    fmt,
    hash::{Hash, Hasher},
};

use super::final_collection::FinalCollection;

const PENDING: u8 = 0;
const ADMITTED: u8 = 1;
const ABANDONED: u8 = 2;
const COMPLETED: u8 = 3;
type Outcome = Result<JjSnapshot, String>;
type Completion = Arc<Mutex<Option<Outcome>>>;
type Handle = Shared<BoxFuture<'static, Outcome>>;
type Execution = Arc<Mutex<Option<Handle>>>;

pub(crate) struct Pending {
    generation: u64,
    state: Arc<AtomicU8>,
    completion: Completion,
    execution: Execution,
    wake: Arc<tokio::sync::Notify>,
}
// Unico permit non clonabile, creato prima della future per rendere Drop cold.
pub(crate) struct Permit {
    generation: u64,
    state: Arc<AtomicU8>,
    completion: Completion,
    execution: Execution,
    wake: Arc<tokio::sync::Notify>,
    final_collection: Option<Arc<FinalCollection>>,
}
#[derive(Clone)]
pub(crate) struct Collector {
    generation: u64,
    state: Arc<AtomicU8>,
    completion: Completion,
    execution: Execution,
    wake: Arc<tokio::sync::Notify>,
}
impl fmt::Debug for Collector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JjLoadCollector { .. }")
    }
}
impl PartialEq for Collector {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
            && Arc::ptr_eq(&self.state, &other.state)
            && Arc::ptr_eq(&self.completion, &other.completion)
            && Arc::ptr_eq(&self.execution, &other.execution)
    }
}
impl Eq for Collector {}
impl Hash for Collector {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.generation.hash(state);
        Arc::as_ptr(&self.state).hash(state);
        Arc::as_ptr(&self.completion).hash(state);
        Arc::as_ptr(&self.execution).hash(state);
    }
}

fn publish(state: &AtomicU8, completion: &Mutex<Option<Outcome>>, result: Outcome) {
    if state
        .compare_exchange(ADMITTED, COMPLETED, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        *completion.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
    }
}

impl Pending {
    #[cfg(test)]
    pub(crate) fn new(generation: u64) -> (Self, Permit) {
        Self::build(generation, None)
    }
    /// Ticket dell'app: dopo lo spawn il worker entra anche nella raccolta
    /// finale posseduta dall'app, che l'uscita attende fuori dal thread UI.
    pub(crate) fn with_final(
        generation: u64,
        final_collection: Arc<FinalCollection>,
    ) -> (Self, Permit) {
        Self::build(generation, Some(final_collection))
    }
    fn build(generation: u64, final_collection: Option<Arc<FinalCollection>>) -> (Self, Permit) {
        let state = Arc::new(AtomicU8::new(PENDING));
        let completion = Arc::new(Mutex::new(None));
        let execution = Arc::new(Mutex::new(None));
        let wake = Arc::new(tokio::sync::Notify::new());
        (
            Self {
                generation,
                state: state.clone(),
                completion: completion.clone(),
                execution: execution.clone(),
                wake: wake.clone(),
            },
            Permit {
                generation,
                state,
                completion,
                execution,
                wake,
                final_collection,
            },
        )
    }
    pub(crate) fn abandoned(&self) -> bool {
        self.state.load(Ordering::Acquire) == ABANDONED
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    /// Solo un load non avviato è revocabile al cambio workspace.
    pub(crate) fn revoke(&self) -> bool {
        match self
            .state
            .compare_exchange(PENDING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                self.wake.notify_waiters();
                true
            }
            Err(ABANDONED) => true,
            Err(_) => false,
        }
    }
    #[cfg(test)]
    pub(crate) fn inspect_completion(&self, inspect: impl FnOnce(Option<&Outcome>)) {
        let completion = self.completion.lock().unwrap_or_else(|e| e.into_inner());
        inspect(completion.as_ref());
    }
    pub(crate) fn take_completion(&self) -> Option<Outcome> {
        self.completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
    pub(crate) fn collector(&self) -> Collector {
        Collector {
            generation: self.generation,
            state: self.state.clone(),
            completion: self.completion.clone(),
            execution: self.execution.clone(),
            wake: self.wake.clone(),
        }
    }
    pub(crate) fn matches(&self, signal: &Collector) -> bool {
        Arc::ptr_eq(&self.state, &signal.state)
            && Arc::ptr_eq(&self.completion, &signal.completion)
            && Arc::ptr_eq(&self.execution, &signal.execution)
    }
}
impl Permit {
    pub(crate) fn admit(&self) -> bool {
        self.state
            .compare_exchange(PENDING, ADMITTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    /// Stesso ticket condiviso, senza un nuovo worker: serve al registro
    /// finale dell'app per conservare il JoinHandle anche senza Pending.
    fn retained(&self) -> Collector {
        Collector {
            generation: self.generation,
            state: self.state.clone(),
            completion: self.completion.clone(),
            execution: self.execution.clone(),
            wake: self.wake.clone(),
        }
    }
    pub(crate) fn start(&self, work: impl FnOnce() -> Outcome + Send + 'static) -> Option<Handle> {
        if self.state.load(Ordering::Acquire) != ADMITTED {
            return None;
        }
        let handle = {
            let mut slot = self.execution.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(handle) = &*slot {
                return Some(handle.clone());
            }
            // Runtime catturato prima della Shared: producer e ticket app
            // attendono lo stesso JoinHandle, senza riavviare il loader.
            let worker = tokio::runtime::Handle::try_current().map(|rt| rt.spawn_blocking(work));
            let state = self.state.clone();
            let completion = self.completion.clone();
            let handle = async move {
                let result = match worker {
                    Ok(worker) => worker.await.unwrap_or_else(|error| Err(error.to_string())),
                    Err(_) => Err("Runtime Jujutsu non disponibile: nessun load avviato".into()),
                };
                publish(&state, &completion, result.clone());
                result
            }
            .boxed()
            .shared();
            *slot = Some(handle.clone());
            handle
        };
        // Registrazione dopo lo slot e prima del wake: l'uscita trova sempre
        // il ticket, e `collected()` resta pubblicato dopo l'await del worker.
        if let Some(collection) = &self.final_collection {
            collection.retain_load(self.retained());
        }
        self.wake.notify_waiters();
        Some(handle)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if self
            .state
            .compare_exchange(PENDING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.wake.notify_waiters();
        }
    }
}
impl Collector {
    pub(crate) fn collected(&self) -> bool {
        matches!(self.state.load(Ordering::Acquire), COMPLETED | ABANDONED)
    }
    pub(crate) async fn collect(&self) {
        loop {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let handle = self
                .execution
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(handle) = handle {
                let _ = handle.await;
                return;
            }
            if self.state.load(Ordering::Acquire) == ABANDONED {
                return;
            }
            notified.await;
        }
    }
}
pub(crate) fn completed(collector: &Collector) -> impl Stream<Item = Collector> + Send + use<> {
    let collector = collector.clone();
    futures::stream::once(async move {
        collector.collect().await;
        collector
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::{sync::atomic::AtomicUsize, time::Duration};

    #[test]
    fn load_cold_drop_revoca_ma_admitted_drop_non_raccoglie() {
        let (pending, permit) = Pending::new(0);
        drop(permit);
        assert!(pending.abandoned());
        let (pending, permit) = Pending::new(0);
        assert!(permit.admit());
        drop(permit);
        assert!(!pending.abandoned());
        assert!(!pending.revoke());
    }
    #[test]
    fn revoca_esplicita_pending_non_permette_late_admission() {
        let (pending, permit) = Pending::new(0);
        assert!(pending.revoke());
        assert!(pending.revoke());
        assert!(!permit.admit());
        assert!(pending.abandoned());
    }
    #[test]
    fn permit_conservato_non_revoca_e_non_ammette_due_volte() {
        let (pending, permit) = Pending::new(0);
        assert!(!pending.abandoned());
        assert!(permit.admit());
        assert!(!permit.admit());
    }
    #[tokio::test]
    async fn collector_conserva_il_load_ammesso_se_la_task_originale_scompare()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (pending, permit) = Pending::new(0);
        assert!(permit.admit());
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let producer = permit
            .start(move || {
                worker_calls.fetch_add(1, Ordering::AcqRel);
                let _ = started_tx.send(());
                release_rx
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|_| "barriera del load scaduta".to_owned())?;
                Ok(JjSnapshot {
                    entries: Vec::new(),
                    status: "caricamento autentico".to_owned(),
                })
            })
            .ok_or("worker ammesso assente")?;
        started_rx.await?;
        drop(producer);
        drop(permit);

        let collector = pending.collector();
        let collected =
            tokio::spawn(async move { completed(&collector).collect::<Vec<_>>().await });
        release_tx.send(())?;
        let signals = collected.await?;
        assert_eq!(signals.len(), 1);
        assert_eq!(calls.load(Ordering::Acquire), 1);
        let result = pending.take_completion().ok_or("esito non conservato")??;
        assert_eq!(result.status, "caricamento autentico");
        assert!(pending.take_completion().is_none());
        Ok(())
    }
    #[test]
    fn collector_raccoglie_worker_ammesso_dopo_shutdown_runtime()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let (pending, permit) = Pending::new(0);
        assert!(permit.admit());
        let (started, ready) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let producer = {
            let _context = runtime.enter();
            permit
                .start(move || {
                    let _ = started.send(());
                    wait.recv_timeout(std::time::Duration::from_secs(10))
                        .map_err(|_| "barriera shutdown scaduta".to_owned())?;
                    Ok(JjSnapshot {
                        entries: Vec::new(),
                        status: "esito dopo shutdown".to_owned(),
                    })
                })
                .ok_or("worker non ammesso")?
        };
        ready.recv_timeout(std::time::Duration::from_secs(10))?;
        drop(producer);
        drop(permit);
        let collector = pending.collector();
        // Il worker è ancora bloccato: shutdown non può raccoglierlo.
        runtime.shutdown_timeout(std::time::Duration::ZERO);
        assert!(!collector.collected());
        assert!(pending.take_completion().is_none());
        assert!(collector.collect().now_or_never().is_none());
        release.send(())?;
        // Nessun runtime Tokio attivo: si attende lo stesso JoinHandle.
        futures::executor::block_on(collector.collect());
        assert!(collector.collected());
        let result = pending.take_completion().ok_or("esito perso")??;
        assert_eq!(result.status, "esito dopo shutdown");
        assert!(pending.take_completion().is_none());
        Ok(())
    }
}
