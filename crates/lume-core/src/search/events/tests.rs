//! Engine/thread/TempDir veri oppure prove esplicitamente del solo canale.
//! Nessun filesystem simulato, executor, socket o driver fake.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::{
    future::Future,
    sync::{atomic::AtomicUsize, mpsc},
    task::{Wake, Waker},
    time::Duration,
};
const DEADLINE: Duration = Duration::from_secs(2);

struct WakeSignal {
    sender: mpsc::Sender<()>,
    count: AtomicUsize,
}
impl Wake for WakeSignal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.count.fetch_add(1, Ordering::SeqCst);
        let _ = self.sender.send(());
    }
}
fn signal() -> (Arc<WakeSignal>, mpsc::Receiver<()>) {
    let (sender, receiver) = mpsc::channel();
    (
        Arc::new(WakeSignal {
            sender,
            count: AtomicUsize::new(0),
        }),
        receiver,
    )
}
fn fixture() -> (tempfile::TempDir, SearchRun) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for index in 0..200 {
        let path = dir.path().join(format!("query-{index:03}.txt"));
        std::fs::write(&path, "ago é reale\n").unwrap();
        paths.push(path);
    }
    let run = spawn_search_owned(
        SearchScope::Aperti(paths),
        SearchOptions::literal("ago"),
        Arc::new(AtomicU64::new(4)),
        4,
    )
    .unwrap();
    (dir, run)
}
fn waiting(events: &SearchEvents) -> (bool, usize) {
    let pipe = &events.origin.pipe;
    let (state, _) = pipe
        .space
        .wait_timeout_while(lock(pipe), DEADLINE, |state| {
            !state.producer_waiting && !state.producer_closed
        })
        .unwrap();
    (state.producer_waiting, state.queue.len())
}
fn collect(events: &mut SearchEvents) -> Vec<SearchUpdate> {
    let (signal, notifications) = signal();
    let waker = Waker::from(signal);
    let mut cx = Context::from_waker(&waker);
    let mut out = Vec::new();
    loop {
        // Si guida la vera future std next(), non un receiver di prova che
        // replica i dati del motore.
        let mut next = std::pin::pin!(events.next());
        let update = loop {
            match next.as_mut().poll(&mut cx) {
                Poll::Ready(update) => break update,
                Poll::Pending => notifications.recv_timeout(DEADLINE).unwrap(),
            }
        };
        if let Some(update) = update {
            out.push(update);
        } else {
            return out;
        }
    }
}
fn join(worker: std::thread::JoinHandle<()>) {
    // Il collector possiede realmente l'handle; nessun join sul consumer UI.
    let (sender, receiver) = mpsc::sync_channel(1);
    let collector = std::thread::spawn(move || {
        let _ = sender.send(worker.join());
    });
    let result = receiver
        .recv_timeout(DEADLINE)
        .expect("producer da raccogliere entro deadline");
    collector.join().unwrap();
    result.unwrap();
}

#[test]
fn engine_non_drenato_ferma_producer_a_due_eventi_non_a_done() {
    let (_dir, run) = fixture();
    let (blocked, queued) = waiting(&run.events);
    let finished = run.worker.is_finished();
    // Cleanup prima delle assert anche quando il bound è ancora difettoso.
    drop(run.events);
    join(run.worker);
    assert_eq!(
        queued, SEARCH_EVENT_CAPACITY,
        "coda deve essere bounded anche se il consumer non polla"
    );
    assert!(
        blocked && !finished,
        "producer deve attendere spazio, non completare accumulando eventi"
    );
}

#[test]
fn drop_receiver_reale_sveglia_producer_pieno_e_handle_viene_raccolto() {
    let (_dir, run) = fixture();
    let (blocked, _) = waiting(&run.events);
    let origin = run.events.origin();
    drop(run.events);
    join(run.worker);
    assert!(
        blocked,
        "il test deve partire con un producer veramente bloccato"
    );
    assert!(!origin.is_current());
    assert!(lock(&origin.pipe).queue.is_empty());
}

#[test]
fn cancel_notificato_raccoglie_engine_pieno_senza_drain_ne_timer() {
    let (_dir, mut run) = fixture();
    let (blocked, _) = waiting(&run.events);
    run.events.origin().cancel();
    join(run.worker);
    assert!(
        collect(&mut run.events).is_empty(),
        "cancellazione non deve fabbricare Done"
    );
    assert!(blocked, "producer pieno reale richiesto");
}

