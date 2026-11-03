//! Owner del save distinto dalla Task. Il collector non ammette richieste
//! fredde e condivide una sola join future, anche durante l'handshake Ready.
use super::{
    Completion, Request, Ticket,
    gate::{Gate, Lease, Pause, WeakLease},
};
use futures::{FutureExt, Stream, future::BoxFuture};
use std::{
    fmt,
    hash::{Hash, Hasher},
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, Weak},
    task::{Context, Poll},
};

type Handle = futures::future::Shared<BoxFuture<'static, Receipt>>;
enum Phase {
    Pending(Option<Box<Request>>),
    Running(Handle),
    Done(Receipt),
}
struct Inner {
    ticket: Ticket,
    phase: Mutex<Phase>,
    wake: tokio::sync::Notify,
    gate: Option<Arc<Gate>>,
    mailbox: Arc<Mailbox>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(gate) = &self.gate {
            gate.deny();
        }
    }
}
/// Fotografia canonica della preparazione effettiva, non un diritto a scrivere.
pub(crate) struct Metadata {
    pub(crate) ticket: Ticket,
    pub(crate) path: std::path::PathBuf,
    pub(crate) snapshot: lume_core::SaveSnapshot,
    pub(crate) resolved: std::path::PathBuf,
    pub(crate) note: Option<NoteAuthority>,
}
pub(crate) struct NoteAuthority {
    pub(crate) scope: u64,
    pub(crate) id: lume_notes::NoteId,
}
struct Notice {
    metadata: Arc<Metadata>,
    initial: Option<Lease>,
    decision: WeakLease,
}
struct Mailbox {
    notice: Mutex<Option<Notice>>,
    wake: tokio::sync::Notify,
}
/// Il worker trattiene soltanto il gate e un riferimento debole alla mailbox:
/// né ciclo owner/future né ultimo owner artificialmente tenuto dal worker.
pub(super) struct WorkerGate {
    gate: Arc<Gate>,
    mailbox: Weak<Mailbox>,
}
impl WorkerGate {
    pub(super) fn ready(&self, metadata: Metadata) -> bool {
        let Some(lease) = self.gate.ready() else {
            return false;
        };
        let Some(mailbox) = self.mailbox.upgrade() else {
            return false;
        };
        let decision = lease.downgrade();
        {
            *mailbox
                .notice
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Notice {
                metadata: Arc::new(metadata),
                initial: Some(lease),
                decision,
            });
        }
        mailbox.wake.notify_waiters();
        drop(mailbox);
        self.gate.wait_permission()
    }
    pub(super) fn begin_commit(&self) -> bool {
        self.gate.wait_commit()
    }
    pub(super) fn done(&self) {
        self.gate.done();
    }
}
#[derive(Clone)]
pub(crate) struct Ready {
    metadata: Arc<Metadata>,
    lease: Lease,
}
impl Ready {
    pub(crate) fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub(crate) fn grant(&self) -> bool {
        self.lease.grant()
    }
    pub(crate) fn deny(&self) {
        self.lease.deny();
    }
}
impl fmt::Debug for Ready {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveReady")
            .field("ticket", &self.metadata.ticket)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug)]
pub(crate) enum Event {
    Ready(Ready),
    Done(Receipt),
}

/// Ricevuta condivisa consumabile una volta, mai token ricostruito dal ticket.
#[derive(Clone)]
pub(crate) struct Receipt(Arc<Mutex<Option<Completion>>>);
impl Receipt {
    fn new(completion: Completion) -> Self {
        Self(Arc::new(Mutex::new(Some(completion))))
    }
    pub(crate) fn take(&self) -> Option<Completion> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}
