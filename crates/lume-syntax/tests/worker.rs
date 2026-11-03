#![allow(clippy::unwrap_used)]
use lume_syntax::{LanguageId, SyntaxRequest, SyntaxWorker};
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

#[test]
fn lazy_worker_delivers_tokens_and_explicit_shutdown_joins() {
    let mut worker = SyntaxWorker::default();
    assert!(!worker.is_started());
    let source = Rope::from_str("// città 😀\nfn main() {}\n");
    let ticket = worker
        .submit(SyntaxRequest {
            document: 1,
            epoch: 7,
            revision: 1,
            language: LanguageId::Rust,
            window: 0..source.len_bytes(),
            source,
            edit: None,
        })
        .unwrap();
    assert!(worker.is_started());
    let (tx, rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(Signal(tx)));
    let mut cx = Context::from_waker(&waker);
    let result = loop {
        match worker.poll_result(&mut cx) {
            Poll::Ready(Some(result)) => break result,
            Poll::Ready(None) => panic!("unexpected shutdown"),
            Poll::Pending => rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        }
    };
    assert_eq!(result.ticket, ticket);
    assert_eq!(result.document, 1);
    assert_eq!(result.epoch, 7);
    assert!(!result.result.unwrap().snapshot.tokens().is_empty());
    worker.shutdown().join().unwrap();
}

#[test]
fn shutdown_of_cold_worker_never_starts_thread() {
    let worker = SyntaxWorker::default();
    assert!(!worker.is_started());
    worker.shutdown().join().unwrap();
}

#[test]
fn receiver_does_not_keep_worker_alive_after_owner_drop() {
    let worker = SyntaxWorker::default();
    let receiver = worker.receiver();
    let (tx, rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(Signal(tx)));
    let mut cx = Context::from_waker(&waker);
    assert!(receiver.poll_result(&mut cx).is_pending());
    drop(worker);
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(receiver.poll_result(&mut cx), Poll::Ready(None)));
}

#[test]
fn revoking_cold_worker_does_not_start_it() {
    let mut worker = SyntaxWorker::default();
    worker.revoke().unwrap();
    assert!(!worker.is_started());
    worker.shutdown().join().unwrap();
}

#[test]
fn latest_revision_after_burst_matches_fresh_token_oracle() {
    let mut worker = SyntaxWorker::default();
    for revision in 1..40 {
        let source = Rope::from_str(&format!("// città 😀 {revision}\nfn pending() {{}}\n"));
        worker
            .submit(SyntaxRequest {
                document: 3,
                epoch: 9,
                revision,
                language: LanguageId::Rust,
                window: 0..source.len_bytes(),
                source,
                edit: None,
            })
            .unwrap();
    }
    let source = Rope::from_str("// città 漢字 😀\nfn final_revision() { let x = 42; }\n");
    let expected = lume_syntax::SyntaxState::new(LanguageId::Rust, source.slice(..))
        .unwrap()
        .highlight_snapshot(source.slice(..), 40, 0..source.len_bytes())
        .unwrap();
    let ticket = worker
        .submit(SyntaxRequest {
            document: 3,
            epoch: 9,
            revision: 40,
            language: LanguageId::Rust,
            window: 0..source.len_bytes(),
            source,
            edit: None,
        })
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(Signal(tx)));
    let mut cx = Context::from_waker(&waker);
    let completion = loop {
        match worker.poll_result(&mut cx) {
            Poll::Ready(Some(completion)) => break completion,
            Poll::Ready(None) => panic!("unexpected close"),
            Poll::Pending => rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        }
    };
    assert_eq!(completion.ticket, ticket);
    assert_eq!(completion.revision, 40);
    let result = completion.result.unwrap();
    assert_eq!(result.document, 3);
    assert_eq!(result.epoch, 9);
    assert_eq!(result.snapshot.tokens(), expected.tokens());
    worker.shutdown().join().unwrap();
}

#[test]
fn revocation_context_is_stable_on_submit_and_receiver_identity_is_unchanged() {
    let mut worker = SyntaxWorker::default();
    let receiver = worker.receiver();
    assert_eq!(receiver.revocation(), 0);
    for revision in 1..=2 {
        let source = Rope::from_str("fn main() {}\n");
        worker
            .submit(SyntaxRequest {
                document: 1,
                epoch: 7,
                revision,
                language: LanguageId::Rust,
                window: 0..source.len_bytes(),
                source,
                edit: None,
            })
            .unwrap();
        assert_eq!(receiver.revocation(), 0);
    }
    worker.revoke().unwrap();
    let context = receiver.revocation();
    assert_ne!(context, 0);
    assert_eq!(receiver, worker.receiver());
    let source = Rope::from_str("fn final_revision() {}\n");
    worker
        .submit(SyntaxRequest {
            document: 1,
            epoch: 7,
            revision: 3,
            language: LanguageId::Rust,
            window: 0..source.len_bytes(),
            source,
            edit: None,
        })
        .unwrap();
    assert_eq!(receiver.revocation(), context);
    assert_eq!(receiver, worker.receiver());
    worker.shutdown().join().unwrap();
}
