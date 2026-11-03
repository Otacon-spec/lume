//! Invalidazioni limitate: non attendere spazio nella callback del backend.
use super::{FsEvent, accumulate_paths};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, MutexGuard, mpsc},
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

const ERROR_BYTES: usize = 8 * 1024;

#[derive(Default, Debug)]
struct Pending {
    paths: HashSet<PathBuf>,
    bytes: usize,
    rescan: bool,
    error: Option<String>,
    prefer_data: bool,
}
impl Pending {
    fn push(&mut self, event: FsEvent) {
        match event {
            FsEvent::Changed(paths) => {
                accumulate_paths(&mut self.paths, &mut self.bytes, &mut self.rescan, paths);
            }
            FsEvent::Rescan => {
                self.paths.clear();
                self.bytes = 0;
                self.rescan = true;
            }
            FsEvent::Error(error) => {
                const SUFFIX: &str = "… [troncato]";
                self.error = Some(if error.len() > ERROR_BYTES {
                    let mut end = ERROR_BYTES - SUFFIX.len();
                    while !error.is_char_boundary(end) {
                        end -= 1;
                    }
                    let mut bounded = String::with_capacity(ERROR_BYTES);
                    bounded.push_str(&error[..end]);
                    bounded.push_str(SUFFIX);
                    bounded
                } else if error.capacity() > ERROR_BYTES {
                    error.as_str().to_owned()
                } else {
                    error
                });
            }
        }
    }
    fn pop(&mut self) -> Option<FsEvent> {
        let data = self.rescan || !self.paths.is_empty();
        if self.error.is_some() && (!data || !self.prefer_data) {
            self.prefer_data = true;
            return self.error.take().map(FsEvent::Error);
        }
        if self.rescan {
            self.rescan = false;
            self.prefer_data = false;
            return Some(FsEvent::Rescan);
        }
        if !self.paths.is_empty() {
            self.bytes = 0;
            self.prefer_data = false;
            return Some(FsEvent::Changed(
                std::mem::take(&mut self.paths).into_iter().collect(),
            ));
        }
        None
    }
}
#[derive(Default, Debug)]
struct State {
    pending: Pending,
    sender_closed: bool,
    receiver_closed: bool,
    waker: Option<Waker>,
}
#[derive(Default, Debug)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Sender singolo: fonde invalidazioni, conserva l'ultimo errore, mai attesa di spazio.
#[derive(Debug)]
pub struct FsEventSender(Arc<Shared>);
/// Receiver posseduto, bloccante o pollable con Waker std, senza runtime richiesto.
#[derive(Debug)]
pub struct FsEventReceiver(Arc<Shared>);