impl fmt::Debug for Receipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SaveReceipt { .. }")
    }
}
#[derive(Clone)]
pub(crate) struct Job(Arc<Inner>);
impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveJob")
            .field("ticket", &self.0.ticket)
            .finish_non_exhaustive()
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
    fn lock(&self) -> MutexGuard<'_, Phase> {
        self.0
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn new(request: Request) -> Self {
        Self(Arc::new(Inner {
            ticket: request.ticket,
            // File e NoteStore espongono entrambi Ready solo dopo la
            // preparazione e prima del primo effetto di pubblicazione.
            gate: Some(Gate::new()),
            mailbox: Arc::new(Mailbox {
                notice: Mutex::new(None),
                wake: tokio::sync::Notify::new(),
            }),
            phase: Mutex::new(Phase::Pending(Some(Box::new(request)))),
            wake: tokio::sync::Notify::new(),
        }))
    }
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        matches!(&*self.lock(), Phase::Running(_))
    }
    pub(crate) fn matches(&self, receipt: &Receipt) -> bool {
        matches!(&*self.lock(), Phase::Done(actual) if Arc::ptr_eq(&actual.0, &receipt.0))
    }
    pub(crate) fn matches_ready(&self, ready: &Ready) -> bool {
        self.0.gate.as_ref().is_some_and(|gate| gate.is_ready())
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
    pub(crate) fn ticket(&self) -> Ticket {
        self.0.ticket
    }
    pub(crate) fn pause(&self) -> Option<Pause> {
        self.0.gate.as_ref().map(|gate| gate.pause())
    }
    #[cfg(test)]
    pub(crate) fn set_commit_started_hook(&self, hook: impl FnOnce() + Send + 'static) {
        if let Some(gate) = &self.0.gate {
            gate.set_commit_started_hook(hook);
        }
    }
    pub(crate) fn revoke(&self) {
        if let Some(gate) = &self.0.gate {
            gate.deny();
        }
    }
    pub(crate) fn delivery(&self) -> Delivery {
        Delivery {
            job: self.clone(),
            future: None,
            polled: false,
            ready_seen: false,
            done: false,
        }
    }
    fn not_started(request: Request, reason: &str) -> Receipt {
        Receipt::new(Completion {
            ticket: request.ticket,
            path: request.path,
            snapshot: request.snapshot,
            result: Err(reason.into()),
            fingerprint: None,
            resolved: None,
            warning: None,
        })
    }
    fn start(&self) {
        {
            let mut phase = self.lock();
            let Phase::Pending(input) = &mut *phase else {
                return;
            };
            let Some(request) = input.take() else {
                return;
            };
            *phase = match tokio::runtime::Handle::try_current() {
                Err(_) => Phase::Done(Self::not_started(
                    *request,
                    "Runtime di salvataggio non disponibile: nessuna scrittura avviata",
                )),
                Ok(runtime) => {
                    let gate = self.0.gate.as_ref().map(|gate| WorkerGate {
                        gate: gate.clone(),
                        mailbox: Arc::downgrade(&self.0.mailbox),
                    });
                    Phase::Running(
                        async move { Receipt::new(request.run_on(runtime, gate).await) }
                            .boxed()
                            .shared(),
                    )
                }
            };
        }
        self.0.wake.notify_waiters();
    }
    /// NonSalvare revoca anche Preparing/Ready/Granted; NormalSave ammesso
    /// mantiene la semantica precedente. Flight vive fino alla vera ricevuta.
    pub(crate) fn discard_pending(&self, document: Option<lume_core::DocId>) {
        if document.is_none_or(|id| id == self.0.ticket.document) {
            self.revoke();
            self.abandon_pending();
        }
    }
    fn abandon_pending(&self) {
        {
            let mut phase = self.lock();
            let Phase::Pending(input) = &mut *phase else {
                return;
            };
            let Some(request) = input.take() else {
                return;
            };
            *phase = Phase::Done(Self::not_started(
                *request,
                "Salvataggio non avviato: richiesta abbandonata",
            ));
        }
        self.0.wake.notify_waiters();
    }
    fn ready(&self) -> Option<Ready> {
        let gate = self.0.gate.as_ref()?;
        if !gate.is_ready() {
            return None;
        }
        let mut notice = self
            .0
            .mailbox
            .notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let actual = notice.as_mut()?;
        // Una volta consegnata, soltanto upgrade della STESSA lease ancora
        // viva. Non ricreare diritti persi da drop/decisione precedente.
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
                        { *self.lock() = Phase::Done(receipt.clone()); }
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
    /// Aiutante collect-only per ricevute storiche: non approva mai un Ready.
    #[cfg(test)]
    pub(crate) async fn collect(&self) -> Receipt {
        let mut seen = false;
        loop {
            match self.next_event(seen).await {
                Event::Ready(ready) => {
                    seen = true;
                    drop(ready);
                }
                Event::Done(receipt) => return receipt,
            }
        }
    }
}

/// Il permit di ammissione è fuori da async move: anche Drop prima del primo
/// poll abbandona davvero Pending. Nessun secondo writer nel fallback.
pub(crate) struct Delivery {
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
#[cfg(test)]
impl futures::Future for Delivery {
    type Output = Receipt;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Receipt> {
        loop {
            match self.as_mut().poll_next(cx) {
                Poll::Ready(Some(Event::Done(receipt))) => return Poll::Ready(receipt),
                Poll::Ready(Some(Event::Ready(ready))) => drop(ready),
                _ => return Poll::Pending,
            }
        }
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        self.job.abandon_pending();
    }
}

pub(crate) fn completed(job: &Job) -> impl Stream<Item = Event> + Send + use<> {
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
