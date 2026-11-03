//! Cleanup esplicito di temporanei reali: non release receipt universale.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
#[cfg(unix)]
use lume_git::GitError;
use lume_git::GitRepository;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};
fn target(root: &Path) -> gix::ObjectId {
    let raw = gix::open(root).unwrap();
    let mut editor = raw.empty_tree().edit().unwrap();
    for path in ["a.txt", "c.txt"] {
        let payload = format!("nuovo {path}\n");
        let id = raw
            .write_object(gix::objs::BlobRef {
                data: payload.as_bytes(),
            })
            .unwrap()
            .detach();
        editor
            .upsert(
                gix::bstr::BStr::new(path),
                gix::object::tree::EntryKind::Blob,
                id,
            )
            .unwrap();
    }
    editor.write().unwrap().detach()
}
fn temps(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".lume-save-")
        })
        .collect()
}
#[test]
fn abort_esplicito_raccoglie_cleanup_e_non_pubblica_o_cancella_head() {
    let dir = common::init_repo();
    let tree = target(dir.path());
    let repo = GitRepository::discover(dir.path()).unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    let prepared = repo
        .prepare_update(tree, Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert_eq!(temps(dir.path()).len(), 2);
    prepared.abort().unwrap();
    assert!(temps(dir.path()).is_empty());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!dir.path().join("c.txt").exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
    for lock in [
        ".git/index.lock",
        ".git/HEAD.lock",
        ".git/refs/heads/main.lock",
    ] {
        assert!(!dir.path().join(lock).exists());
    }
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[cfg(unix)]
#[test]
fn errore_primo_cleanup_non_abbandona_il_secondo_o_millanta_successo() {
    let dir = common::init_repo();
    let tree = target(dir.path());
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo
        .prepare_update(tree, Arc::new(AtomicBool::new(false)))
        .unwrap();
    // Primo Put del delta (a), indipendente dall'ordine di read_dir.
    let temp = temps(dir.path())
        .into_iter()
        .find(|path| fs::read(path).unwrap() == b"nuovo a.txt\n")
        .unwrap();
    fs::remove_file(&temp).unwrap();
    fs::create_dir(&temp).unwrap();
    let result = prepared.abort();
    assert!(matches!(
        result,
        Err(GitError::UpdateCleanup {
            attempted: 2,
            failed: 1
        })
    ));
    assert_eq!(temps(dir.path()), vec![temp.clone()]);
    assert!(temp.is_dir());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!dir.path().join("c.txt").exists());
    for lock in [
        ".git/index.lock",
        ".git/HEAD.lock",
        ".git/refs/heads/main.lock",
    ] {
        assert!(!dir.path().join(lock).exists());
    }
    fs::remove_dir(temp).unwrap();
}
