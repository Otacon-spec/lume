//! Payload staged reali contro nomi ITA e index malformato, solo TempDir.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::GitRepository;
use std::fs;
#[test]
fn intent_to_add_non_e_payload_vuoto_da_ripristinare() {
    let dir = common::init_repo();
    fs::write(dir.path().join("nuovo.sql"), b"lavoro locale\n").unwrap();
    common::git(dir.path(), &["add", "-N", "--", "nuovo.sql"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(
        repo.discard(["nuovo.sql"]).is_err(),
        "il nome intent-to-add non autorizza un empty blob restore"
    );
    assert_eq!(
        fs::read(dir.path().join("nuovo.sql")).unwrap(),
        b"lavoro locale\n"
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn intent_to_add_suffisso_blocca_prima_del_primo_restore() {
    let dir = common::init_repo();
    fs::write(dir.path().join("a.txt"), b"dirty a\n").unwrap();
    fs::write(dir.path().join("nuovo.sql"), b"locale\n").unwrap();
    common::git(dir.path(), &["add", "-N", "--", "nuovo.sql"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(repo.discard(["a.txt", "nuovo.sql"]).is_err());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"dirty a\n");
    assert_eq!(fs::read(dir.path().join("nuovo.sql")).unwrap(), b"locale\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn entry_file_che_punta_tree_non_pubblica_serializzazione_tree() {
    let dir = common::init_repo();
    let tree = common::git(dir.path(), &["rev-parse", "HEAD^{tree}"]);
    common::git(
        dir.path(),
        &["update-index", "--cacheinfo", "100644", &tree, "a.txt"],
    );
    fs::write(dir.path().join("a.txt"), b"dirty da conservare\n").unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(
        repo.discard(["a.txt"]).is_err(),
        "mode FILE non basta a provare che ODB object e un blob"
    );
    assert_eq!(
        fs::read(dir.path().join("a.txt")).unwrap(),
        b"dirty da conservare\n"
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn intent_to_add_fuori_selezione_non_blocca_ripristino_ordinario() {
    let dir = common::init_repo();
    fs::write(dir.path().join("nuovo.sql"), b"locale\n").unwrap();
    common::git(dir.path(), &["add", "-N", "--", "nuovo.sql"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    GitRepository::discover(dir.path())
        .unwrap()
        .discard(["a.txt"])
        .unwrap();
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(fs::read(dir.path().join("nuovo.sql")).unwrap(), b"locale\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
}
#[test]
fn non_blob_suffisso_dichiara_restore_gia_concluso_senza_rollback() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), "b.txt", "base b\n", "b");
    let tree = common::git(dir.path(), &["rev-parse", "HEAD^{tree}"]);
    common::git(
        dir.path(),
        &["update-index", "--cacheinfo", "100644", &tree, "b.txt"],
    );
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    fs::write(dir.path().join("a.txt"), b"dirty a\n").unwrap();
    fs::write(dir.path().join("b.txt"), b"dirty b\n").unwrap();
    let error = GitRepository::discover(dir.path())
        .unwrap()
        .discard(["a.txt", "b.txt"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("dopo 1 percorsi conclusi"), "{error}");
    assert!(error.contains("nessun rollback"), "{error}");
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"dirty b\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn blob_vuoto_realmente_staged_e_blob_binario_restano_supportati() {
    let dir = common::init_repo();
    fs::write(dir.path().join("vuoto"), b"").unwrap();
    fs::write(dir.path().join("binario"), b"\xff\x00\x01\n").unwrap();
    common::git(dir.path(), &["add", "--", "vuoto", "binario"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    fs::write(dir.path().join("vuoto"), b"dirty\n").unwrap();
    fs::write(dir.path().join("binario"), b"altro\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    repo.discard(["vuoto", "binario"]).unwrap();
    assert_eq!(fs::read(dir.path().join("vuoto")).unwrap(), b"");
    assert_eq!(
        fs::read(dir.path().join("binario")).unwrap(),
        b"\xff\x00\x01\n"
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert!(!dir.path().join(".git/index.lock").exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
