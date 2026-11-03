//! Canale async bounded posseduto dal consumer: nessun reader/relay thread.
use super::*;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub(super) enum EventSender {
    Blocking(SyncSender<QueryEvent>),
    Async(tokio::sync::mpsc::Sender<QueryEvent>),
}
impl From<SyncSender<QueryEvent>> for EventSender {
    fn from(sender: SyncSender<QueryEvent>) -> Self {
        Self::Blocking(sender)
    }
}
impl EventSender {
    pub(super) fn try_send(&self, event: QueryEvent) -> Result<(), TrySendError<QueryEvent>> {
        match self {
            Self::Blocking(sender) => sender.try_send(event),
            Self::Async(sender) => sender.try_send(event).map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(event) => TrySendError::Full(event),
                tokio::sync::mpsc::error::TrySendError::Closed(event) => {
                    TrySendError::Disconnected(event)
                }
            }),
        }
    }
    pub(super) fn is_closed(&self) -> bool {
        match self {
            Self::Blocking(_) => false,
            Self::Async(sender) => sender.is_closed(),
        }
    }
}

/// Provenienza viva della richiesta, non autorizzazione a SQL/MCP. Nessun
/// segreto/profile/socket esposto. Gen uguale su una connessione ricreata
/// non riattiva il vecchio token: shutdown dell'istanza resta osservabile.
#[derive(Clone)]
pub struct QueryOrigin {
    shared: Arc<Shared>,
    generation: Arc<AtomicU64>,
    expected: u64,
    alive: Arc<AtomicBool>,
}
impl QueryOrigin {
    /// Abbandona solo questo consumer (griglia nascosta/scartata). Non
    /// chiude la connessione, non interrompe SQL sul server e non è rollback.
    pub fn abandon(&self) {
        self.alive.store(false, Ordering::Release);
        self.shared.event_wake.notify_waiters();
    }
    pub fn is_current(&self) -> bool {
        self.alive.load(Ordering::Acquire)
            && !self.shared.shutdown.load(Ordering::Acquire)
            && self.generation.load(Ordering::Acquire) == self.expected
    }
}
impl std::fmt::Debug for QueryOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryOrigin").finish_non_exhaustive()
    }
}

/// Ricevitore async della stessa coda di due slot del worker. Drop chiude
/// prima il canale e sveglia la pausa del SUO sender; non manda Stop a un
/// altro job che potrebbe già aver occupato il controllo condiviso.
pub struct QueryEvents {
    receiver: tokio::sync::mpsc::Receiver<QueryEvent>,
    origin: QueryOrigin,
    // Future posseduta, droppata col receiver; non un task/thread/timer.
    scope_wake: Pin<Box<tokio::sync::futures::OwnedNotified>>,
}
impl QueryEvents {
    pub(super) fn channel(
        shared: Arc<Shared>,
        generation: Arc<AtomicU64>,
        expected: u64,
    ) -> (EventSender, Self) {
        let (sender, receiver) = tokio::sync::mpsc::channel(QUERY_EVENT_QUEUE_CAPACITY);
        let scope_wake = Box::pin(shared.event_wake.clone().notified_owned());
        let origin = QueryOrigin {
            shared,
            generation,
            expected,
            alive: Arc::new(AtomicBool::new(true)),
        };
        (
            EventSender::Async(sender),
            Self {
                receiver,
                origin,
                scope_wake,
            },
        )
    }
    pub fn origin(&self) -> QueryOrigin {
        self.origin.clone()
    }
    fn close(&mut self) {
        self.receiver.close();
        self.origin.alive.store(false, Ordering::Release);
        self.origin.shared.query_wake.notify_all();
    }
}
impl futures::Stream for QueryEvents {
    type Item = QueryEvent;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<QueryEvent>> {
        let this = self.get_mut();
        // Registrare PRIMA di leggere i flag: notify_waiters copre anche
        // notifiche tra creazione e primo poll della future posseduta.
        let notified = this.scope_wake.as_mut().poll(cx).is_ready();
        if notified {
            this.scope_wake
                .set(this.origin.shared.event_wake.clone().notified_owned());
        }
        if !this.origin.is_current() {
            this.close();
            return Poll::Ready(None);
        }
        if notified {
            // Solo fronte di notifica benigno: ripoll una volta per armare
            // la future nuova; non loop su notify né polling da inattivo.
            cx.waker().wake_by_ref();
        }
        let event = this.receiver.poll_recv(cx);
        if event.is_ready() {
            this.origin.shared.query_wake.notify_all();
        }
        event
    }
}
impl Drop for QueryEvents {
    fn drop(&mut self) {
        self.close();
    }
}
