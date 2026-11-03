#![allow(clippy::unwrap_used)]
use lume_syntax::{InputEditRaw, LanguageId, SyntaxState};
use ropey::Rope;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn cancelled_edit_preserves_previous_tree_and_retry_matches_fresh_parse() {
    let mut rope = Rope::from_str("fn main() {}\n");
    let mut state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let old_tree = state.tree().root_node().to_sexp();
    let old_bytes = state.tree().root_node().end_byte();
    rope.insert(0, "// città 😀\n");
    let added = "// città 😀\n".len();
    let edit = InputEditRaw {
        start_byte: 0,
        old_end_byte: 0,
        new_end_byte: added,
        start_point: (0, 0),
        old_end_point: (0, 0),
        new_end_point: (1, 0),
    };
    let cancel = AtomicBool::new(true);
    assert!(!state.apply_edit_cancellable(&edit, rope.slice(..), &cancel));
    assert_eq!(state.tree().root_node().end_byte(), old_bytes);
    assert_eq!(state.tree().root_node().to_sexp(), old_tree);
    cancel.store(false, Ordering::Release);
    assert!(state.apply_edit_cancellable(&edit, rope.slice(..), &cancel));
    let fresh = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    assert_eq!(
        state.tree().root_node().to_sexp(),
        fresh.tree().root_node().to_sexp()
    );
    assert_eq!(
        state.highlights(rope.slice(..), 0..rope.len_bytes()),
        fresh.highlights(rope.slice(..), 0..rope.len_bytes())
    );
}

#[test]
fn cancelled_query_never_returns_partial_tokens_and_retry_matches_oracle() {
    let rope = Rope::from_str("// città 😀\nfn main() { let x = 42; }\n");
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let cancel = AtomicBool::new(true);
    assert!(
        state
            .highlights_cancellable(rope.slice(..), 0..rope.len_bytes(), &cancel)
            .is_none()
    );
    cancel.store(false, Ordering::Release);
    assert_eq!(
        state
            .highlights_cancellable(rope.slice(..), 0..rope.len_bytes(), &cancel)
            .unwrap(),
        state.highlights(rope.slice(..), 0..rope.len_bytes())
    );
}

#[test]
fn cancelled_snapshot_is_absent_even_for_empty_window() {
    let rope = Rope::from_str("fn main() {}\n");
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let cancel = AtomicBool::new(true);
    assert!(
        state
            .highlight_snapshot_cancellable(rope.slice(..), 3, 0..0, &cancel)
            .is_none()
    );
    assert!(
        state
            .highlight_snapshot_cancellable(rope.slice(..), 3, 0..rope.len_bytes(), &cancel)
            .is_none()
    );
    cancel.store(false, Ordering::Release);
    let actual = state
        .highlight_snapshot_cancellable(rope.slice(..), 3, 0..rope.len_bytes(), &cancel)
        .unwrap();
    assert_eq!(
        actual.tokens(),
        state
            .highlight_snapshot(rope.slice(..), 3, 0..rope.len_bytes())
            .unwrap()
            .tokens()
    );
}

#[test]
fn cancelled_initial_parse_has_no_state_and_retry_matches_oracle() {
    let rope = Rope::from_str("// città 😀\nfn main() {}\n");
    let cancel = AtomicBool::new(true);
    assert!(SyntaxState::new_cancellable(LanguageId::Rust, rope.slice(..), &cancel).is_none());
    cancel.store(false, Ordering::Release);
    let state = SyntaxState::new_cancellable(LanguageId::Rust, rope.slice(..), &cancel).unwrap();
    let oracle = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    assert_eq!(
        state.tree().root_node().to_sexp(),
        oracle.tree().root_node().to_sexp()
    );
    assert_eq!(
        state.highlights(rope.slice(..), 0..rope.len_bytes()),
        oracle.highlights(rope.slice(..), 0..rope.len_bytes())
    );
}