/// Una mailbox per uno scope immutabile, con sender e receiver unici.
pub fn fs_event_channel() -> (FsEventSender, FsEventReceiver) {
    let shared = Arc::new(Shared::default());
    (FsEventSender(Arc::clone(&shared)), FsEventReceiver(shared))
}
impl FsEventSender {
    /// Err restituisce il payload se il consumer è stato abbandonato.
    pub fn send(&self, event: FsEvent) -> Result<(), FsEvent> {
        let wake = {
            let mut state = self.0.lock();
            if state.receiver_closed {
                return Err(event);
            }
            state.pending.push(event);
            state.waker.take()
        };
        self.0.wake.notify_all();
        if let Some(wake) = wake {
            wake.wake();
        }
        Ok(())
    }
}
impl Drop for FsEventSender {
    fn drop(&mut self) {
        let wake = {
            let mut state = self.0.lock();
            state.sender_closed = true;
            state.waker.take()
        };
        self.0.wake.notify_all();
        if let Some(wake) = wake {
            wake.wake();
        }
    }
}
impl FsEventReceiver {
    /// Eventi già pendenti; limite superiore finito soltanto dopo EOF.
    pub fn size_hint(&self) -> (usize, Option<usize>) {
        let state = self.0.lock();
        let count = usize::from(state.pending.error.is_some())
            + usize::from(state.pending.rescan || !state.pending.paths.is_empty());
        (count, state.sender_closed.then_some(count))
    }
    pub fn try_recv(&self) -> Result<FsEvent, mpsc::TryRecvError> {
        let mut state = self.0.lock();
        if let Some(event) = state.pending.pop() {
            return Ok(event);
        }
        Err(if state.sender_closed {
            mpsc::TryRecvError::Disconnected
        } else {
            mpsc::TryRecvError::Empty
        })
    }
    pub fn recv(&self) -> Result<FsEvent, mpsc::RecvError> {
        let mut state = self.0.lock();
        loop {
            if let Some(event) = state.pending.pop() {
                return Ok(event);
            }
            if state.sender_closed {
                return Err(mpsc::RecvError);
            }
            state = self
                .0
                .wake
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<FsEvent, mpsc::RecvTimeoutError> {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = self.0.lock();
        loop {
            if let Some(event) = state.pending.pop() {
                return Ok(event);
            }
            if state.sender_closed {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            let Some(left) = deadline.map(|end| end.saturating_duration_since(Instant::now()))
            else {
                state = self
                    .0
                    .wake
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            };
            if left.is_zero() {
                return Err(mpsc::RecvTimeoutError::Timeout);
            }
            state = self
                .0
                .wake
                .wait_timeout(state, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
    pub fn try_iter(&self) -> impl Iterator<Item = FsEvent> + '_ {
        std::iter::from_fn(|| self.try_recv().ok())
    }
    /// Registra il Waker sotto lo stesso lock del controllo pending/EOF.
    pub fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<FsEvent>> {
        let mut incoming = Some(cx.waker().clone());
        let (result, retired) = {
            let mut state = self.0.lock();
            let old = state.waker.take();
            let result = if let Some(event) = state.pending.pop() {
                Poll::Ready(Some(event))
            } else if state.sender_closed {
                Poll::Ready(None)
            } else {
                state.waker = incoming.take();
                Poll::Pending
            };
            (result, old)
        };
        drop(retired);
        drop(incoming);
        result
    }
}
impl Drop for FsEventReceiver {
    fn drop(&mut self) {
        let (pending, waker) = {
            let mut state = self.0.lock();
            state.receiver_closed = true;
            (std::mem::take(&mut state.pending), state.waker.take())
        };
        drop(pending);
        drop(waker);
        self.0.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    #[test]
    fn notifiche_spurie_non_rinnovano_timeout() {
        let (_sender, receiver) = fs_event_channel();
        let shared = Arc::clone(&receiver.0);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer_stop = Arc::clone(&stop);
        let notifier = std::thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(2);
            while !writer_stop.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < end {
                shared.wake.notify_all();
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let started = Instant::now();
        let result = receiver.recv_timeout(Duration::from_millis(10));
        let elapsed = started.elapsed();
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        notifier.join().unwrap();
        assert_eq!(result, Err(mpsc::RecvTimeoutError::Timeout));
        // Tolleranza della prova, non un budget UI: il flusso spurio potrebbe
        // continuare2s, ma non deve rinnovare una deadline di10ms.
        assert!(elapsed < Duration::from_secs(1));
    }
    #[test]
    fn drop_receiver_droppa_waker_fuori_mutex() {
        let (_sender, mut receiver) = fs_event_channel();
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let waker = Waker::from(Arc::new(DropObserver {
                shared: Arc::downgrade(&receiver.0),
                dropped: Arc::clone(&dropped),
            }));
            assert!(
                receiver
                    .poll_next(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        }
        drop(receiver);
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    #[test]
    fn send_concorrente_a_poll_non_perde_notifica_o_evento() {
        for _ in 0..100 {
            let (sender, mut receiver) = fs_event_channel();
            let observer = Arc::new(Observer {
                shared: Arc::clone(&receiver.0),
                wakes: std::sync::atomic::AtomicUsize::new(0),
            });
            let waker = Waker::from(Arc::clone(&observer));
            let mut cx = Context::from_waker(&waker);
            let producer = std::thread::spawn(move || sender.send(FsEvent::Rescan).unwrap());
            let first = receiver.poll_next(&mut cx);
            producer.join().unwrap();
            if first.is_pending() {
                assert!(observer.wakes.load(std::sync::atomic::Ordering::SeqCst) > 0);
                assert_eq!(
                    receiver.poll_next(&mut cx),
                    Poll::Ready(Some(FsEvent::Rescan))
                );
            } else {
                assert_eq!(first, Poll::Ready(Some(FsEvent::Rescan)));
            }
            assert_eq!(receiver.poll_next(&mut cx), Poll::Ready(None));
        }
    }
    #[test]
    fn errore_corto_con_capacita_enorme_non_conserva_allocazione_enorme() {
        let (sender, receiver) = fs_event_channel();
        let mut error = String::with_capacity(1 << 20);
        error.push_str("errore");
        sender.send(FsEvent::Error(error)).unwrap();
        let FsEvent::Error(error) = receiver.recv().unwrap() else {
            panic!("errore atteso")
        };
        assert_eq!(error, "errore");
        assert!(error.capacity() <= ERROR_BYTES);
    }
    #[test]
    fn size_hint_conta_slot_e_limite_eof() {
        let (sender, receiver) = fs_event_channel();
        assert_eq!(receiver.size_hint(), (0, None));
        sender.send(FsEvent::Rescan).unwrap();
        sender.send(FsEvent::Error("corrente".into())).unwrap();
        assert_eq!(receiver.size_hint(), (2, None));
        drop(sender);
        assert_eq!(receiver.size_hint(), (2, Some(2)));
        receiver.recv().unwrap();
        assert_eq!(receiver.size_hint(), (1, Some(1)));
        receiver.recv().unwrap();
        assert_eq!(receiver.size_hint(), (0, Some(0)));
    }

    struct DropObserver {
        shared: std::sync::Weak<Shared>,
        dropped: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl std::task::Wake for DropObserver {
        fn wake(self: Arc<Self>) {
            if let Some(shared) = self.shared.upgrade() {
                assert!(shared.state.try_lock().is_ok(), "wake Waker sotto mutex");
            }
        }
    }
    impl Drop for DropObserver {
        fn drop(&mut self) {
            if let Some(shared) = self.shared.upgrade() {
                assert!(shared.state.try_lock().is_ok(), "drop Waker sotto mutex");
            }
            self.dropped
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    #[test]
    fn sostituzione_waker_droppa_callback_fuori_mutex() {
        let (_sender, mut receiver) = fs_event_channel();
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let waker = Waker::from(Arc::new(DropObserver {
                shared: Arc::downgrade(&receiver.0),
                dropped: Arc::clone(&dropped),
            }));
            assert!(
                receiver
                    .poll_next(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        }
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(
            receiver
                .poll_next(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    #[test]
    fn send_esegue_wake_e_drop_del_waker_fuori_mutex() {
        let (sender, mut receiver) = fs_event_channel();
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let waker = Waker::from(Arc::new(DropObserver {
                shared: Arc::downgrade(&receiver.0),
                dropped: Arc::clone(&dropped),
            }));
            assert!(
                receiver
                    .poll_next(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        }
        sender.send(FsEvent::Rescan).unwrap();
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(receiver.try_recv(), Ok(FsEvent::Rescan));
    }
    struct Observer {
        shared: Arc<Shared>,
        wakes: std::sync::atomic::AtomicUsize,
    }
    impl std::task::Wake for Observer {
        fn wake(self: Arc<Self>) {
            assert!(self.shared.state.try_lock().is_ok(), "wake sotto mutex");
            self.wakes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    #[test]
    fn poll_registra_wake_fuori_mutex_e_drena_prima_di_eof() {
        let (sender, mut receiver) = fs_event_channel();
        let observer = Arc::new(Observer {
            shared: Arc::clone(&receiver.0),
            wakes: std::sync::atomic::AtomicUsize::new(0),
        });
        let waker = Waker::from(Arc::clone(&observer));
        let mut cx = Context::from_waker(&waker);
        assert!(receiver.poll_next(&mut cx).is_pending());
        sender.send(FsEvent::Rescan).unwrap();
        assert_eq!(observer.wakes.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(sender);
        assert_eq!(
            receiver.poll_next(&mut cx),
            Poll::Ready(Some(FsEvent::Rescan))
        );
        assert_eq!(receiver.poll_next(&mut cx), Poll::Ready(None));
    }
    #[test]
    fn sender_drop_notifica_poll_in_attesa() {
        let (sender, mut receiver) = fs_event_channel();
        let observer = Arc::new(Observer {
            shared: Arc::clone(&receiver.0),
            wakes: std::sync::atomic::AtomicUsize::new(0),
        });
        let waker = Waker::from(Arc::clone(&observer));
        let mut cx = Context::from_waker(&waker);
        assert!(receiver.poll_next(&mut cx).is_pending());
        drop(sender);
        assert_eq!(observer.wakes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(receiver.poll_next(&mut cx), Poll::Ready(None));
    }
    #[test]
    fn consumer_fermo_limita_paths_errori_e_drena_dopo_eof() {
        let (sender, receiver) = fs_event_channel();
        for i in 0..10000 {
            sender
                .send(FsEvent::Changed(vec![PathBuf::from(format!("file-{i}"))]))
                .unwrap();
            sender.send(FsEvent::Error(format!("errore-{i}"))).unwrap();
        }
        drop(sender);
        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert!(events.contains(&FsEvent::Rescan));
        assert!(events.contains(&FsEvent::Error("errore-9999".into())));
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }
    #[test]
    fn errori_continui_non_affamano_rescan_e_utf8_resta_limitato() {
        let (sender, receiver) = fs_event_channel();
        sender.send(FsEvent::Rescan).unwrap();
        sender.send(FsEvent::Error("é".repeat(10000))).unwrap();
        let FsEvent::Error(error) = receiver.recv().unwrap() else {
            panic!("errore atteso")
        };
        assert!(error.len() <= ERROR_BYTES && error.capacity() <= ERROR_BYTES);
        assert!(error.ends_with("… [troncato]"));
        sender.send(FsEvent::Error("nuovo".into())).unwrap();
        assert_eq!(receiver.recv().unwrap(), FsEvent::Rescan);
        assert_eq!(receiver.recv().unwrap(), FsEvent::Error("nuovo".into()));
    }
    #[test]
    fn drop_receiver_revoca_send_e_timeout_non_e_eof() {
        let (sender, receiver) = fs_event_channel();
        assert_eq!(
            receiver.recv_timeout(Duration::ZERO),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        drop(receiver);
        assert_eq!(sender.send(FsEvent::Rescan), Err(FsEvent::Rescan));
    }
}
