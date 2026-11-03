#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Regresso del ripristino reale, soltanto TempDir possedute.
mod common;
use lume_git::GitRepository;
use std::fs;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
#[test]
fn ripristino_non_scrive_attraverso_link_leaf_o_parent() {
    use std::os::unix::fs::symlink;
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    for parent in [false, true] {
        let root = common::init_repo();
        let outside = tempfile::tempdir().unwrap();
        let file = if parent {
            common::commit_file(root.path(), "dir/file.txt", "base\n", "dir");
            fs::remove_dir_all(root.path().join("dir")).unwrap();
            symlink(outside.path(), root.path().join("dir")).unwrap();
            "dir/file.txt"
        } else {
            fs::remove_file(root.path().join("a.txt")).unwrap();
            symlink(outside.path().join("file.txt"), root.path().join("a.txt")).unwrap();
            "a.txt"
        };
        fs::write(outside.path().join("file.txt"), "esterno\n").unwrap();
        let repo = GitRepository::discover(root.path()).unwrap();
        let result = repo.discard([file]);
        assert!(
            result.is_err(),
            "link da rifiutare, non seguire: {result:?}"
        );
        assert_eq!(
            fs::read(outside.path().join("file.txt")).unwrap(),
            b"esterno\n"
        );
    }
}
#[test]
fn ripristino_stacca_hardlink_senza_modificare_altro_nome() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let root = common::init_repo();
    let outside = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "modificato\n").unwrap();
    fs::hard_link(root.path().join("a.txt"), outside.path().join("altro.txt")).unwrap();
    GitRepository::discover(root.path())
        .unwrap()
        .discard(["a.txt"])
        .unwrap();
    assert_eq!(fs::read(root.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(
        fs::read(outside.path().join("altro.txt")).unwrap(),
        b"modificato\n"
    );
}
#[test]
fn readonly_suffisso_preflight_non_pubblica_il_primo_file() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let root = common::init_repo();
    common::commit_file(root.path(), "b.txt", "due\n", "b");
    fs::write(root.path().join("a.txt"), "locale a\n").unwrap();
    fs::write(root.path().join("b.txt"), "locale b\n").unwrap();
    let path = root.path().join("b.txt");
    let original = fs::metadata(&path).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&path, readonly).unwrap();
    let result = GitRepository::discover(root.path())
        .unwrap()
        .discard(["a.txt", "b.txt"]);
    fs::set_permissions(&path, original).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(root.path().join("a.txt")).unwrap(), b"locale a\n");
    assert_eq!(fs::read(path).unwrap(), b"locale b\n");
}
#[test]
fn metadata_git_e_absolute_con_parentdir_non_si_toccano() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let root = common::init_repo();
    let repo = GitRepository::discover(root.path()).unwrap();
    let config = root.path().join(".git/config");
    let bytes = fs::read(&config).unwrap();
    assert!(repo.discard([&config]).is_err());
    assert_eq!(fs::read(config).unwrap(), bytes);
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("privato.txt");
    fs::write(&file, "fuori\n").unwrap();
    let attack = root
        .path()
        .join("..")
        .join(outside.path().file_name().unwrap())
        .join("privato.txt");
    assert!(repo.discard([attack]).is_err());
    assert_eq!(fs::read(file).unwrap(), b"fuori\n");
}
#[cfg(unix)]
#[test]
fn nomi_non_utf8_e_backslash_non_diventano_altri_file() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    for name in [
        OsString::from_vec(b"nome-\xff.txt".to_vec()),
        OsString::from("nome\\file.txt"),
    ] {
        let root = common::init_repo();
        let path = PathBuf::from(name);
        fs::write(root.path().join(&path), "originale\n").unwrap();
        let repo = GitRepository::discover(root.path()).unwrap();
        repo.stage([&path]).unwrap();
        repo.commit("nomi").unwrap();
        fs::write(root.path().join(&path), "modificato\n").unwrap();
        repo.discard([&path]).unwrap();
        assert_eq!(fs::read(root.path().join(&path)).unwrap(), b"originale\n");
    }
}
#[cfg(unix)]
#[test]
fn ripristino_recupera_executable_e_conserva_permessi_non_exec() {
    use std::os::unix::fs::PermissionsExt;
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let root = common::init_repo();
    common::git(root.path(), &["update-index", "--chmod=+x", "a.txt"]);
    common::git(root.path(), &["commit", "-m", "eseguibile"]);
    let path = root.path().join("a.txt");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    fs::write(&path, "locale\n").unwrap();
    GitRepository::discover(root.path())
        .unwrap()
        .discard(["a.txt"])
        .unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_ne!(mode & 0o100, 0);
    assert_eq!(mode & 0o666, 0o640);
    assert_eq!(common::git(root.path(), &["status", "--porcelain"]), "");
    fs::remove_file(&path).unwrap();
    GitRepository::discover(root.path())
        .unwrap()
        .discard(["a.txt"])
        .unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(common::git(root.path(), &["status", "--porcelain"]), "");
}

