//! Ripristino UI reale: index fresco leased, Git/FS solo su TempDir possedute.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::GitRepository;
use std::{fs, path::Path, time::SystemTime};
fn reset_time(path: &Path, time: SystemTime) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
}
fn warm(repo: &GitRepository) {
    // Stash45 è fresco: snapshot precedente, non cache gix. Le prove con
    // cache nativa verificata sono in tests/stash_index/mod.rs (cfg(test)); preservate
    // qui le stesse assert bytes/flags/lease per l'index esterno invariato.
    repo.stash_push(Some("fixture snapshot index")).unwrap();
}
#[test]
fn lock_esterno_rifiuta_prima_di_restore_e_delete_senza_sottrarlo() {
    let dir = common::init_repo();
    fs::write(dir.path().join("a.txt"), b"locale\n").unwrap();
    fs::write(dir.path().join("nuovo.sql"), b"nuovo\n").unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    fs::write(dir.path().join(".git/index.lock"), b"altro writer\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(
        repo.discard(["a.txt", "nuovo.sql"]).is_err(),
        "index.lock deve escludere tutto il ripristino"
    );
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"locale\n");
    assert_eq!(fs::read(dir.path().join("nuovo.sql")).unwrap(), b"nuovo\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(
        fs::read(dir.path().join(".git/index.lock")).unwrap(),
        b"altro writer\n"
    );
}
#[test]
fn timestamp_uguale_non_ripristina_blob_dello_snapshot_precedente() {
    let dir = common::init_repo();
    let repo = GitRepository::discover(dir.path()).unwrap();
    warm(&repo);
    let indexpath = dir.path().join(".git/index");
    let time = fs::metadata(&indexpath).unwrap().modified().unwrap();
    fs::write(dir.path().join("a.txt"), b"staged esterno\n").unwrap();
    common::git(dir.path(), &["add", "--", "a.txt"]);
    reset_time(&indexpath, time);
    let index = fs::read(&indexpath).unwrap();
    fs::write(dir.path().join("a.txt"), b"nuovo dirty\n").unwrap();
    repo.discard(["a.txt"]).unwrap();
    assert_eq!(
        fs::read(dir.path().join("a.txt")).unwrap(),
        b"staged esterno\n",
        "non usare blob della cache al vecchio index"
    );
    assert_eq!(fs::read(indexpath).unwrap(), index);
    assert!(!dir.path().join(".git/index.lock").exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn index_corrotto_stesso_mtime_non_autorizza_restore_dopo_snapshot() {
    let dir = common::init_repo();
    let repo = GitRepository::discover(dir.path()).unwrap();
    warm(&repo);
    let path = dir.path().join(".git/index");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"corrotto\n").unwrap();
    reset_time(&path, time);
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    assert!(
        repo.discard(["a.txt"]).is_err(),
        "file index attuale corrotto non si sostituisce con cache valida"
    );
    assert_eq!(fs::read(&path).unwrap(), b"corrotto\n");
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"dirty\n");
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn attributi_fallback_provengono_dallo_stesso_index_fresco_del_blob() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), ".gitattributes", "a.txt text eol=lf\n", "attrs");
    let repo = GitRepository::discover(dir.path()).unwrap();
    warm(&repo);
    let path = dir.path().join(".git/index");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(dir.path().join(".gitattributes"), b"a.txt text eol=crlf\n").unwrap();
    common::git(dir.path(), &["add", "--", ".gitattributes"]);
    reset_time(&path, time);
    let index = fs::read(&path).unwrap();
    fs::remove_file(dir.path().join(".gitattributes")).unwrap();
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    repo.discard(["a.txt"]).unwrap();
    assert_eq!(
        fs::read(dir.path().join("a.txt")).unwrap(),
        b"uno\r\n",
        "pipeline non deve caricare una seconda versione cached dell'index"
    );
    assert_eq!(fs::read(&path).unwrap(), index);
    assert!(!dir.path().join(".gitattributes").exists());
}
#[test]
fn errore_preflight_libera_lease_e_retry_protetto_non_riscrive_index() {
    let dir = common::init_repo();
    let repo = GitRepository::discover(dir.path()).unwrap();
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    assert!(repo.discard(["a.txt", ".git/config"]).is_err());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"dirty\n");
    assert!(!dir.path().join(".git/index.lock").exists());
    repo.discard(["a.txt"]).unwrap();
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn index_assente_conserva_head_fallback_senza_ricreare_file_index() {
    let dir = common::init_repo();
    let repo = GitRepository::discover(dir.path()).unwrap();
    fs::write(dir.path().join("a.txt"), b"staged\n").unwrap();
    repo.stage(["a.txt"]).unwrap();
    warm(&repo);
    fs::remove_file(dir.path().join(".git/index")).unwrap();
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    repo.discard(["a.txt"]).unwrap();
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!dir.path().join(".git/index").exists());
    assert!(!dir.path().join(".git/index.lock").exists());
}
