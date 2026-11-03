//! Canali pieni con receiver vivo: sincronizzazione vera, nessun Driver/DB.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::protocol_tests::{handle, shared};
use super::*;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum Trigger {
    DropConnection,
    Generation,
    Pending,
    Stop,
}
fn full_queue(trigger: Trigger) {
    let shared = shared();
    let mut connection = Some(handle(shared.clone()));
    let generation = Arc::new(AtomicU64::new(7));
    let (events, receiver) = sync_channel(2);
    events
        .send(QueryEvent::Started {
            columns: Vec::new(),
        })
        .unwrap();
    events
        .send(QueryEvent::Batch(QueryBatch::new(vec![Vec::new()])))
        .unwrap();
    if matches!(trigger, Trigger::Stop) {
        shared.query_control.lock().unwrap().generation = Some(7);
    }
    let (entered, started) = std::sync::mpsc::channel();
    let (done, completed) = std::sync::mpsc::channel();
    let worker_shared = shared.clone();
    let worker_generation = generation.clone();
    let worker = thread::spawn(move || {
        let mut sink = EventSink {
            events: events.into(),
            generation: &worker_generation,
            expected_generation: 7,
            rows: 0,
            accounted_bytes: 0,
            budget_reached: false,
            shared: &worker_shared,
            byte_limit: usize::MAX,
            row_limit: usize::MAX,
            stopped: false,
        };
        entered.send(()).unwrap();
        let result = sink.send(QueryEvent::BudgetReached {
            rows: 1,
            accounted_bytes: 0,
        });
        done.send((result, sink.stopped)).unwrap();
    });
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    match trigger {
        Trigger::DropConnection => drop(connection.take()),
        Trigger::Generation => {
            generation.store(8, Ordering::Release);
        }
        Trigger::Pending => {
            drop(
                connection
                    .as_ref()
                    .unwrap()
                    .submit(Request::Tables, Arc::new(AtomicU64::new(8)), 8)
                    .unwrap(),
            );
        }
        Trigger::Stop => connection.as_ref().unwrap().stop_query(7).unwrap(),
    }
    let result = completed.recv_timeout(Duration::from_millis(500));
    // Cleanup anche nel rosso: il receiver resta VIVO durante l'asserzione
    // temporale, poi chiuderlo permette sempre join, senza thread staccati.
    drop(receiver);
    worker.join().unwrap();
    assert!(
        result.is_ok(),
        "{trigger:?}: producer bloccato malgrado richiesta di uscita, receiver vivo e nessun drain"
    );
    let (outcome, stopped) = result.unwrap();
    assert!(outcome.is_err());
    if matches!(trigger, Trigger::Stop) {
        assert!(stopped, "Stop dev'essere riconoscibile al sink");
    }
}
#[test]
fn drop_non_aspetta_drain_di_un_receiver_aperto() {
    full_queue(Trigger::DropConnection);
}
#[test]
fn generazione_superata_non_resta_in_send_pieno() {
    full_queue(Trigger::Generation);
}
#[test]
fn nuovo_job_non_resta_dietro_send_pieno() {
    full_queue(Trigger::Pending);
}
#[test]
fn stop_armato_non_resta_in_send_pieno() {
    full_queue(Trigger::Stop);
}
