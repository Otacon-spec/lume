//! Trasporto async reale senza Driver/factory/socket.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::protocol_tests::{handle, shared};
use super::*;
use futures::StreamExt;
#[test]
fn queue_async_bounded_e_origine_invalida_al_drop_connessione() {
    let shared = shared();
    let connection = handle(shared.clone());
    let generation = Arc::new(AtomicU64::new(7));
    let mut receiver = connection
        .submit_query_events(String::new(), generation, 7)
        .unwrap();
    let origin = receiver.origin();
    let sender = shared.mailbox.lock().unwrap().pending.take().unwrap().work;
    let Work::Stream { events: sender, .. } = sender else {
        panic!("stream atteso")
    };
    sender
        .try_send(QueryEvent::Started {
            columns: Vec::new(),
        })
        .unwrap();
    sender
        .try_send(QueryEvent::Batch(QueryBatch::new(vec![Vec::new()])))
        .unwrap();
    assert!(matches!(
        sender.try_send(QueryEvent::BudgetReached {
            rows: 1,
            accounted_bytes: 0
        }),
        Err(TrySendError::Full(_))
    ));
    assert!(origin.is_current());
    assert!(matches!(
        futures::executor::block_on(receiver.next()),
        Some(QueryEvent::Started { .. })
    ));
    drop(connection);
    assert!(!origin.is_current());
}
#[test]
fn drop_receiver_async_sveglia_pausa_senza_drop_connessione() {
    let shared = shared();
    let connection = handle(shared.clone());
    let generation = Arc::new(AtomicU64::new(7));
    let mut receiver = connection
        .submit_query_events(String::new(), generation.clone(), 7)
        .unwrap();
    let origin = receiver.origin();
    let events = shared.mailbox.lock().unwrap().pending.take().unwrap().work;
    let Work::Stream { events, .. } = events else {
        panic!("stream atteso")
    };
    let worker_shared = shared.clone();
    let (sender, done) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let mut sink = EventSink {
            events,
            generation: &generation,
            expected_generation: 7,
            rows: 0,
            accounted_bytes: 0,
            budget_reached: false,
            shared: &worker_shared,
            byte_limit: usize::MAX,
            row_limit: 1,
            stopped: false,
        };
        sender
            .send(sink.batch(QueryBatch::new(vec![Vec::new()])))
            .unwrap();
    });
    futures::executor::block_on(async {
        assert!(matches!(receiver.next().await, Some(QueryEvent::Batch(_))));
        assert!(matches!(
            receiver.next().await,
            Some(QueryEvent::BudgetReached { .. })
        ));
    });
    drop(receiver);
    assert!(!origin.is_current());
    let result = done.recv_timeout(std::time::Duration::from_secs(2));
    if result.is_err() {
        drop(connection);
    }
    worker.join().unwrap();
    assert!(
        result.is_ok(),
        "receiver chiuso deve svegliare la pausa senza shutdown"
    );
    assert!(shared.query_control.lock().unwrap().generation.is_none());
}
