//! Permit unico revoca solo la Task cold. Dopo ammissione l'app conserva
//! l'attesa condivisa del worker, distinta dalla Task originale di consegna.
//! Non copre lastOwner/main-close/runtime loss o tutti i thread/descendenti.
use futures::{
    FutureExt, Stream,
    future::{BoxFuture, Shared},
};
use lume_ui::tool::JjAction;
use std::{
    fmt,
    hash::{Hash, Hasher},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

use super::final_collection::FinalCollection;
const PENDING: u8 = 0;
const ADMITTED: u8 = 1;
const ABANDONED: u8 = 2;
const COMPLETED: u8 = 3;
type Outcome = Result<String, String>;
type Completion = Arc<Mutex<Option<Outcome>>>;
type Handle = Shared<BoxFuture<'static, Outcome>>;
type Execution = Arc<Mutex<Option<Handle>>>;

#[cfg_attr(test, derive(Clone))]
pub(crate) struct Pending {
    state: Arc<AtomicU8>,
    action: JjAction,
    completion: Completion,
    execution: Execution,
    wake: Arc<tokio::sync::Notify>,
}
// Unico permit non clonabile, costruito prima della future (cold Drop incluso).
pub(crate) struct Permit {
    state: Arc<AtomicU8>,
    completion: Completion,
    execution: Execution,
    wake: Arc<tokio::sync::Notify>,
    final_collection: Option<Arc<FinalCollection>>,
}
// Segnale/ricetta di raccolta, non autorizzazione ad avviare un'azione.
#[derive(Clone)]
pub(crate) struct Collector {
    state: Arc<AtomicU8>,
    completion: Completion,
    execution: Execution,
    wake: Arc<tokio::sync::Notify>,
}
impl fmt::Debug for Collector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JjCollector { .. }")
    }
}
impl PartialEq for Collector {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
            && Arc::ptr_eq(&self.completion, &other.completion)
            && Arc::ptr_eq(&self.execution, &other.execution)
    }
}
impl Eq for Collector {}
impl Hash for Collector {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Arc::as_ptr(&self.state).hash(h);
        Arc::as_ptr(&self.completion).hash(h);
        Arc::as_ptr(&self.execution).hash(h);
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
    pub(crate) fn new(action: JjAction) -> (Self, Permit) {
        Self::build(action, None)
    }
    /// Ticket dell'app: dopo lo spawn il worker entra anche nella raccolta
    /// finale posseduta dall'app, che l'uscita attende fuori dal thread UI.
    pub(crate) fn with_final(
        action: JjAction,
        final_collection: Arc<FinalCollection>,
    ) -> (Self, Permit) {
        Self::build(action, Some(final_collection))
    }
    fn build(action: JjAction, final_collection: Option<Arc<FinalCollection>>) -> (Self, Permit) {
        let state = Arc::new(AtomicU8::new(PENDING));
        let completion = Arc::new(Mutex::new(None));
        let execution = Arc::new(Mutex::new(None));
        let wake = Arc::new(tokio::sync::Notify::new());
        (
            Self {
                state: state.clone(),
                action,
                completion: completion.clone(),
                execution: execution.clone(),
                wake: wake.clone(),
            },
            Permit {
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
    /// Solo Pending revocabile; admitted/completed restano fino alla raccolta.
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
    pub(crate) fn simulate_admitted(&self) {
        let _ = self
            .state
            .compare_exchange(PENDING, ADMITTED, Ordering::AcqRel, Ordering::Acquire);
    }
    #[cfg(all(test, target_os = "linux"))]
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
    pub(crate) fn into_action(self) -> JjAction {
        self.action
    }
    pub(crate) fn collector(&self) -> Collector {
        Collector {
            state: self.state.clone(),
            completion: self.completion.clone(),
            execution: self.execution.clone(),
            wake: self.wake.clone(),
        }
    }
    pub(crate) fn matches(&self, signal: &Collector) -> bool {
        self.collector() == *signal
    }
}
impl Permit {
    #[cfg(test)]
    fn complete(&self, result: Outcome) {
        publish(&self.state, &self.completion, result);
    }
    pub(crate) fn admit(&self) -> bool {
        self.state
            .compare_exchange(PENDING, ADMITTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    /// Stesso ticket condiviso, senza un nuovo worker: serve al registro
    /// finale dell'app per conservare il JoinHandle anche senza Pending.
    fn retained(&self) -> Collector {
        Collector {
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
            // Runtime catturato prima della Shared: il fallback attende il
            // vero JoinHandle anche fuori dal contesto del producer originale.
            let worker = tokio::runtime::Handle::try_current().map(|rt| rt.spawn_blocking(work));
            let state = self.state.clone();
            let completion = self.completion.clone();
            let handle = async move {
                let result = match worker {
                    Ok(worker) => worker.await.unwrap_or_else(|e| Err(e.to_string())),
                    Err(_) => Err("Runtime Jujutsu non disponibile: nessuna azione avviata".into()),
                };
                // NON catturare execution/wake/Collector: niente ciclo Arc.
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
            collection.retain_action(self.retained());
        }
        // Notify dopo la pubblicazione dello slot e fuori dal mutex.
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
    #[test]
    fn esito_completato_conservato_dopo_drop_e_consumato_una_volta() {
        let (pending, permit) = Pending::new(JjAction::New(None));
        assert!(permit.admit());
        permit.complete(Ok("esito reale del worker".into()));
        permit.complete(Err("seconda pubblicazione vietata".into()));
        drop(permit);
        assert_eq!(
            pending.take_completion(),
            Some(Ok("esito reale del worker".into()))
        );
        assert_eq!(pending.take_completion(), None);
    }
    #[test]
    fn nessuna_pubblicazione_prima_dell_ammissione() {
        let (pending, permit) = Pending::new(JjAction::New(None));
        permit.complete(Ok("non ammesso".into()));
        assert_eq!(pending.take_completion(), None);
        drop(permit);
        assert!(pending.abandoned());
    }
    #[test]
    fn permit_cold_drop_revoca_solo_pending() {
        let (pending, permit) = Pending::new(JjAction::New(None));
        drop(permit);
        assert!(pending.abandoned());
    }
    #[test]
    fn permit_conservato_non_autorizza_recupero_del_gate() {
        let (pending, _permit) = Pending::new(JjAction::New(None));
        assert!(!pending.abandoned());
    }
    #[test]
    fn permit_admitted_drop_non_e_abbandono_pre_ammissione() {
        let (pending, permit) = Pending::new(JjAction::New(None));
        assert!(permit.admit());
        assert!(!permit.admit(), "non ammettere due volte");
        drop(permit);
        assert!(
            !pending.abandoned(),
            "nessuna collection/cancel/rollback inventata"
        );
    }
    #[tokio::test]
    async fn collector_pending_non_ammette_e_abbandono_sveglia() {
        let (pending, permit) = Pending::new(JjAction::New(None));
        let collector = pending.collector();
        let mut collect = Box::pin(collector.collect());
        assert!(collect.as_mut().now_or_never().is_none());
        assert_eq!(pending.state.load(Ordering::Acquire), PENDING);
        drop(permit);
        collect.await;
        assert!(pending.abandoned());
        assert!(pending.take_completion().is_none());
    }
    #[tokio::test]
    async fn shared_attende_il_worker_unico_dopo_drop_del_producer()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (pending, permit) = Pending::new(JjAction::New(None));
        assert!(permit.admit());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let original = permit
            .start(move || {
                observed.fetch_add(1, Ordering::AcqRel);
                let _ = started.send(());
                wait.recv_timeout(std::time::Duration::from_secs(2))
                    .map_err(|_| "barriera componente scaduta".to_owned())?;
                Ok("payload controllato".into())
            })
            .ok_or("invocazione componente assente")?;
        ready.await?;
        drop(original);
        drop(permit.start(|| panic!("non avviare un secondo worker")));
        drop(permit);
        release.send(())?;
        pending.collector().collect().await;
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(
            pending.take_completion(),
            Some(Ok("payload controllato".into()))
        );
        Ok(())
    }
    #[test]
    fn collector_estraneo_non_corrisponde_al_ticket_corrente() {
        let (a, _) = Pending::new(JjAction::New(None));
        let (b, _) = Pending::new(JjAction::New(None));
        assert!(a.matches(&a.collector()));
        assert!(!a.matches(&b.collector()));
    }
    #[test]
    fn collector_raccoglie_worker_ammesso_dopo_shutdown_runtime()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let (pending, permit) = Pending::new(JjAction::New(None));
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
                    Ok("esito dopo shutdown".to_owned())
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
        assert_eq!(result, "esito dopo shutdown");
        assert!(pending.take_completion().is_none());
        Ok(())
    }
}