#[test]
fn drain_engine_conserva_batch_cap_testo_unicode_e_done_ed_eof() {
    let (dir, mut run) = fixture();
    let updates = collect(&mut run.events);
    join(run.worker);
    let mut files = 0;
    let mut hits = 0;
    let mut batches = 0;
    for update in &updates {
        if let SearchUpdate::Batch(batch) = update {
            batches += 1;
            assert_eq!(batch.len(), 20);
            for file in batch {
                files += 1;
                assert!(file.path.starts_with(dir.path()));
                for hit in &file.hits {
                    hits += 1;
                    assert_eq!(hit.line_text.as_ref(), "ago é reale");
                    assert_eq!(hit.line_number, 1);
                    assert_eq!(hit.range_in_line, 0..3);
                }
            }
        }
    }
    assert_eq!((files, hits, batches), (200, 200, 10));
    assert!(matches!(
        updates.last(),
        Some(SearchUpdate::Done {
            files: 200,
            matches: 200,
            truncated: false
        })
    ));
    // Fonte fused anche dopo EOF, senza panic unfold o wake periodici.
    assert!(collect(&mut run.events).is_empty());
}

#[test]
fn engine_regex_invalida_e_un_solo_errore_reale_non_done() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = spawn_search_owned(
        SearchScope::Albero(dir.path().to_path_buf()),
        SearchOptions {
            needle: "(".into(),
            regex: true,
            ..SearchOptions::default()
        },
        Arc::new(AtomicU64::new(1)),
        1,
    )
    .unwrap();
    let updates = collect(&mut run.events);
    join(run.worker);
    assert!(matches!(updates.as_slice(), [SearchUpdate::Error(error)] if !error.is_empty()));
}

#[test]
fn engine_pre_cancellato_non_pubblica_done_neanche_su_scope_vuoto() {
    let dir = tempfile::tempdir().unwrap();
    for scope in [
        SearchScope::Aperti(Vec::new()),
        SearchScope::Albero(dir.path().to_path_buf()),
        SearchScope::Sorgenti(dir.path().to_path_buf()),
    ] {
        let mut run = spawn_search_owned(
            scope,
            SearchOptions::literal("ago"),
            Arc::new(AtomicU64::new(2)),
            1,
        )
        .unwrap();
        assert!(collect(&mut run.events).is_empty());
        join(run.worker);
    }
}

#[test]
fn engine_cancel_vecchio_non_revoca_worker_nuovo_con_stessa_generation() {
    let (dir, old) = fixture();
    let (blocked, _) = waiting(&old.events);
    let generation = Arc::clone(&old.events.origin.pipe.generation);
    let mut new = spawn_search_owned(
        SearchScope::Albero(dir.path().to_path_buf()),
        SearchOptions::literal("ago"),
        Arc::clone(&generation),
        4,
    )
    .unwrap();
    old.events.origin().cancel();
    join(old.worker);
    let updates = collect(&mut new.events);
    join(new.worker);
    assert!(blocked);
    assert_eq!(generation.load(Ordering::SeqCst), 4);
    assert!(matches!(
        updates.last(),
        Some(SearchUpdate::Done {
            files: 200,
            matches: 200,
            truncated: false
        })
    ));
}

#[test]
fn canale_source_silente_svegliata_da_cancel_esplicito() {
    let generation = Arc::new(AtomicU64::new(7));
    let (sender, mut events) = channel(Arc::clone(&generation), 7);
    let origin = events.origin();
    let (signal, notifications) = signal();
    let waker = Waker::from(Arc::clone(&signal));
    let mut cx = Context::from_waker(&waker);
    assert!(events.poll_next(&mut cx).is_pending());
    // AtomicU64 pubblico da solo non ha un meccanismo di notifica.
    generation.fetch_add(1, Ordering::SeqCst);
    assert!(notifications.try_recv().is_err());
    origin.cancel();
    notifications.recv_timeout(DEADLINE).unwrap();
    assert!(matches!(events.poll_next(&mut cx), Poll::Ready(None)));
    assert!(!origin.is_current());
    drop(sender); // rimasto silente/vivo fino a dopo il wake
}

#[test]
fn canale_pending_non_genera_wake_automatici_e_eof_sveglia() {
    let (sender, mut events) = channel(Arc::new(AtomicU64::new(1)), 1);
    let (signal, notifications) = signal();
    let waker = Waker::from(Arc::clone(&signal));
    let mut cx = Context::from_waker(&waker);
    for _ in 0..100 {
        assert!(events.poll_next(&mut cx).is_pending());
    }
    assert_eq!(signal.count.load(Ordering::SeqCst), 0);
    drop(sender);
    notifications.recv_timeout(DEADLINE).unwrap();
    assert!(matches!(events.poll_next(&mut cx), Poll::Ready(None)));
    let count = signal.count.load(Ordering::SeqCst);
    for _ in 0..100 {
        assert!(matches!(events.poll_next(&mut cx), Poll::Ready(None)));
    }
    assert_eq!(signal.count.load(Ordering::SeqCst), count);
}

