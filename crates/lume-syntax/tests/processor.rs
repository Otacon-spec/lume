#![allow(clippy::unwrap_used)]
use lume_syntax::{LanguageId, SyntaxProcessor, SyntaxRequest, SyntaxState};
use ropey::Rope;
use std::sync::atomic::AtomicBool;

fn request(revision: u64, source: &str) -> SyntaxRequest {
    let source = Rope::from_str(source);
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

#[test]
fn skipped_revisions_and_identity_changes_match_fresh_oracle() {
    let mut worker = SyntaxProcessor::default();
    let cancel = AtomicBool::new(false);
    for mut request in [
        request(1, "fn main() {}\n"),
        request(4, "// città 😀\nfn main() { let x = 42; }\n"),
        request(1, "fn other() {}\n"),
    ] {
        if request.revision == 1 && request.source.len_bytes() != 13 {
            request.document = 2;
        }
        let oracle = SyntaxState::new(request.language, request.source.slice(..))
            .unwrap()
            .highlight_snapshot(
                request.source.slice(..),
                request.revision,
                request.window.clone(),
            )
            .unwrap();
        let result = worker.process(request.clone(), &cancel).unwrap();
        assert_eq!(result.document, request.document);
        assert_eq!(result.epoch, request.epoch);
        assert_eq!(result.snapshot.tokens(), oracle.tokens());
        assert!(
            result
                .snapshot
                .covers(request.language, request.revision, request.window)
        );
    }
}

#[test]
fn cancelled_request_can_be_retried_and_changed_text_is_not_queried_on_old_tree() {
    let mut worker = SyntaxProcessor::default();
    let initial = request(1, "fn main() {}\n");
    assert!(
        worker
            .process(initial.clone(), &AtomicBool::new(true))
            .is_none()
    );
    assert!(worker.process(initial, &AtomicBool::new(false)).is_some());
    // Stessa identità/versione, testo diverso: deve ricostruire lo stato.
    let changed = request(1, "// changed\nfn other() {}\n");
    let result = worker
        .process(changed.clone(), &AtomicBool::new(false))
        .unwrap();
    let oracle = SyntaxState::new(LanguageId::Rust, changed.source.slice(..))
        .unwrap()
        .highlight_snapshot(changed.source.slice(..), 1, changed.window)
        .unwrap();
    assert_eq!(result.snapshot.tokens(), oracle.tokens());
}

#[test]
fn incremental_unicode_invalid_edit_and_epoch_change_use_matching_text() {
    use lume_syntax::InputEditRaw;
    let mut worker = SyntaxProcessor::default();
    let cancel = AtomicBool::new(false);
    worker
        .process(request(1, "fn main() {}\n"), &cancel)
        .unwrap();
    let added = "// città 😀\n";
    let mut next = request(2, &(added.to_owned() + "fn main() {}\n"));
    next.edit = Some((
        1,
        InputEditRaw {
            start_byte: 0,
            old_end_byte: 0,
            new_end_byte: added.len(),
            start_point: (0, 0),
            old_end_point: (0, 0),
            new_end_point: (1, 0),
        },
    ));
    let incremental = worker.process(next.clone(), &cancel).unwrap();
    let oracle = SyntaxState::new(LanguageId::Rust, next.source.slice(..))
        .unwrap()
        .highlight_snapshot(next.source.slice(..), 2, next.window.clone())
        .unwrap();
    assert_eq!(incremental.snapshot.tokens(), oracle.tokens());
    // Un edit con coordinate incongrue non deve corrompere l'albero.
    let mut changed = request(3, "// città 😀\nfn other() { let x = 42; }\n");
    changed.edit = next.edit.map(|(_, mut edit)| {
        edit.start_byte = 1;
        (2, edit)
    });
    let result = worker.process(changed.clone(), &cancel).unwrap();
    let oracle = SyntaxState::new(LanguageId::Rust, changed.source.slice(..))
        .unwrap()
        .highlight_snapshot(changed.source.slice(..), 3, changed.window.clone())
        .unwrap();
    assert_eq!(result.snapshot.tokens(), oracle.tokens());
    changed.epoch = 8;
    let result = worker.process(changed, &cancel).unwrap();
    assert_eq!(result.epoch, 8);
    assert_eq!(result.snapshot.tokens(), oracle.tokens());
}
