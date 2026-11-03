//! Owner e consenso separati dalla consegna UI; nessun Plan attraversa la UI.
use super::{OpenInput, Outcome, aborted, commit_sync, preflight, prepare_sync, worker_error};
use crate::save::gate::{Gate, Lease, Pause, WeakLease};
use futures::{FutureExt, Stream, future::BoxFuture};
use lume_core::{DocId, FileMatches};
use std::{
    fmt,
    hash::{Hash, Hasher},
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

type Handle = futures::future::Shared<BoxFuture<'static, Receipt>>;
struct Inputs {
    matches: Vec<FileMatches>,
    replacement: String,
    opened: Vec<OpenInput>,
    mapping: Vec<(DocId, PathBuf)>,
}
enum Phase {
    Pending(Option<Inputs>),
    Running(Handle),
    Done(Receipt),
}
struct Inner {
    id: u64,
    phase: Mutex<Phase>,
    wake: tokio::sync::Notify,
    gate: Arc<Gate>,
    mailbox: Arc<Mailbox>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        // Dopo Done il flag appartiene ancora alla fase autosave dell'app:
        // liberare il collector concluso non revoca quei nuovi salvataggi.
        let finished = matches!(
            self.phase
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Phase::Done(_)
        );
        if !finished {
            self.gate.deny();
            self.cancel.store(true, Ordering::Release);
        }
    }
}
// Identità canonica senza sorgenti, path o handle filesystem nel messaggio.
struct Metadata;
struct Notice {
    metadata: Arc<Metadata>,
    initial: Option<Lease>,
    decision: WeakLease,
}
struct Mailbox {
    notice: Mutex<Option<Notice>>,
    wake: tokio::sync::Notify,
}
struct WorkerGate {
    gate: Arc<Gate>,
    mailbox: Weak<Mailbox>,
}
impl WorkerGate {
    fn ready(&self) -> bool {
        let Some(lease) = self.gate.ready() else {
            return false;
        };
        let Some(mailbox) = self.mailbox.upgrade() else {
            return false;
        };
        let decision = lease.downgrade();
        *mailbox
            .notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Notice {
            metadata: Arc::new(Metadata),
            initial: Some(lease),
            decision,
        });
        mailbox.wake.notify_waiters();
        drop(mailbox);
        self.gate.wait_permission()
    }
    fn run(self, inputs: Inputs, cancel: Arc<AtomicBool>) -> Outcome {
        let result = match prepare_sync(
            inputs.matches,
            inputs.replacement,
            inputs.opened,
            inputs.mapping,
            cancel.clone(),
        ) {
            Err(report) => Outcome {
                opened: Vec::new(),
                report: *report,
            },
            Ok(plan) => {
                if !self.ready() {
                    aborted(plan, "consenso negato; nessun commit avviato".into())
                } else if let Err(error) = preflight(&plan, &cancel) {
                    aborted(plan, error)
                } else if !self.gate.wait_commit() {
                    aborted(plan, "consenso revocato prima dei commit".into())
                } else {
                    // Un solo punto di linearizzazione per il batch, non una
                    // riserva per file né garanzia su crash/ACL/Windows GUI.
                    commit_sync(plan, &cancel)
                }
            }
        };
        self.gate.done();
        result
    }
}
#[derive(Clone)]
pub struct Ready {
    metadata: Arc<Metadata>,
    lease: Lease,
}
impl Ready {
    pub fn grant(&self) -> bool {
        self.lease.grant()
    }
    pub fn deny(&self) {
        self.lease.deny();
    }
}
impl fmt::Debug for Ready {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplaceReady { .. }")
    }
}
#[derive(Clone, Debug)]
pub enum Event {
    Ready(Ready),
    Done(Receipt),
}
#[derive(Clone)]
pub struct Receipt(Arc<Mutex<Option<Outcome>>>);
impl Receipt {
    fn new(outcome: Outcome) -> Self {
        Self(Arc::new(Mutex::new(Some(outcome))))
    }
    pub fn take(&self) -> Option<Outcome> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}
