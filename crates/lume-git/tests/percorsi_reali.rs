//! Prerequisito dei piani di integrazione: nomi Git reali non lossy.
//! Solo fixture TempDir e Git CLI per costruzione/verifica; backend gix vero.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::{ChangeKind, DiffRow, DiffSign, GitRepository};
use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};
fn names() -> [PathBuf; 2] {
    [
        PathBuf::from(OsString::from_vec(b"nome-\xff.txt".to_vec())),
        PathBuf::from(OsString::from_vec(b"nome-\xfe.txt".to_vec())),
    ]
}
fn commit(dir: &Path) {
    common::git(dir, &["add", "--all"]);
    common::git(dir, &["commit", "-m", "nomi raw"]);
}
fn text_present(rows: &[DiffRow], sign: DiffSign, text: &str) -> bool {
    rows.iter()
        .any(|row| matches!(row,DiffRow::Line{sign:s,text:t,..} if *s==sign&&t==text))
}
#[test]
fn status_conserva_due_nomi_non_utf8_distinti_senza_collassarli() {
    let dir = common::init_repo();
    let paths = names();
    for path in &paths {
        std::fs::write(dir.path().join(path), b"contenuto\n").unwrap();
    }
    let repo = GitRepository::discover(dir.path()).unwrap();
    let status = repo.status().unwrap();
    assert_eq!(
        status.len(),
        2,
        "due nomi diversi non diventano una sola voce di stato"
    );
    for path in paths {
        let entry = status
            .iter()
            .find(|entry| entry.path == path)
            .expect("nome raw esatto nello stato");
        assert!(entry.is_untracked());
    }
}
#[test]
fn file_del_commit_conservano_byte_dei_nomi_anziche_testo_lossy() {
    let dir = common::init_repo();
    let paths = names();
    for path in &paths {
        std::fs::write(dir.path().join(path), b"contenuto\n").unwrap();
    }
    commit(dir.path());
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let files = GitRepository::discover(dir.path())
        .unwrap()
        .commit_files(&head)
        .unwrap();
    for path in paths {
        assert!(
            files
                .iter()
                .any(|file| file.path == path && file.kind == ChangeKind::Added),
            "elenco commit deve restituire il nome raw {path:?}"
        );
    }
    assert_eq!(files.len(), 2);
}
#[test]
fn diff_non_utf8_usa_index_e_head_corretti_invece_di_file_nuovo() {
    let dir = common::init_repo();
    let path = names().into_iter().next().unwrap();
    std::fs::write(dir.path().join(&path), b"prima\n").unwrap();
    commit(dir.path());
    std::fs::write(dir.path().join(&path), b"dopo\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let work = repo.diff_worktree_file(&path).unwrap();
    assert!(text_present(&work.rows, DiffSign::Removed, "prima"));
    assert!(text_present(&work.rows, DiffSign::Added, "dopo"));
    repo.stage([&path]).unwrap();
    let staged = repo.diff_index_file(&path).unwrap();
    assert!(
        text_present(&staged.rows, DiffSign::Removed, "prima"),
        "lookup HEAD non può trattare nome raw come file nuovo"
    );
    assert!(text_present(&staged.rows, DiffSign::Added, "dopo"));
    common::git(dir.path(), &["commit", "-m", "modifica raw"]);
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let historical = repo.diff_commit_file(&head, &path).unwrap();
    assert!(text_present(&historical.rows, DiffSign::Removed, "prima"));
    assert!(text_present(&historical.rows, DiffSign::Added, "dopo"));
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn hunk_non_utf8_diff_stage_unstage_restano_sullo_stesso_file_reale() {
    let dir = common::init_repo();
    let path = names().into_iter().next().unwrap();
    std::fs::write(dir.path().join(&path), b"prima\n").unwrap();
    commit(dir.path());
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    std::fs::write(dir.path().join(&path), b"dopo\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let diff = repo.diff_worktree_file(&path).unwrap();
    repo.stage_hunk(&path, diff.hunks.first().unwrap()).unwrap();
    assert!(repo.diff_worktree_file(&path).unwrap().hunks.is_empty());
    let staged = repo.diff_index_file(&path).unwrap();
    assert!(text_present(&staged.rows, DiffSign::Removed, "prima"));
    repo.unstage_hunk(&path, staged.hunks.first().unwrap())
        .unwrap();
    assert!(repo.diff_index_file(&path).unwrap().hunks.is_empty());
    assert_eq!(std::fs::read(dir.path().join(&path)).unwrap(), b"dopo\n");
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    let state = repo.status().unwrap();
    assert_eq!(state.len(), 1);
    assert_eq!(state.first().unwrap().path, path);
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}

#[test]
fn backslash_unix_letterale_non_seleziona_il_file_nella_directory() {
    let dir = common::init_repo();
    let literal = Path::new("dir\\file.txt");
    std::fs::create_dir(dir.path().join("dir")).unwrap();
    std::fs::write(dir.path().join("dir/file.txt"), b"altro file\n").unwrap();
    std::fs::write(dir.path().join(literal), b"prima\n").unwrap();
    commit(dir.path());
    std::fs::write(dir.path().join(literal), b"dopo\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let diff = repo.diff_worktree_file(literal).unwrap();
    assert!(text_present(&diff.rows, DiffSign::Removed, "prima"));
    assert!(text_present(&diff.rows, DiffSign::Added, "dopo"));
    assert_eq!(
        std::fs::read(dir.path().join("dir/file.txt")).unwrap(),
        b"altro file\n"
    );
}
