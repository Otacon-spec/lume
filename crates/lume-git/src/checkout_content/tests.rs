//! Tree/ODB/index/worktree reali; conversione native, non checkout su disco.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use gix::object::tree::EntryKind;
use std::fs;
fn fixture() -> (tempfile::TempDir, GitRepository) {
    let dir = tempfile::tempdir().unwrap();
    gix::init(dir.path()).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    fs::write(dir.path().join(".gitattributes"), b"*.sql text eol=lf\n").unwrap();
    fs::write(dir.path().join("utente.sql"), b"select 1;\n").unwrap();
    repo.stage([".gitattributes", "utente.sql"]).unwrap();
    (dir, repo)
}
fn tree(repo: &GitRepository, entries: &[(&[u8], EntryKind, &[u8])]) -> gix::ObjectId {
    let mut editor = repo.repository.empty_tree().edit().unwrap();
    for (path, kind, data) in entries {
        let id = repo.repository.write_blob(data).unwrap().detach();
        editor
            .upsert(gix::bstr::BStr::new(path), *kind, id)
            .unwrap();
    }
    editor.write().unwrap().detach()
}
fn read(repo: &GitRepository, id: gix::ObjectId, path: &Path) -> Result<Vec<u8>, GitError> {
    repo.checkout_content(id, path, &AtomicBool::new(false))
}
#[test]
fn nuovi_attributi_non_sostituiti_da_index_o_worktree_corrente() {
    let (dir, repo) = fixture();
    let id = tree(
        &repo,
        &[
            (b".gitattributes", EntryKind::Blob, b"*.sql text eol=crlf\n"),
            (b"q.sql", EntryKind::Blob, b"select 2;\n"),
        ],
    );
    fs::write(dir.path().join(".gitattributes"), b"*.sql -text\n").unwrap();
    let before = fs::read(dir.path().join(".git/index")).unwrap();
    let head = fs::read(dir.path().join(".git/HEAD")).unwrap();
    assert_eq!(
        read(&repo, id, Path::new("q.sql")).unwrap(),
        b"select 2;\r\n",
        "checkout deve usare attributi del tree richiesto"
    );
    assert_eq!(
        fs::read(dir.path().join(".gitattributes")).unwrap(),
        b"*.sql -text\n"
    );
    assert_eq!(
        fs::read(dir.path().join("utente.sql")).unwrap(),
        b"select 1;\n"
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), before);
    assert_eq!(fs::read(dir.path().join(".git/HEAD")).unwrap(), head);
    assert!(!dir.path().join("q.sql").exists());
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn attributi_rimossi_nel_tree_non_riappaiono_dal_worktree() {
    let (dir, repo) = fixture();
    fs::write(dir.path().join(".gitattributes"), b"*.sql text eol=crlf\n").unwrap();
    let id = tree(&repo, &[(b"q.sql", EntryKind::Blob, b"solo LF\n")]);
    assert_eq!(
        read(&repo, id, Path::new("q.sql")).unwrap(),
        b"solo LF\n",
        "assenza nel tree è assenza, non fallback worktree"
    );
}
#[test]
fn attributi_annidati_del_tree_prevalgono_e_cache_pipeline_non_attraversa_due_tree() {
    let (_dir, repo) = fixture();
    let a = tree(
        &repo,
        &[
            (b".gitattributes", EntryKind::Blob, b"*.sql text eol=crlf\n"),
            (
                b"sub/.gitattributes",
                EntryKind::Blob,
                b"*.sql text eol=lf\n",
            ),
            (b"sub/q.sql", EntryKind::Blob, b"x\n"),
            (b"q.sql", EntryKind::Blob, b"y\n"),
        ],
    );
    assert_eq!(read(&repo, a, Path::new("sub/q.sql")).unwrap(), b"x\n");
    assert_eq!(read(&repo, a, Path::new("q.sql")).unwrap(), b"y\r\n");
    let b = tree(
        &repo,
        &[
            (b".gitattributes", EntryKind::Blob, b"*.sql text eol=lf\n"),
            (b"q.sql", EntryKind::Blob, b"y\n"),
        ],
    );
    assert_eq!(read(&repo, b, Path::new("q.sql")).unwrap(), b"y\n");
    assert_eq!(read(&repo, a, Path::new("q.sql")).unwrap(), b"y\r\n");
}
#[test]
fn info_attributes_native_conservano_precedenza_su_tree() {
    let (dir, repo) = fixture();
    fs::create_dir_all(dir.path().join(".git/info")).unwrap();
    fs::write(
        dir.path().join(".git/info/attributes"),
        b"q.sql text eol=lf\n",
    )
    .unwrap();
    let id = tree(
        &repo,
        &[
            (b".gitattributes", EntryKind::Blob, b"*.sql text eol=crlf\n"),
            (b"q.sql", EntryKind::Blob, b"x\n"),
            (b"altro.sql", EntryKind::Blob, b"y\n"),
        ],
    );
    assert_eq!(read(&repo, id, Path::new("q.sql")).unwrap(), b"x\n");
    assert_eq!(read(&repo, id, Path::new("altro.sql")).unwrap(), b"y\r\n");
}
#[test]
fn tipi_non_checkout_paths_ostili_oid_mancanti_e_cancel_non_diventano_payload() {
    let (_dir, repo) = fixture();
    let id = tree(
        &repo,
        &[
            (b"link", EntryKind::Link, b"fuori"),
            (b"sub/file", EntryKind::Blob, b"x"),
            (b"run", EntryKind::BlobExecutable, b"#!/bin/sh\n"),
        ],
    );
    for name in ["link", "sub", "manca", "../fuori", ".git/config"] {
        assert!(read(&repo, id, Path::new(name)).is_err(), "{name}");
    }
    assert_eq!(read(&repo, id, Path::new("run")).unwrap(), b"#!/bin/sh\n");
    let missing = gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
    assert!(read(&repo, missing, Path::new("q.sql")).is_err());
    assert!(matches!(
        repo.checkout_content(missing, Path::new("q.sql"), &AtomicBool::new(true)),
        Err(GitError::Cancelled)
    ));
    let blob = repo.repository.write_blob(b"non tree").unwrap().detach();
    assert!(read(&repo, blob, Path::new("q.sql")).is_err());
}
#[cfg(unix)]
#[test]
fn nomi_raw_e_backslash_restano_identita_native_nella_conversione() {
    use std::os::unix::ffi::OsStringExt;
    let (_dir, repo) = fixture();
    let id = tree(
        &repo,
        &[
            (b".gitattributes", EntryKind::Blob, b"*.sql text eol=crlf\n"),
            (b"q\xff.sql", EntryKind::Blob, b"raw\n"),
            (b"a\\b.sql", EntryKind::Blob, b"literal\n"),
        ],
    );
    let raw = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"q\xff.sql".to_vec()));
    assert_eq!(read(&repo, id, &raw).unwrap(), b"raw\r\n");
    assert_eq!(
        read(&repo, id, Path::new("a\\b.sql")).unwrap(),
        b"literal\r\n"
    );
    assert!(read(&repo, id, Path::new("a/b.sql")).is_err());
}
