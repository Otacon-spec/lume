#![allow(clippy::unwrap_used, clippy::expect_used)]
use lume_syntax::{LanguageId, SyntaxState};
use ropey::Rope;

#[test]
fn snapshot_matches_oracle_and_rejects_stale_or_uncovered_reads() {
    let rope = Rope::from_str("// città 漢字 😀\nfn main() { let x = 42; }\n");
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let range = 3..rope.len_bytes() - 2;
    let expected: Vec<_> = state
        .highlights(rope.slice(..), range.clone())
        .into_iter()
        .filter_map(|(r, c)| {
            let clipped = r.start.max(range.start)..r.end.min(range.end);
            (!clipped.is_empty()).then_some((clipped, c))
        })
        .collect();
    let snapshot = state
        .highlight_snapshot(rope.slice(..), 17, range.clone())
        .unwrap();
    assert_eq!(snapshot.tokens(), expected);
    assert!(snapshot.covers(LanguageId::Rust, 17, range.clone()));
    assert!(!snapshot.covers(LanguageId::Rust, 18, range.clone()));
    assert!(!snapshot.covers(LanguageId::Toml, 17, range.clone()));
    assert!(!snapshot.covers(LanguageId::Rust, 17, 0..range.end));
    assert!(!snapshot.covers(LanguageId::Rust, 17, range.end..range.start));
    let cloned = snapshot.clone();
    assert_eq!(cloned.tokens().as_ptr(), snapshot.tokens().as_ptr());
    drop(state);
    drop(rope);
    assert_eq!(snapshot.tokens(), expected);
}

#[test]
fn snapshot_validates_bounds_and_preserves_empty_window() {
    let rope = Rope::from_str("fn main() {}\n");
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    assert!(
        state
            .highlight_snapshot(rope.slice(..), 0, std::ops::Range { start: 2, end: 1 })
            .is_none()
    );
    assert!(
        state
            .highlight_snapshot(rope.slice(..), 0, 0..rope.len_bytes() + 1)
            .is_none()
    );
    let empty = state.highlight_snapshot(rope.slice(..), 0, 2..2).unwrap();
    assert!(empty.tokens().is_empty());
    assert!(empty.covers(LanguageId::Rust, 0, 2..2));
}
