//! Cancellazione prima del walk, filesystem/engine/canali veri.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use lume_core::search::{SearchOptions, SearchScope, spawn_search};
use std::sync::{Arc, atomic::AtomicU64};
#[test]
fn ricerca_cancellata_su_albero_vuoto_non_diventa_done() {
    let root = tempfile::tempdir().unwrap();
    let updates = spawn_search(
        SearchScope::Albero(root.path().to_path_buf()),
        SearchOptions::literal("ago"),
        Arc::new(AtomicU64::new(2)),
        1,
    )
    .into_iter()
    .collect::<Vec<_>>();
    assert!(
        updates.is_empty(),
        "una ricerca cancellata non deve diventare Done: {updates:?}"
    );
}
#[test]
fn ricerca_cancellata_senza_candidati_sorgenti_non_diventa_done() {
    let root = tempfile::tempdir().unwrap();
    for n in 0..20 {
        let sub = root.path().join(format!("dir-{n}"));
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("a.txt"), "ago\n").unwrap();
    }
    let updates = spawn_search(
        SearchScope::Sorgenti(root.path().to_path_buf()),
        SearchOptions::literal("ago"),
        Arc::new(AtomicU64::new(2)),
        1,
    )
    .into_iter()
    .collect::<Vec<_>>();
    assert!(
        updates.is_empty(),
        "nessun candidato non significa completamento di query cancellata: {updates:?}"
    );
}
