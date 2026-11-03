//! Boundary di raccolta usato dal percorso prepare-error, file/temp reali.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
fn prepared(path: &Path, write: lume_core::file_write::PreparedWrite) -> PreparedFile {
    PreparedFile {
        _path: path.to_path_buf(),
        _mode: gix::object::tree::EntryKind::Blob.into(),
        content: PreparedContent::Atomic(write),
    }
}
#[test]
fn cleanup_riuscito_preserva_categoria_del_primary() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("originale");
    fs::write(&destination, b"originale\n").unwrap();
    let write = lume_core::file_write::prepare_atomic(&destination, b"nuovo\n").unwrap();
    let primary = cancelled(&AtomicBool::new(true)).unwrap_err();
    assert!(matches!(
        failed_preparation(primary, vec![prepared(&destination, write)]),
        GitError::Cancelled
    ));
    assert_eq!(fs::read(destination).unwrap(), b"originale\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
#[cfg(unix)]
#[test]
fn cleanup_fallito_preserva_primary_e_raccoglie_anche_suffisso() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::write(&a, b"a\n").unwrap();
    fs::write(&b, b"b\n").unwrap();
    let first = lume_core::file_write::prepare_atomic(&a, b"primo\n").unwrap();
    let second = lume_core::file_write::prepare_atomic(&b, b"secondo\n").unwrap();
    let temp = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| fs::read(p).unwrap() == b"primo\n")
        .unwrap();
    fs::remove_file(&temp).unwrap();
    fs::create_dir(&temp).unwrap();
    let primary = cancelled(&AtomicBool::new(true)).unwrap_err();
    match failed_preparation(primary, vec![prepared(&a, first), prepared(&b, second)]) {
        GitError::UpdatePrepareCleanup {
            source,
            attempted: 2,
            failed: 1,
        } => assert!(matches!(*source, GitError::Cancelled)),
        other => panic!("esito cleanup atteso: {other:?}"),
    };
    assert!(temp.is_dir());
    assert_eq!(fs::read(a).unwrap(), b"a\n");
    assert_eq!(fs::read(b).unwrap(), b"b\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    fs::remove_dir(temp).unwrap();
}
