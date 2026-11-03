//! Trasporto della ricerca con future std, senza dipendenze async/executor.
//! Il caller possiede e DEVE raccogliere il JoinHandle su un worker, non UI.
use super::*;
use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex, MutexGuard},
    task::{Context, Poll, Waker},
};

/// Due eventi nel canale, non un tetto RAM: batch, testo trattenuto nel
/// modello e un'emissione in costruzione hanno costi separati.
pub const SEARCH_EVENT_CAPACITY: usize = 2;

struct State {
    queue: VecDeque<SearchUpdate>,
    canceled: bool,
    producer_closed: bool,
    consumer: Option<Waker>,
    #[cfg(test)]
    producer_waiting: bool,
}
struct Pipe {
    state: Mutex<State>,
    space: Condvar,
    generation: Arc<AtomicU64>,
    expected: u64,
}
fn lock(pipe: &Pipe) -> MutexGuard<'_, State> {
    pipe.state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Identità della singola ricerca e wake esplicito. Non è un permesso
/// filesystem/MCP né una conferma per sostituzioni o SQL.
#[derive(Clone)]
pub struct SearchOrigin {
    pipe: Arc<Pipe>,
}
impl SearchOrigin {
    /// Revoca SOLO questo consumer; non incrementa la generazione condivisa
    /// e non interrompe una read del filesystem già iniziata.
    pub fn cancel(&self) {
        let wake = {
            let mut state = lock(&self.pipe);
            state.canceled = true;
            state.queue = VecDeque::new();
            state.consumer.take()
        };
        self.pipe.space.notify_all();
        if let Some(wake) = wake {
            wake.wake();
        }
    }
    pub fn is_current(&self) -> bool {
        self.pipe.generation.load(Ordering::SeqCst) == self.pipe.expected
            && !lock(&self.pipe).canceled
    }
}
impl std::fmt::Debug for SearchOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchOrigin").finish_non_exhaustive()
    }
}

/// Source posseduta: drop/cancel libera la coda e sveglia producer e future.
/// Modificare direttamente AtomicU64 NON notifica una future o un producer
/// già in attesa: in quel caso il caller deve anche origin.cancel().
pub struct SearchEvents {
    origin: SearchOrigin,
}
impl SearchEvents {
    pub fn origin(&self) -> SearchOrigin {
        self.origin.clone()
    }
    pub fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<SearchUpdate>> {
        // Anche clone/drop di un RawWaker può richiamare codice esterno:
        // non eseguirlo sotto il lock di scope/coda.
        let mut incoming = Some(cx.waker().clone());
        let (result, retired) = {
            let mut state = lock(&self.origin.pipe);
            let mut retired = state.consumer.take();
            // Registrazione sotto lo stesso lock dei flag: nessuna notifica
            // persa fra controllo canceled/EOF e registrazione del waker.
            let result = if self.origin.pipe.generation.load(Ordering::SeqCst)
                != self.origin.pipe.expected
                || state.canceled
            {
                state.canceled = true;
                state.queue = VecDeque::new();
                Poll::Ready(None)
            } else if let Some(update) = state.queue.pop_front() {
                Poll::Ready(Some(update))
            } else if state.producer_closed {
                Poll::Ready(None)
            } else {
                state.consumer = if retired
                    .as_ref()
                    .is_some_and(|old| old.will_wake(cx.waker()))
                {
                    retired.take()
                } else {
                    incoming.take()
                };
                Poll::Pending
            };
            (result, retired)
        };
        drop(retired);
        drop(incoming);
        if result.is_ready() {
            self.origin.pipe.space.notify_all();
        }
        result
    }
    pub async fn next(&mut self) -> Option<SearchUpdate> {
        std::future::poll_fn(|cx| self.poll_next(cx)).await
    }
}
impl Drop for SearchEvents {
    fn drop(&mut self) {
        self.origin.cancel();
    }
}
impl std::fmt::Debug for SearchEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchEvents").finish_non_exhaustive()
    }
}

pub(super) struct EventSender {
    origin: SearchOrigin,
}
impl EventSender {
    pub(super) fn is_canceled(&self) -> bool {
        !self.origin.is_current()
    }
    pub(super) fn send(&self, update: SearchUpdate) -> Result<(), SearchUpdate> {
        let wake = {
            let mut state = lock(&self.origin.pipe);
            loop {
                if state.canceled
                    || self.origin.pipe.generation.load(Ordering::SeqCst)
                        != self.origin.pipe.expected
                {
                    return Err(update);
                }
                if state.queue.len() < SEARCH_EVENT_CAPACITY {
                    break;
                }
                #[cfg(test)]
                {
                    state.producer_waiting = true;
                    self.origin.pipe.space.notify_all();
                }
                // Spazio, drop/cancel o chiusura sono fronti notificati;
                // niente sleep/polling mentre il canale è pieno o idle.
                state = self
                    .origin
                    .pipe
                    .space
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                #[cfg(test)]
                {
                    state.producer_waiting = false;
                }
            }
            state.queue.push_back(update);
            state.consumer.take()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
        Ok(())
    }
}
impl Drop for EventSender {
    fn drop(&mut self) {
        let wake = {
            let mut state = lock(&self.origin.pipe);
            state.producer_closed = true;
            state.consumer.take()
        };
        self.origin.pipe.space.notify_all();
        if let Some(wake) = wake {
            wake.wake();
        }
    }
}
fn channel(generation: Arc<AtomicU64>, expected: u64) -> (EventSender, SearchEvents) {
    let pipe = Arc::new(Pipe {
        state: Mutex::new(State {
            queue: VecDeque::new(),
            canceled: false,
            producer_closed: false,
            consumer: None,
            #[cfg(test)]
            producer_waiting: false,
        }),
        space: Condvar::new(),
        generation,
        expected,
    });
    let origin = SearchOrigin { pipe };
    (
        EventSender {
            origin: origin.clone(),
        },
        SearchEvents { origin },
    )
}

/// Il receiver controlla la cancellazione, il JoinHandle controlla la
/// raccolta. Drop del receiver NON è join; drop dell'handle stacca il thread
/// standard, non è una strategia di cleanup. Trasferire l'handle a un worker
/// che lo attenda anche quando il consumer è stato abbandonato.
#[must_use = "drenare/cancellare gli eventi e raccogliere il producer fuori dalla UI"]
pub struct SearchRun {
    pub events: SearchEvents,
    pub worker: std::thread::JoinHandle<()>,
}
impl std::fmt::Debug for SearchRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchRun").finish_non_exhaustive()
    }
}

/// Stesso engine e stessi limiti di spawn_search. Fallimento di spawn
/// esplicito; nessun relay, runtime, timer o pool in lume-core.
/// Il caller deve cancel notificato prima di raccogliere un producer la cui
/// coda non viene più drenata. L'I/O già in corso resta cooperativa.
pub fn spawn_search_owned(
    scope: SearchScope,
    options: SearchOptions,
    generation: Arc<AtomicU64>,
    expected: u64,
) -> std::io::Result<SearchRun> {
    let (sender, events) = channel(Arc::clone(&generation), expected);
    let worker = std::thread::Builder::new()
        .name("lume-cerca".into())
        .spawn(move || {
            let emitter = Emitter::Owned(sender);
            run_search(&scope, &options, &generation, expected, &emitter);
        })?;
    Ok(SearchRun { events, worker })
}

#[cfg(test)]
mod tests;
