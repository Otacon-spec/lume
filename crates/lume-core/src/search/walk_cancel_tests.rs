//! Walk reale, predicato di generazione controllato senza clock/thread fake.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
#[test]
fn cancellazione_osservata_anche_fra_directory_prima_di_un_file() {
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("dir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("a.txt"), "ago\n").unwrap();
    let generation = AtomicU64::new(7);
    let mut checks = 0;
    let found = walk_files(root.path(), || {
        checks += 1;
        if checks == 2 {
            generation.store(8, Ordering::SeqCst);
        }
        generation.load(Ordering::SeqCst) != 7
    })
    .collect::<Vec<_>>();
    assert!(found.is_empty());
    assert_eq!(
        checks, 2,
        "la directory radice non deve essere filtrata prima di osservare nuova generazione"
    );
}
#[test]
fn candidati_aperti_e_sorgenti_pre_cancellati_sono_vuoti() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    std::fs::create_dir(&src).unwrap();
    let path = src.join("a.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let generation = AtomicU64::new(8);
    for scope in [
        SearchScope::Aperti(vec![path]),
        SearchScope::Sorgenti(root.path().to_path_buf()),
    ] {
        assert_eq!(candidati(&scope, &generation, 7).count(), 0);
    }
}
