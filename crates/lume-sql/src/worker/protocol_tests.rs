//! Protocollo del worker: thread/mutex/canali veri, nessun Driver o database.
//! Non prova autenticazione, SQL, read-only o integrazione MCP.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::time::Duration;

pub(super) fn shared() -> Arc<Shared> {
    Arc::new(Shared {
        mailbox: Mutex::new(Mailbox::default()),
        wake: Condvar::new(),
        shutdown: AtomicBool::new(false),
        state: AtomicU8::new(State::Connecting.as_u8()),
        error: Mutex::new(None),
        auth_failed: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        interrupter: Mutex::new(None),
        cancelled: AtomicBool::new(false),
        query_control: Mutex::new(QueryControl::default()),
        query_wake: Condvar::new(),
        event_wake: Arc::new(tokio::sync::Notify::new()),
    })
}
/// Solo handle del protocollo, nessun open/factory/driver o thread DB.
pub(super) fn handle(shared: Arc<Shared>) -> Connection {
    Connection {
        shared,
        thread: None,
        profile: ConnectionProfile {
            id: "protocollo-unit".into(),
            label: "protocollo unitario".into(),
            engine: crate::driver::Engine::AzureSql,
            host: String::new(),
            database: String::new(),
            auth: crate::driver::AuthKind::SqlLogin {
                user: String::new(),
            },
        },
    }
}
fn batch_once(
    shared: &Shared,
    events: SyncSender<QueryEvent>,
    generation: &Arc<AtomicU64>,
) -> Result<QueryFlow, SqlError> {
    let mut sink = EventSink {
        events: events.into(),
        generation,
        expected_generation: 7,
        rows: 0,
        accounted_bytes: 0,
        budget_reached: false,
        shared,
        byte_limit: usize::MAX,
        row_limit: 1,
        stopped: false,
    };
    sink.batch(QueryBatch::new(vec![Vec::new()]))
}

