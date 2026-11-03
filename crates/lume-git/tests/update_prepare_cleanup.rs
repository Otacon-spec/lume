//! Fence Task5 prima di helper non posseduti: attrib/config nativi, no mock.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::{GitError, GitRepository};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};
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
fn target(root: &Path, selected: bool) -> gix::ObjectId {
    let raw = gix::open(root).unwrap();
    let mut editor = raw.empty_tree().edit().unwrap();
    let mut entries = vec![
        ("a.txt", b"nuovo a\n".as_slice()),
        ("b.txt", b"nuovo b\n".as_slice()),
    ];
    if selected {
        entries.push((".gitattributes", b"b.txt filter=cleanup\n"));
    }
    for (path, bytes) in entries {
        let id = raw
            .write_object(gix::objs::BlobRef { data: bytes })
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
fn helper(root: &Path, dir: &Path, key: &str) {
    let path = dir.join("helper.py");
    fs::write(&path,"import pathlib,sys\npathlib.Path(sys.argv[1]).write_text('avviato')\nsys.stdout.buffer.write(sys.stdin.buffer.read())\n").unwrap();
    common::git(
        root,
        &[
            "config",
            key,
            &format!(
                "python3 '{}' '{}'",
                path.display(),
                dir.join("started").display()
            ),
        ],
    );
    common::git(root, &["config", "filter.cleanup.required", "true"]);
}
fn refusal(result: Result<usize, GitError>) {
    match result {
        Err(GitError::Worktree(message)) => assert!(message.contains("filtro comando selezionato")),
        other => panic!("rifiuto preventivo atteso: {other:?}"),
    }
}
#[test]
fn helper_checkout_selezionato_rifiutato_prima_di_tutti_i_temp() {
    let dir = common::init_repo();
    let script = tempfile::tempdir().unwrap();
    helper(dir.path(), script.path(), "filter.cleanup.smudge");
    let tree = target(dir.path(), true);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    refusal(
        GitRepository::discover(dir.path())
            .unwrap()
            .prepare_update(tree, Arc::new(AtomicBool::new(false)))
            .map(|plan| plan.prepared_files()),
    );
    assert!(!script.path().join("started").exists());
    assert!(temps(dir.path()).is_empty());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!dir.path().join("b.txt").exists());
    assert!(!dir.path().join(".gitattributes").exists());
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
#[test]
fn helper_configurato_ma_non_selezionato_non_blocca_prepare_reale() {
    let dir = common::init_repo();
    let script = tempfile::tempdir().unwrap();
    helper(dir.path(), script.path(), "filter.cleanup.smudge");
    let tree = target(dir.path(), false);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo
        .prepare_update(tree, Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert_eq!(prepared.prepared_files(), 2);
    assert_eq!(temps(dir.path()).len(), 2);
    prepared.abort().unwrap();
    assert!(temps(dir.path()).is_empty());
    assert!(!script.path().join("started").exists());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
}
#[test]
fn helper_clean_vecchio_rifiutato_prima_di_status_dirty() {
    let dir = common::init_repo();
    common::commit_file(
        dir.path(),
        ".gitattributes",
        "a.txt filter=cleanup\n",
        "attributi vecchi",
    );
    let script = tempfile::tempdir().unwrap();
    helper(dir.path(), script.path(), "filter.cleanup.clean");
    let tree = target(dir.path(), false);
    refusal(
        GitRepository::discover(dir.path())
            .unwrap()
            .prepare_update(tree, Arc::new(AtomicBool::new(false)))
            .map(|plan| plan.prepared_files()),
    );
    assert!(!script.path().join("started").exists());
    assert!(temps(dir.path()).is_empty());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(
        fs::read(dir.path().join(".gitattributes")).unwrap(),
        b"a.txt filter=cleanup\n"
    );
}
#[test]
fn macro_nativa_e_driver_process_selezionato_rifiutati_senza_handshake() {
    let dir = common::init_repo();
    let script = tempfile::tempdir().unwrap();
    helper(dir.path(), script.path(), "filter.cleanup.process");
    let raw = gix::open(dir.path()).unwrap();
    let mut editor = raw.empty_tree().edit().unwrap();
    for (path, bytes) in [
        (
            ".gitattributes",
            b"[attr]personalizzato filter=cleanup\nb.txt personalizzato\n".as_slice(),
        ),
        ("a.txt", b"nuovo a\n".as_slice()),
        ("b.txt", b"nuovo b\n".as_slice()),
    ] {
        let id = raw
            .write_object(gix::objs::BlobRef { data: bytes })
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
    let tree = editor.write().unwrap().detach();
    refusal(
        GitRepository::discover(dir.path())
            .unwrap()
            .prepare_update(tree, Arc::new(AtomicBool::new(false)))
            .map(|plan| plan.prepared_files()),
    );
    assert!(!script.path().join("started").exists());
    assert!(temps(dir.path()).is_empty());
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn info_override_nativo_toglie_filter_e_consente_builtin() {
    let dir = common::init_repo();
    let script = tempfile::tempdir().unwrap();
    helper(dir.path(), script.path(), "filter.cleanup.smudge");
    fs::write(
        dir.path().join(".git/info/attributes"),
        b"b.txt -filter text eol=crlf\n",
    )
    .unwrap();
    let tree = target(dir.path(), true);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo
        .prepare_update(tree, Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert!(
        temps(dir.path())
            .iter()
            .any(|p| fs::read(p).unwrap() == b"nuovo b\r\n")
    );
    prepared.abort().unwrap();
    assert!(!script.path().join("started").exists());
    assert!(temps(dir.path()).is_empty());
}
