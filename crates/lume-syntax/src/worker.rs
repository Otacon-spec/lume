//! Un solo thread posseduto, attesa a evento e coda latest-only.
//! `shutdown().join()` va eseguito da un collector fuori dal thread UI.
//! Drop è una rete di raccolta sincrona: non può lasciare thread detached.
use crate::{SyntaxProcessor, SyntaxRequest, SyntaxResult};
use std::{
    io,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    thread::{self, JoinHandle},
};

#[derive(Clone, Debug)]
pub struct SyntaxCompletion {
    /// Ticket del submit, monotono per owner (anche a revisione invariata).
    pub ticket: u64,
    pub document: u64,
    pub epoch: u64,
    pub revision: u64,
    /// None indica parse/query indisponibile; non un risultato parziale.
    pub result: Option<SyntaxResult>,
}

struct Job {
    generation: u64,
    request: Option<SyntaxRequest>,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
struct State {
    generation: u64,
    revocation: u64,
    pending: Option<Job>,
    active: Option<Arc<AtomicBool>>,
    result: Option<SyntaxCompletion>,
    closed: bool,
    waker: Option<Waker>,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn close(&self) {
        let waker = {
            let mut state = self.lock();
            state.closed = true;
            state.pending = None;
            if let Some(cancel) = &state.active {
                cancel.store(true, Ordering::Release);
            }
            state.waker.take()
        };
        self.ready.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[derive(Default)]
pub struct SyntaxWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}
impl SyntaxWorker {
    pub fn is_started(&self) -> bool {
        self.thread.is_some()
    }

    pub fn submit(&mut self, request: SyntaxRequest) -> io::Result<u64> {
        if self.shared.lock().closed {
            return Err(io::Error::other("syntax worker closed"));
        }
        if self.thread.is_none() {
            let shared = self.shared.clone();
            self.thread = Some(thread::Builder::new().name("lume-syntax".into()).spawn(
                move || {
                    let mut processor = SyntaxProcessor::default();
                    run(shared, |request, cancel| match request {
                        Some(request) => processor.process(request, cancel),
                        None => {
                            processor.clear();
                            None
                        }
                    });
                },
            )?);
        }
        let mut state = self.shared.lock();
        if state.closed {
            return Err(io::Error::other("syntax worker closed"));
        }
        let generation = state
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("syntax generation exhausted"))?;
        if let Some(cancel) = &state.active {
            cancel.store(true, Ordering::Release);
        }
        state.generation = generation;
        state.result = None;
        state.pending = Some(Job {
            generation,
            request: Some(request),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        drop(state);
        self.shared.ready.notify_one();
        Ok(generation)
    }

    /// Revoca job attivo/pending e svuota il parser sul thread proprietario.
    /// Nessun nuovo thread per la chiusura di una scheda mai elaborata.
    pub fn revoke(&mut self) -> io::Result<()> {
        let mut state = self.shared.lock();
        if state.closed {
            return Err(io::Error::other("syntax worker closed"));
        }
        let generation = state
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("syntax generation exhausted"))?;
        if let Some(cancel) = &state.active {
            cancel.store(true, Ordering::Release);
        }
        state.generation = generation;
        state.revocation = generation;
        state.result = None;
        state.pending = self.thread.as_ref().map(|_| Job {
            generation,
            request: None,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        drop(state);
        self.shared.ready.notify_one();
        Ok(())
    }

    /// Ricevitore per subscription: non possiede né prolunga il thread.
    pub fn receiver(&self) -> SyntaxReceiver {
        SyntaxReceiver {
            shared: self.shared.clone(),
        }
    }

    pub fn poll_result(&self, cx: &mut Context<'_>) -> Poll<Option<SyntaxCompletion>> {
        self.receiver().poll_result(cx)
    }

    pub fn shutdown(mut self) -> SyntaxCollector {
        self.shared.close();
        SyntaxCollector {
            thread: self.thread.take(),
        }
    }
}
impl Drop for SyntaxWorker {
    fn drop(&mut self) {
        self.shared.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Un solo consumer logico, clonabile per ricreare la subscription Iced.
/// Tutti i cloni condividono lo slot; solo l'ultimo waker registrato è svegliato.
#[derive(Clone)]
pub struct SyntaxReceiver {
    shared: Arc<Shared>,
}
impl std::fmt::Debug for SyntaxReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxReceiver").finish_non_exhaustive()
    }
}
impl PartialEq for SyntaxReceiver {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}
impl Eq for SyntaxReceiver {}
impl std::hash::Hash for SyntaxReceiver {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&Arc::as_ptr(&self.shared), state);
    }
}
impl SyntaxReceiver {
    /// Un solo consumer registra il waker; nessun polling periodico.
    /// Contesto della lane: stabile sui submit, cambia solo alla revoca.
    /// Non cambia Eq/Hash del receiver usati dalla subscription.
    pub fn revocation(&self) -> u64 {
        self.shared.lock().revocation
    }

