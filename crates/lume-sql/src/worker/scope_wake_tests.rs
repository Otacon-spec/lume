//! Lifetime del receiver async senza Driver/factory/socket: il producer del
//! canale rimane vivo e SILENZIOSO, quindi EOF non può mascherare il problema.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::protocol_tests::{handle, shared};
use super::*;
use futures::{
    Stream,
    task::{ArcWake, waker},
};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
struct WakeCount(AtomicU64);
impl ArcWake for WakeCount {
    fn wake_by_ref(wake: &Arc<Self>) {
        wake.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn source() -> (Connection, QueryEvents, EventSender, Arc<AtomicU64>) {
    let shared = shared();
    let connection = handle(shared.clone());
    let generation = Arc::new(AtomicU64::new(7));
    let receiver = connection
        .submit_query_events(String::new(), generation.clone(), 7)
        .unwrap();
    let producer = shared.mailbox.lock().unwrap().pending.take().unwrap().work;
    let Work::Stream {
        events: producer, ..
    } = producer
    else {
        panic!("stream atteso")
    };
    (connection, receiver, producer, generation)
}
fn park(receiver: &mut QueryEvents, context: &mut Context<'_>, count: &WakeCount) -> u64 {
    // Prima consumare anche l'eventuale fronte benigno del proprio enqueue.
    assert!(Pin::new(&mut *receiver).poll_next(context).is_pending());
    assert!(Pin::new(receiver).poll_next(context).is_pending());
    count.0.load(Ordering::SeqCst)
}
#[test]
fn shutdown_sveglia_receiver_vuoto_anche_con_producer_vivo_silenzioso() {
    let (connection, mut receiver, producer, _) = source();
    let count = Arc::new(WakeCount(AtomicU64::new(0)));
    let wake = waker(count.clone());
    let mut context = Context::from_waker(&wake);
    let before = park(&mut receiver, &mut context, &count);
    drop(connection);
    assert!(
        count.0.load(Ordering::SeqCst) > before,
        "shutdown deve svegliare la future senza evento, EOF o polling"
    );
    assert!(matches!(
        Pin::new(&mut receiver).poll_next(&mut context),
        Poll::Ready(None)
    ));
    assert!(producer.is_closed());
}
#[test]
fn nuova_richiesta_sveglia_la_generazione_superata_anche_senza_eventi() {
    let (connection, mut receiver, producer, generation) = source();
    let count = Arc::new(WakeCount(AtomicU64::new(0)));
    let wake = waker(count.clone());
    let mut context = Context::from_waker(&wake);
    let before = park(&mut receiver, &mut context, &count);
    let next = generation.fetch_add(1, Ordering::SeqCst) + 1;
    drop(
        connection
            .submit(Request::Tables, generation, next)
            .unwrap(),
    );
    assert!(
        count.0.load(Ordering::SeqCst) > before,
        "enqueue deve svegliare receiver superato"
    );
    assert!(matches!(
        Pin::new(&mut receiver).poll_next(&mut context),
        Poll::Ready(None)
    ));
    assert!(producer.is_closed());
}
#[test]
fn notifica_benigna_riarma_una_volta_poi_resta_quieta() {
    let (connection, mut receiver, _producer, _) = source();
    let count = Arc::new(WakeCount(AtomicU64::new(0)));
    let wake = waker(count.clone());
    let mut context = Context::from_waker(&wake);
    let before = park(&mut receiver, &mut context, &count);
    connection.shared.event_wake.notify_waiters();
    assert!(count.0.load(Ordering::SeqCst) > before);
    let settled = park(&mut receiver, &mut context, &count);
    for _ in 0..100 {
        assert!(Pin::new(&mut receiver).poll_next(&mut context).is_pending());
    }
    assert_eq!(
        count.0.load(Ordering::SeqCst),
        settled,
        "nessuna rischedulazione ripetuta da source quieta"
    );
}
#[test]
fn shutdown_risveglia_tutti_i_receiver_non_solo_un_waiter() {
    let (connection, mut first, _producer, _) = source();
    let mut second = connection
        .submit_query_events(String::new(), Arc::new(AtomicU64::new(7)), 7)
        .unwrap();
    let second_producer = connection
        .shared
        .mailbox
        .lock()
        .unwrap()
        .pending
        .take()
        .unwrap()
        .work;
    let Work::Stream {
        events: second_producer,
        ..
    } = second_producer
    else {
        panic!("stream atteso")
    };
    let one = Arc::new(WakeCount(AtomicU64::new(0)));
    let two = Arc::new(WakeCount(AtomicU64::new(0)));
    let w1 = waker(one.clone());
    let w2 = waker(two.clone());
    let mut c1 = Context::from_waker(&w1);
    let mut c2 = Context::from_waker(&w2);
    let b1 = park(&mut first, &mut c1, &one);
    let b2 = park(&mut second, &mut c2, &two);
    drop(connection);
    assert!(one.0.load(Ordering::SeqCst) > b1 && two.0.load(Ordering::SeqCst) > b2);
    assert!(matches!(
        Pin::new(&mut first).poll_next(&mut c1),
        Poll::Ready(None)
    ));
    assert!(matches!(
        Pin::new(&mut second).poll_next(&mut c2),
        Poll::Ready(None)
    ));
    assert!(second_producer.is_closed());
}
#[test]
fn abbandono_del_singolo_scope_libera_receiver_ma_non_la_connessione() {
    let (connection, mut receiver, producer, _) = source();
    let count = Arc::new(WakeCount(AtomicU64::new(0)));
    let wake = waker(count.clone());
    let mut context = Context::from_waker(&wake);
    let before = park(&mut receiver, &mut context, &count);
    receiver.origin().abandon();
    assert!(count.0.load(Ordering::SeqCst) > before);
    assert!(!connection.shared.shutdown.load(Ordering::Acquire));
    assert!(matches!(
        Pin::new(&mut receiver).poll_next(&mut context),
        Poll::Ready(None)
    ));
    assert!(producer.is_closed());
    let _next = connection
        .submit_query_events(String::new(), Arc::new(AtomicU64::new(8)), 8)
        .unwrap();
}
#[test]
fn shutdown_prima_del_poll_non_consegna_eventi_residui() {
    let (connection, mut receiver, producer, _) = source();
    producer
        .try_send(QueryEvent::Started {
            columns: Vec::new(),
        })
        .unwrap();
    drop(connection);
    let wake = futures::task::noop_waker();
    let mut context = Context::from_waker(&wake);
    assert!(
        matches!(
            Pin::new(&mut receiver).poll_next(&mut context),
            Poll::Ready(None)
        ),
        "evento residuo non deve attraversare scope già chiuso"
    );
    assert!(producer.is_closed());
}