#[test]
fn budget_non_pubblicato_prima_che_il_controllo_sia_armato() {
    let shared = shared();
    let locked = shared.query_control.lock().unwrap();
    let (events, receiver) = sync_channel(2);
    let worker_shared = shared.clone();
    let worker =
        thread::spawn(move || batch_once(&worker_shared, events, &Arc::new(AtomicU64::new(7))));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)),
        Ok(QueryEvent::Batch(_))
    ));
    // Il lock impedisce di armare la pausa. Prima della correzione l'evento
    // arrivava comunque: recv attende il produttore, non usa uno sleep/rerun.
    let premature = receiver.recv_timeout(Duration::from_millis(200));
    shared.shutdown.store(true, Ordering::Release);
    drop(locked);
    shared.query_wake.notify_all();
    let _ = worker.join().unwrap();
    assert!(
        matches!(premature, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
        "BudgetReached non deve precedere controllo pronto"
    );
    assert!(shared.query_control.lock().unwrap().generation.is_none());
}
#[test]
fn stop_e_continue_subito_dopo_budget_non_si_perdono() {
    for command in [QueryCommand::Stop, QueryCommand::Continue] {
        let shared = shared();
        let (events, receiver) = sync_channel(0);
        let worker_shared = shared.clone();
        let worker =
            thread::spawn(move || batch_once(&worker_shared, events, &Arc::new(AtomicU64::new(7))));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Ok(QueryEvent::Batch(_))
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Ok(QueryEvent::BudgetReached { .. })
        ));
        let connection = handle(shared.clone());
        let armed = shared.query_control.lock().unwrap().generation == Some(7);
        let accepted = match command {
            QueryCommand::Stop => connection.stop_query(7),
            QueryCommand::Continue => connection.continue_query(7),
        };
        if accepted.is_err() {
            shared.shutdown.store(true, Ordering::Release);
            shared.query_wake.notify_all();
        }
        let result = worker.join().unwrap();
        assert!(armed, "BudgetReached deve garantire controllo pronto");
        assert!(accepted.is_ok());
        let result = result.unwrap();
        assert!(matches!(
            (command, result),
            (QueryCommand::Stop, QueryFlow::Stop) | (QueryCommand::Continue, QueryFlow::Continue)
        ));
        let control = shared.query_control.lock().unwrap();
        assert!(control.generation.is_none());
        assert!(control.command.is_none());
    }
}
#[test]
fn chiusura_receiver_durante_pubblicazione_disarma_la_pausa() {
    let shared = shared();
    let locked = shared.query_control.lock().unwrap();
    let (events, receiver) = sync_channel(2);
    let worker_shared = shared.clone();
    let worker =
        thread::spawn(move || batch_once(&worker_shared, events, &Arc::new(AtomicU64::new(7))));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)),
        Ok(QueryEvent::Batch(_))
    ));
    drop(receiver);
    drop(locked);
    // La chiusura deve completare senza shutdown. Se il vecchio wait resta
    // bloccato, il canale terminale scade e cleanup risveglia il thread.
    let (done, completed) = std::sync::mpsc::channel();
    let cleanup = shared.clone();
    let reaper = thread::spawn(move || {
        done.send(worker.join().unwrap()).unwrap();
    });
    let result = completed.recv_timeout(Duration::from_secs(2));
    if result.is_err() {
        cleanup.shutdown.store(true, Ordering::Release);
        cleanup.query_wake.notify_all();
    }
    reaper.join().unwrap();
    assert!(result.unwrap().is_err());
    let control = shared.query_control.lock().unwrap();
    assert!(control.generation.is_none());
    assert!(control.command.is_none());
}
#[test]
fn lavoro_pendente_prima_della_pausa_non_rimane_dietro_il_cursore() {
    let shared = shared();
    let generation = Arc::new(AtomicU64::new(7));
    let (sender, _) = oneshot::channel();
    shared.mailbox.lock().unwrap().pending = Some(Job {
        work: Work::Request {
            request: Request::Tables,
            sender,
        },
        generation: generation.clone(),
        my_generation: 7,
    });
    let (events, receiver) = sync_channel(2);
    let worker_shared = shared.clone();
    let worker = thread::spawn(move || batch_once(&worker_shared, events, &generation));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)),
        Ok(QueryEvent::Batch(_))
    ));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)),
        Ok(QueryEvent::BudgetReached { .. })
    ));
    // Vecchio backend resterebbe in wait. Shutdown è soltanto cleanup del
    // caso rosso: il risultato deve essere Stop, non Unavailable.
    shared.shutdown.store(true, Ordering::Release);
    shared.query_wake.notify_all();
    assert!(matches!(worker.join().unwrap(), Ok(QueryFlow::Stop)));
}

#[test]
fn controllo_rifiuta_generazione_stantia_e_continue_non_resuscita_stop() {
    let shared = shared();
    let connection = handle(shared.clone());
    shared.query_control.lock().unwrap().generation = Some(7);
    assert!(connection.stop_query(6).is_err());
    assert!(connection.continue_query(6).is_err());
    assert!(shared.query_control.lock().unwrap().command.is_none());
    connection.stop_query(7).unwrap();
    connection.stop_query(7).unwrap();
    assert!(connection.continue_query(7).is_err());
    assert!(matches!(
        shared.query_control.lock().unwrap().command,
        Some(QueryCommand::Stop)
    ));
}

#[test]
fn enqueue_ferma_solo_il_cursore_davanti_a_un_job_non_consumato() {
    let shared = shared();
    let connection = handle(shared.clone());
    shared.query_control.lock().unwrap().generation = Some(7);
    let response = connection
        .submit(Request::Tables, Arc::new(AtomicU64::new(8)), 8)
        .unwrap();
    assert!(matches!(
        shared.query_control.lock().unwrap().command,
        Some(QueryCommand::Stop)
    ));
    let job = shared.mailbox.lock().unwrap().pending.take().unwrap();
    assert_eq!(job.my_generation, 8);
    // Il worker ha già preso il job. Un interrupt ritardato non deve fermare
    // la sua nuova pausa, anche se questa è armata prima del notify.
    {
        let mut control = shared.query_control.lock().unwrap();
        control.generation = Some(8);
        control.command = None;
    }
    connection.interrupt_paused_query();
    assert!(shared.query_control.lock().unwrap().command.is_none());
    drop(job);
    assert!(futures::executor::block_on(response).is_err());
}