    pub fn poll_result(&self, cx: &mut Context<'_>) -> Poll<Option<SyntaxCompletion>> {
        let mut state = self.shared.lock();
        if let Some(result) = state.result.take() {
            return Poll::Ready(Some(result));
        }
        if state.closed {
            return Poll::Ready(None);
        }
        if !state
            .waker
            .as_ref()
            .is_some_and(|w| w.will_wake(cx.waker()))
        {
            state.waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// Handle trasferibile al collector dell'app. Non dipende da un runtime Tokio.
/// Drop raccoglie il thread anche se la future di join non è più eseguita.
pub struct SyntaxCollector {
    thread: Option<JoinHandle<()>>,
}
impl SyntaxCollector {
    pub fn join(mut self) -> thread::Result<()> {
        self.thread.take().map_or(Ok(()), JoinHandle::join)
    }
}
impl Drop for SyntaxCollector {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct CloseOnExit(Arc<Shared>);
impl Drop for CloseOnExit {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn run(
    shared: Arc<Shared>,
    mut process: impl FnMut(Option<SyntaxRequest>, &AtomicBool) -> Option<SyntaxResult>,
) {
    let _finish = CloseOnExit(shared.clone());
    loop {
        let job = {
            let mut state = shared.lock();
            while state.pending.is_none() && !state.closed {
                state = shared.ready.wait(state).unwrap_or_else(|p| p.into_inner());
            }
            if state.closed {
                return;
            }
            let Some(job) = state.pending.take() else {
                continue;
            };
            state.active = Some(job.cancel.clone());
            job
        };
        let identity = job
            .request
            .as_ref()
            .map(|request| (request.document, request.epoch, request.revision));
        let result = process(job.request, &job.cancel);
        let waker = {
            let mut state = shared.lock();
            state.active = None;
            if !state.closed
                && state.generation == job.generation
                && !job.cancel.load(Ordering::Acquire)
            {
                if let Some(identity) = identity {
                    state.result = Some(SyntaxCompletion {
                        ticket: job.generation,
                        document: identity.0,
                        epoch: identity.1,
                        revision: identity.2,
                        result,
                    });
                    state.waker.take()
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LanguageId;
    use ropey::Rope;
    use std::{sync::mpsc, time::Duration};

    fn request(revision: u64) -> SyntaxRequest {
        let source = Rope::from_str("fn main() {}\n");
        SyntaxRequest {
            document: 1,
            epoch: 7,
            revision,
            language: LanguageId::Rust,
            window: 0..source.len_bytes(),
            source,
            edit: None,
        }
    }

    struct Signal(mpsc::Sender<()>);
    impl std::task::Wake for Signal {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
        fn wake_by_ref(self: &Arc<Self>) {
            let _ = self.0.send(());
        }
    }
    fn next(worker: &SyntaxWorker) -> Result<Option<SyntaxCompletion>, Box<dyn std::error::Error>> {
        let (tx, rx) = mpsc::channel();
        let waker = std::task::Waker::from(Arc::new(Signal(tx)));
        let mut cx = Context::from_waker(&waker);
        loop {
            match worker.poll_result(&mut cx) {
                Poll::Ready(result) => return Ok(result),
                Poll::Pending => rx.recv_timeout(Duration::from_secs(5))?,
            }
        }
    }

    #[test]
    fn running_collector_keeps_native_thread_owned_during_runtime_shutdown()
    -> Result<(), Box<dyn std::error::Error>> {
        struct Guard {
            release: Option<mpsc::Sender<()>>,
            shutdown: Option<thread::JoinHandle<()>>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                if let Some(release) = self.release.take() {
                    let _ = release.send(());
                }
                if let Some(shutdown) = self.shutdown.take() {
                    let _ = shutdown.join();
                }
            }
        }
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let shared = Arc::new(Shared::default());
        let child = shared.clone();
        let (native_entered_tx, native_entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let exited = Arc::new(AtomicBool::new(false));
        let exit_flag = exited.clone();
        let mut guard = Guard {
            release: Some(release_tx),
            shutdown: None,
        };
        let native = thread::spawn(move || {
            run(child, |_, cancel| {
                let _ = native_entered_tx.send(());
                let _ = release_rx.recv();
                assert!(cancel.load(Ordering::Acquire));
                None
            });
            exit_flag.store(true, Ordering::Release);
        });
        let mut worker = SyntaxWorker {
            shared,
            thread: Some(native),
        };
        worker.submit(request(1))?;
        if let Err(error) = native_entered_rx.recv_timeout(Duration::from_secs(5)) {
            if let Some(release) = guard.release.take() {
                let _ = release.send(());
            }
            let _ = worker.shutdown().join();
            return Err(error.into());
        }
        let collector = worker.shutdown();
        let (join_entered_tx, join_entered_rx) = mpsc::channel();
        let join = {
            let _entered = runtime.enter();
            tokio::task::spawn_blocking(move || {
                let _ = join_entered_tx.send(());
                collector.join().is_ok()
            })
        };
        join_entered_rx.recv_timeout(Duration::from_secs(5))?;
        assert!(!join.is_finished());
        assert!(!exited.load(Ordering::Acquire));
        let (shutdown_entered_tx, shutdown_entered_rx) = mpsc::channel();
        let (shutdown_done_tx, shutdown_done_rx) = mpsc::channel();
        guard.shutdown = Some(thread::spawn(move || {
            let _ = shutdown_entered_tx.send(());
            drop(runtime);
            let _ = shutdown_done_tx.send(());
        }));
        shutdown_entered_rx.recv_timeout(Duration::from_secs(5))?;
        assert!(matches!(
            shutdown_done_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        guard.release.take().ok_or("release")?.send(())?;
        shutdown_done_rx.recv_timeout(Duration::from_secs(5))?;
        guard
            .shutdown
            .take()
            .ok_or("shutdown")?
            .join()
            .map_err(|_| "shutdown panic")?;
        assert!(exited.load(Ordering::Acquire));
        let replacement = tokio::runtime::Builder::new_current_thread().build()?;
        assert!(replacement.block_on(join)?);
        Ok(())
    }

    #[test]
    fn dropping_live_owner_or_collector_joins_without_runtime()
    -> Result<(), Box<dyn std::error::Error>> {
        struct DisposeGuard {
            release: Option<mpsc::Sender<()>>,
            thread: Option<thread::JoinHandle<()>>,
        }
        impl Drop for DisposeGuard {
            fn drop(&mut self) {
                if let Some(release) = self.release.take() {
                    let _ = release.send(());
                }
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
            }
        }
        for collector in [false, true] {
            let shared = Arc::new(Shared::default());
            let child = shared.clone();
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let exited = Arc::new(AtomicBool::new(false));
            let exit_flag = exited.clone();
            let thread = thread::spawn(move || {
                run(child, |_, cancel| {
                    let _ = entered_tx.send(());
                    let _ = release_rx.recv();
                    assert!(cancel.load(Ordering::Acquire));
                    None
                });
                exit_flag.store(true, Ordering::Release);
            });
            let mut worker = SyntaxWorker {
                shared,
                thread: Some(thread),
            };
            worker.submit(request(1))?;
            let receiver = worker.receiver();
            if let Err(error) = entered_rx.recv_timeout(Duration::from_secs(5)) {
                let _ = release_tx.send(());
                let _ = worker.shutdown().join();
                return Err(error.into());
            }
            let (done_tx, done_rx) = mpsc::channel();
            let mut guard = DisposeGuard {
                release: Some(release_tx),
                thread: Some(thread::spawn(move || {
                    if collector {
                        drop(worker.shutdown());
                    } else {
                        drop(worker);
                    }
                    let _ = done_tx.send(());
                })),
            };
            let (wake_tx, wake_rx) = mpsc::channel();
            let waker = std::task::Waker::from(Arc::new(Signal(wake_tx)));
            let mut cx = Context::from_waker(&waker);
            loop {
                match receiver.poll_result(&mut cx) {
                    Poll::Ready(None) => break,
                    Poll::Ready(Some(_)) => return Err("unexpected completion".into()),
                    Poll::Pending => wake_rx.recv_timeout(Duration::from_secs(5))?,
                }
            }
            assert!(!exited.load(Ordering::Acquire));
            assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
            guard.release.take().ok_or("release")?.send(())?;
            done_rx.recv_timeout(Duration::from_secs(5))?;
            guard
                .thread
                .take()
                .ok_or("disposer")?
                .join()
                .map_err(|_| "disposer panic")?;
            assert!(exited.load(Ordering::Acquire));
        }
        Ok(())
    }

    #[test]
    fn failed_processing_delivers_completion_and_panic_closes_then_join_reports_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let shared = Arc::new(Shared::default());
        let child = shared.clone();
        let mut worker = SyntaxWorker {
            shared,
            thread: Some(thread::spawn(move || run(child, |_, _| None))),
        };
        worker.submit(request(1))?;
        let completion = next(&worker)?.ok_or("missing failure completion")?;
        assert_eq!(completion.revision, 1);
        assert!(completion.result.is_none());
        assert!(worker.shutdown().join().is_ok());

        let shared = Arc::new(Shared::default());
        let child = shared.clone();
        let mut worker = SyntaxWorker {
            shared,
            thread: Some(thread::spawn(move || {
                run(child, |_, _| panic!("injected syntax panic"))
            })),
        };
        worker.submit(request(1))?;
        assert!(next(&worker)?.is_none());
        assert!(worker.shutdown().join().is_err());
        Ok(())
    }

    #[test]
    fn revoke_cancels_live_job_and_resets_on_owner_thread() -> Result<(), Box<dyn std::error::Error>>
    {
        let shared = Arc::new(Shared::default());
        let child = shared.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (reset_tx, reset_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            run(child, |request, cancel| {
                if request.is_some() {
                    let _ = entered_tx.send(());
                    let _ = release_rx.recv();
                    assert!(cancel.load(Ordering::Acquire));
                } else {
                    let _ = reset_tx.send(thread::current().id());
                }
                None
            })
        });
        let owner_id = thread.thread().id();
        let mut worker = SyntaxWorker {
            shared,
            thread: Some(thread),
        };
        // Anche se una asserzione fallisce, sblocca il worker prima del suo Drop.
        struct Release(mpsc::Sender<()>);
        impl Drop for Release {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let _release = Release(release_tx.clone());
        worker.submit(request(1))?;
        entered_rx.recv_timeout(Duration::from_secs(5))?;
        worker.revoke()?;
        {
            let state = worker.shared.lock();
            assert!(
                state
                    .active
                    .as_ref()
                    .is_some_and(|c| c.load(Ordering::Acquire))
            );
            assert!(state.pending.as_ref().is_some_and(|j| j.request.is_none()));
        }
        release_tx.send(())?;
        assert_eq!(reset_rx.recv_timeout(Duration::from_secs(5))?, owner_id);
        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(worker.poll_result(&mut cx).is_pending());
        assert!(worker.shutdown().join().is_ok());
        Ok(())
    }

    #[test]
    fn busy_worker_cancels_active_and_keeps_only_latest_pending()
    -> Result<(), Box<dyn std::error::Error>> {
        let shared = Arc::new(Shared::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (processed_tx, processed_rx) = mpsc::channel();
        let child = shared.clone();
        let thread = thread::spawn(move || {
            run(child, |request, cancel| {
                let request = request?;
                if request.revision == 1 {
                    let _ = entered_tx.send(());
                    let _ = release_rx.recv();
                    assert!(cancel.load(Ordering::Acquire));
                }
                let _ = processed_tx.send(request.revision);
                None
            })
        });
        let mut worker = SyntaxWorker {
            shared,
            thread: Some(thread),
        };
        worker.submit(request(1))?;
        entered_rx.recv_timeout(Duration::from_secs(5))?;
        worker.submit(request(2))?;
        worker.submit(request(3))?;
        {
            let state = worker.shared.lock();
            assert_eq!(
                state
                    .pending
                    .as_ref()
                    .and_then(|j| j.request.as_ref().map(|r| r.revision)),
                Some(3)
            );
            assert!(
                state
                    .active
                    .as_ref()
                    .is_some_and(|c| c.load(Ordering::Acquire))
            );
            assert!(state.result.is_none());
        }
        release_tx.send(())?;
        assert_eq!(processed_rx.recv_timeout(Duration::from_secs(5))?, 1);
        assert_eq!(processed_rx.recv_timeout(Duration::from_secs(5))?, 3);
        let collector = worker.shutdown();
        assert!(collector.join().is_ok());
        assert!(processed_rx.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn collector_keeps_live_thread_until_cancelled_work_returns()
    -> Result<(), Box<dyn std::error::Error>> {
        let shared = Arc::new(Shared::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let child = shared.clone();
        let thread = thread::spawn(move || {
            run(child, |_, cancel| {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
                assert!(cancel.load(Ordering::Acquire));
                None
            })
        });
        let mut worker = SyntaxWorker {
            shared,
            thread: Some(thread),
        };
        worker.submit(request(1))?;
        entered_rx.recv_timeout(Duration::from_secs(5))?;
        let collector = worker.shutdown();
        assert!(collector.thread.as_ref().is_some_and(|t| !t.is_finished()));
        release_tx.send(())?;
        assert!(collector.join().is_ok());
        Ok(())
    }
}