#[test]
fn canale_send_sveglia_future_e_revoca_prima_del_poll_filtra_residui() {
    let (sender, mut events) = channel(Arc::new(AtomicU64::new(1)), 1);
    let (signal, notifications) = signal();
    let waker = Waker::from(signal);
    let mut cx = Context::from_waker(&waker);
    assert!(events.poll_next(&mut cx).is_pending());
    sender
        .send(SearchUpdate::Error("errore del canale reale".into()))
        .unwrap();
    notifications.recv_timeout(DEADLINE).unwrap();
    events.origin().cancel();
    assert!(matches!(events.poll_next(&mut cx), Poll::Ready(None)));
}

#[test]
fn canale_cancel_solo_questo_consumer_non_blocca_nuova_ricerca_stessa_gen() {
    let generation = Arc::new(AtomicU64::new(9));
    let (old_sender, old_events) = channel(Arc::clone(&generation), 9);
    let old_origin = old_events.origin();
    drop(old_events);
    let (new_sender, mut new_events) = channel(Arc::clone(&generation), 9);
    assert!(!old_origin.is_current());
    assert!(new_events.origin().is_current());
    new_sender
        .send(SearchUpdate::Done {
            files: 0,
            matches: 0,
            truncated: false,
        })
        .unwrap();
    drop(new_sender);
    assert_eq!(
        collect(&mut new_events),
        vec![SearchUpdate::Done {
            files: 0,
            matches: 0,
            truncated: false
        }]
    );
    assert_eq!(generation.load(Ordering::SeqCst), 9);
    drop(old_sender);
}

#[test]
fn canale_cancel_rilascia_payload_e_capacita_non_solo_contatore() {
    let (sender, events) = channel(Arc::new(AtomicU64::new(1)), 1);
    let text: Arc<str> = "testo del payload".into();
    let weak = Arc::downgrade(&text);
    sender
        .send(SearchUpdate::Batch(vec![FileMatches {
            path: PathBuf::from("documento.txt"),
            hits: vec![SearchHit {
                line_number: 1,
                line_text: text,
                range_in_line: 0..5,
            }],
        }]))
        .unwrap();
    events.origin().cancel();
    assert!(weak.upgrade().is_none());
    assert_eq!(lock(&events.origin.pipe).queue.capacity(), 0);
}

struct CancelOnWakerDrop {
    origin: SearchOrigin,
    under_lock: Arc<std::sync::atomic::AtomicBool>,
}
impl Wake for CancelOnWakerDrop {
    fn wake(self: Arc<Self>) {
        self.origin.cancel();
    }
}
impl Drop for CancelOnWakerDrop {
    fn drop(&mut self) {
        // Probe del lock reale, non reentry che renderebbe il rosso un hang
        // senza cleanup. Sul percorso sicuro esegue davvero cancel.
        if self.origin.pipe.state.try_lock().is_err() {
            self.under_lock.store(true, Ordering::SeqCst);
            return;
        }
        self.origin.cancel();
    }
}
#[test]
fn canale_drop_waker_ritirato_fuori_mutex_puo_revocare_source() {
    for stale in [false, true] {
        let generation = Arc::new(AtomicU64::new(1));
        let (_sender, mut events) = channel(Arc::clone(&generation), 1);
        let under_lock = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = Arc::new(CancelOnWakerDrop {
            origin: events.origin(),
            under_lock: Arc::clone(&under_lock),
        });
        {
            let waker = Waker::from(Arc::clone(&probe));
            let mut cx = Context::from_waker(&waker);
            assert!(events.poll_next(&mut cx).is_pending());
        }
        drop(probe); // l'ultimo riferimento è adesso il waker registrato
        if stale {
            generation.fetch_add(1, Ordering::SeqCst);
        }
        let (signal, _) = signal();
        let waker = Waker::from(signal);
        let mut cx = Context::from_waker(&waker);
        let _ = events.poll_next(&mut cx); // sostituzione oppure scope revocato
        assert!(
            !under_lock.load(Ordering::SeqCst),
            "drop waker non deve avvenire sotto mutex"
        );
        assert!(
            !events.origin().is_current(),
            "drop waker deve poter fare cancel senza deadlock"
        );
    }
}

#[test]
fn canale_panic_producer_da_join_non_si_scambia_per_successo() {
    let (sender, mut events) = channel(Arc::new(AtomicU64::new(1)), 1);
    let producer = std::thread::spawn(move || {
        let _sender = sender;
        panic!("panic reale del producer di protocollo, non dell'engine");
    });
    assert!(collect(&mut events).is_empty());
    assert!(
        producer.join().is_err(),
        "il caller deve riportare panic/EOF incompleto, non inventare Done"
    );
}