impl fmt::Debug for Receipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplaceReceipt { .. }")
    }
}
#[derive(Clone)]
pub struct Job(Arc<Inner>);
impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplaceJob { .. }")
    }
}
impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Job {}
impl Hash for Job {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}
impl Job {
    pub fn new(
        id: u64,
        matches: Vec<FileMatches>,
        replacement: String,
        opened: Vec<OpenInput>,
        mapping: Vec<(DocId, PathBuf)>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        Self(Arc::new(Inner {
            id,
            phase: Mutex::new(Phase::Pending(Some(Inputs {
                matches,
                replacement,
                opened,
                mapping,
            }))),
            wake: tokio::sync::Notify::new(),
            gate: Gate::new(),
            mailbox: Arc::new(Mailbox {
                notice: Mutex::new(None),
                wake: tokio::sync::Notify::new(),
            }),
            cancel,
        }))
    }
    fn lock(&self) -> MutexGuard<'_, Phase> {
        self.0
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn id(&self) -> u64 {
        self.0.id
    }
    pub fn matches(&self, receipt: &Receipt) -> bool {
        matches!(&*self.lock(), Phase::Done(actual) if Arc::ptr_eq(&actual.0, &receipt.0))
    }
    pub fn matches_ready(&self, ready: &Ready) -> bool {
        self.0.gate.is_ready()
            && self
                .0
                .mailbox
                .notice
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|actual| {
                    Arc::ptr_eq(&actual.metadata, &ready.metadata)
                        && actual.decision.matches(&ready.lease)
                })
    }
    pub fn pause(&self) -> Pause {
        self.0.gate.pause()
    }
    pub fn revoke(&self) {
        self.0.gate.deny();
        self.0.cancel.store(true, Ordering::Release);
    }
    pub fn delivery(&self) -> Delivery {
        Delivery {
            job: self.clone(),
            future: None,
            polled: false,
            ready_seen: false,
            done: false,
        }
    }
    fn start(&self) {
        {
            let mut phase = self.lock();
            let Phase::Pending(input) = &mut *phase else {
                return;
            };
            let Some(inputs) = input.take() else {
                return;
            };
            *phase = match tokio::runtime::Handle::try_current() {
                Err(_) => Phase::Done(Receipt::new(worker_error(
                    "Runtime di sostituzione non disponibile: nessuna scrittura avviata".into(),
                ))),
                Ok(runtime) => {
                    let worker = WorkerGate {
                        gate: self.0.gate.clone(),
                        mailbox: Arc::downgrade(&self.0.mailbox),
                    };
                    let cancel = self.0.cancel.clone();
                    Phase::Running(async move {
                        let outcome = runtime.spawn_blocking(move || worker.run(inputs, cancel)).await
                            .unwrap_or_else(|error| worker_error(format!("Worker sostituzione terminato: {error}; esiti disco e residui da verificare")));
                        Receipt::new(outcome)
                    }.boxed().shared())
                }
            };
        }
        self.0.wake.notify_waiters();
    }
    fn abandon_pending(&self) {
        {
            let mut phase = self.lock();
            let Phase::Pending(input) = &mut *phase else {
                return;
            };
            let Some(_inputs) = input.take() else {
                return;
            };
            *phase = Phase::Done(Receipt::new(worker_error(
                "Sostituzione non avviata: richiesta abbandonata".into(),
            )));
        }
        self.0.wake.notify_waiters();
    }
    fn ready(&self) -> Option<Ready> {
        if !self.0.gate.is_ready() {
            return None;
        }
        let mut notice = self
            .0
            .mailbox
            .notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let actual = notice.as_mut()?;
        // Dopo la prima consegna nessuna ricreazione dell'ultima lease persa.
        let lease = actual
            .initial
            .take()
            .or_else(|| actual.decision.upgrade())?;
        Some(Ready {
            metadata: actual.metadata.clone(),
            lease,
        })
    }
    async fn next_event(&self, ready_seen: bool) -> Event {
        loop {
            let notified = self.0.wake.notified();
            let prepared = self.0.mailbox.wake.notified();
            tokio::pin!(notified, prepared);
            notified.as_mut().enable();
            prepared.as_mut().enable();
            let handle = {
                let phase = self.lock();
                match &*phase {
                    Phase::Done(receipt) => return Event::Done(receipt.clone()),
                    Phase::Running(handle) => Some(handle.clone()),
                    Phase::Pending(_) => None,
                }
            };
            if !ready_seen && let Some(ready) = self.ready() {
                return Event::Ready(ready);
            }
            if let Some(handle) = handle {
                tokio::select! {
                    receipt = handle => {
                        *self.lock() = Phase::Done(receipt.clone());
                        self.0.wake.notify_waiters();
                        return Event::Done(receipt);
                    }
                    _ = &mut prepared => {}
                }
            } else {
                notified.await;
            }
        }
    }
    #[cfg(test)]
    fn set_commit_started_hook(&self, hook: impl FnOnce() + Send + 'static) {
        self.0.gate.set_commit_started_hook(hook);
    }
}
// Il permit di ammissione è sincrono: anche drop senza primo poll chiude Pending.
pub struct Delivery {
    job: Job,
    future: Option<BoxFuture<'static, Event>>,
    polled: bool,
    ready_seen: bool,
    done: bool,
}
impl Stream for Delivery {
    type Item = Event;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        if !this.polled {
            this.polled = true;
            this.job.start();
        }
        if this.future.is_none() {
            let job = this.job.clone();
            let seen = this.ready_seen;
            this.future = Some(async move { job.next_event(seen).await }.boxed());
        }
        let Some(future) = &mut this.future else {
            return Poll::Pending;
        };
        match future.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(event) => {
                this.future = None;
                match &event {
                    Event::Ready(_) => this.ready_seen = true,
                    Event::Done(_) => this.done = true,
                }
                Poll::Ready(Some(event))
            }
        }
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        self.job.abandon_pending();
    }
}
/// Collect-only: stessa invocazione, nessuna ammissione o approvazione implicita.
pub fn completed(job: &Job) -> impl Stream<Item = Event> + Send + use<> {
    futures::stream::unfold(
        (job.clone(), false, false),
        |(job, seen, done)| async move {
            if done {
                return None;
            }
            let event = job.next_event(seen).await;
            let next_seen = seen || matches!(event, Event::Ready(_));
            let next_done = matches!(event, Event::Done(_));
            Some((event, (job, next_seen, next_done)))
        },
    )
}
#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;