#[test]
fn conflitto_non_diventa_blob_ours_o_file_non_versionato_da_eliminare() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    for deleted_ours in [false, true] {
        let root = common::init_repo();
        common::git(root.path(), &["checkout", "-b", "altra"]);
        common::commit_file(root.path(), "a.txt", "loro\n", "loro");
        common::git(root.path(), &["checkout", "main"]);
        if deleted_ours {
            common::git(root.path(), &["rm", "a.txt"]);
            common::git(root.path(), &["commit", "-m", "cancello"]);
        } else {
            common::commit_file(root.path(), "a.txt", "nostro\n", "nostro");
        }
        let result = common::cli()
            .run(
                root.path(),
                &["merge", "--no-edit", "altra"],
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
        assert!(!result.success());
        fs::write(root.path().join("solo.txt"), "non eliminare\n").unwrap();
        let before = fs::read(root.path().join("a.txt")).unwrap();
        let index = fs::read(root.path().join(".git/index")).unwrap();
        let result = GitRepository::discover(root.path())
            .unwrap()
            .discard(["solo.txt", "a.txt"]);
        assert!(result.is_err(), "conflitto ambiguo: {result:?}");
        assert_eq!(
            fs::read(root.path().join("solo.txt")).unwrap(),
            b"non eliminare\n"
        );
        assert_eq!(fs::read(root.path().join("a.txt")).unwrap(), before);
        assert_eq!(fs::read(root.path().join(".git/index")).unwrap(), index);
    }
}

#[test]
fn errore_reale_del_secondo_blob_dichiara_il_primo_concluso_senza_rollback() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let root = common::init_repo();
    common::commit_file(root.path(), "b.txt", "due\n", "b");
    common::git(
        root.path(),
        &[
            "update-index",
            "--cacheinfo",
            "100644,4242424242424242424242424242424242424242,b.txt",
        ],
    );
    fs::write(root.path().join("a.txt"), "locale a\n").unwrap();
    fs::write(root.path().join("b.txt"), "locale b\n").unwrap();
    let error = GitRepository::discover(root.path())
        .unwrap()
        .discard(["a.txt", "b.txt"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("dopo 1 percorsi conclusi"), "{error}");
    assert!(error.contains("nessun rollback"));
    assert_eq!(fs::read(root.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(fs::read(root.path().join("b.txt")).unwrap(), b"locale b\n");
    assert!(!fs::read_dir(root.path()).unwrap().any(|item| {
        item.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".lume-save-")
    }));
}

#[cfg(unix)]
#[test]
fn symlink_tracciato_e_rifiutato_come_tipo_non_supportato_non_come_blob() {
    use std::os::unix::fs::symlink;
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let root = common::init_repo();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("dest.txt");
    fs::write(&file, "esterno\n").unwrap();
    symlink(&file, root.path().join("link")).unwrap();
    common::git(root.path(), &["add", "link"]);
    common::git(root.path(), &["commit", "-m", "link"]);
    let error = GitRepository::discover(root.path())
        .unwrap()
        .discard(["link"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("nell'index"), "{error}");
    assert!(
        fs::symlink_metadata(root.path().join("link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(file).unwrap(), b"esterno\n");
}
