//! File richiesto/tree reali e filtro native preflight, non helper ownership.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::{GitError, GitRepository};
use std::{fs, path::Path, sync::atomic::AtomicBool};
fn target(root: &Path, attrs: &[u8]) -> gix::ObjectId {
    let raw = gix::open(root).unwrap();
    let mut editor = raw.empty_tree().edit().unwrap();
    for (path, bytes) in [
        (".gitattributes", attrs),
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
    editor.write().unwrap().detach()
}
fn driver(root: &Path, script: &Path, key: &str) {
    let path = script.join("helper.py");
    fs::write(&path,"import pathlib,sys\npathlib.Path(sys.argv[1]).write_text('avviato')\nsys.stdout.buffer.write(sys.stdin.buffer.read())\n").unwrap();
    common::git(
        root,
        &[
            "config",
            key,
            &format!(
                "python3 '{}' '{}'",
                path.display(),
                script.join("started").display()
            ),
        ],
    );
    common::git(root, &["config", "filter.cleanup.required", "true"]);
}
#[test]
fn richiesto_helper_rifiutato_senza_conversione_o_pubblicazione() {
    let dir = common::init_repo();
    let script = tempfile::tempdir().unwrap();
    driver(dir.path(), script.path(), "filter.cleanup.smudge");
    let tree = target(dir.path(), b"a.txt filter=cleanup\n");
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let result = GitRepository::discover(dir.path())
        .unwrap()
        .checkout_content(tree, Path::new("a.txt"), &AtomicBool::new(false));
    assert!(
        matches!(result,Err(GitError::Worktree(ref error)) if error.contains("filtro comando selezionato"))
    );
    assert!(!script.path().join("started").exists());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn altro_file_seleziona_helper_ma_richiesto_builtin_rimane_leggibile() {
    let dir = common::init_repo();
    let script = tempfile::tempdir().unwrap();
    driver(dir.path(), script.path(), "filter.cleanup.process");
    let tree = target(dir.path(), b"a.txt text eol=crlf\nb.txt filter=cleanup\n");
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert_eq!(
        repo.checkout_content(tree, Path::new("a.txt"), &AtomicBool::new(false))
            .unwrap(),
        b"nuovo a\r\n"
    );
    assert!(!script.path().join("started").exists());
    assert!(
        repo.checkout_content(tree, Path::new("b.txt"), &AtomicBool::new(false))
            .is_err()
    );
    assert!(!script.path().join("started").exists());
}
#[test]
fn info_override_e_macro_native_sul_solo_richiesto() {
    let dir = common::init_repo();
    let script = tempfile::tempdir().unwrap();
    driver(dir.path(), script.path(), "filter.cleanup.smudge");
    let tree = target(
        dir.path(),
        b"[attr]speciale filter=cleanup\na.txt speciale\nb.txt speciale\n",
    );
    fs::write(
        dir.path().join(".git/info/attributes"),
        b"a.txt -filter text eol=crlf\n",
    )
    .unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert_eq!(
        repo.checkout_content(tree, Path::new("a.txt"), &AtomicBool::new(false))
            .unwrap(),
        b"nuovo a\r\n"
    );
    assert!(
        matches!(repo.checkout_content(tree,Path::new("b.txt"),&AtomicBool::new(false)),Err(GitError::Worktree(ref error)) if error.contains("filtro comando selezionato"))
    );
    assert!(!script.path().join("started").exists());
}
#[test]
fn driver_clean_senza_smudge_non_avviato_per_checkout() {
    let dir = common::init_repo();
    let script = tempfile::tempdir().unwrap();
    driver(dir.path(), script.path(), "filter.cleanup.clean");
    let tree = target(dir.path(), b"a.txt filter=cleanup\n");
    assert_eq!(
        GitRepository::discover(dir.path())
            .unwrap()
            .checkout_content(tree, Path::new("a.txt"), &AtomicBool::new(false))
            .unwrap(),
        b"nuovo a\n"
    );
    assert!(!script.path().join("started").exists());
}
