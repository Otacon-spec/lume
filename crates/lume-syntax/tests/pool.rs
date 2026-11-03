#![allow(clippy::unwrap_used)]
use lume_syntax::{LanguageId, SyntaxLane, SyntaxPool, SyntaxRequest};
use ropey::Rope;
use std::{
    sync::{Arc, mpsc},
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
struct Signal(mpsc::Sender<()>);
impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(());
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let _ = self.0.send(());
    }
}
fn receive(receiver: &lume_syntax::SyntaxReceiver) -> lume_syntax::SyntaxCompletion {
    let (tx, rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(Signal(tx)));
    let mut cx = Context::from_waker(&waker);
    loop {
        match receiver.poll_result(&mut cx) {
            Poll::Ready(Some(completion)) => return completion,
            Poll::Ready(None) => panic!("premature close"),
            Poll::Pending => rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        }
    }
}

#[test]
fn two_groups_have_distinct_receivers_and_owned_collectors() {
    let mut pool = SyntaxPool::default();
    let first = pool.receiver(SyntaxLane::Primary);
    let second = pool.receiver(SyntaxLane::Secondary);
    assert_ne!(first, second);
    assert_eq!(first, pool.receiver(SyntaxLane::Primary));
    assert_eq!(pool.started_threads(), 0);
    for (lane, document) in [(SyntaxLane::Primary, 1), (SyntaxLane::Secondary, 2)] {
        let source = Rope::from_str("fn main() {}\n");
        pool.submit(
            lane,
            SyntaxRequest {
                document,
                epoch: 7,
                revision: 1,
                language: LanguageId::Rust,
                window: 0..source.len_bytes(),
                source,
                edit: None,
            },
        )
        .unwrap();
    }
    assert_eq!(pool.started_threads(), 2);
    // Le code di gruppi diversi non condividono una generazione cancellabile.
    let primary = receive(&first);
    let secondary = receive(&second);
    assert_eq!(primary.document, 1);
    assert_eq!(secondary.document, 2);
    assert!(primary.result.is_some());
    assert!(secondary.result.is_some());
    let [first, second] = pool.shutdown();
    first.join().unwrap();
    second.join().unwrap();
}

#[test]
fn root_revoke_supersedes_large_requests_in_both_fixed_lanes() {
    let mut pool = SyntaxPool::default();
    let first = pool.receiver(SyntaxLane::Primary);
    let second = pool.receiver(SyntaxLane::Secondary);
    let large = Rope::from_str(&"fn old_root() {}\n".repeat(100_000));
    for lane in [SyntaxLane::Primary, SyntaxLane::Secondary] {
        pool.submit(
            lane,
            SyntaxRequest {
                document: 1,
                epoch: 7,
                revision: 1,
                language: LanguageId::Rust,
                window: 0..large.len_bytes(),
                source: large.clone(),
                edit: None,
            },
        )
        .unwrap();
    }
    for lane in [SyntaxLane::Primary, SyntaxLane::Secondary] {
        pool.revoke(lane).unwrap();
        let source = Rope::from_str("// nuova radice città 😀\nfn new_root() {}\n");
        pool.submit(
            lane,
            SyntaxRequest {
                document: 1,
                epoch: 8,
                revision: 1,
                language: LanguageId::Rust,
                window: 0..source.len_bytes(),
                source,
                edit: None,
            },
        )
        .unwrap();
    }
    assert_eq!(pool.started_threads(), 2);
    assert_eq!(first, pool.receiver(SyntaxLane::Primary));
    assert_eq!(second, pool.receiver(SyntaxLane::Secondary));
    for receiver in [&first, &second] {
        let completion = receive(receiver);
        assert_eq!(completion.epoch, 8);
        let result = completion.result.unwrap();
        assert_eq!(result.epoch, 8);
        assert!(!result.snapshot.tokens().is_empty());
    }
    for collector in pool.shutdown() {
        collector.join().unwrap();
    }
}
