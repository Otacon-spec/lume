#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::fs;

use lume_git::{ChangeKind, GitRepository};

/// Repo reale: root (albero vuoto) → secondo commit con `a.txt`; branch
/// `feat` fermo sul root; remote-tracking `origin/main` sul secondo.
fn fixture() -> (std::path::PathBuf, gix::ObjectId, gix::ObjectId) {
    // La creazione esclusiva evita collisioni fra test paralleli: il solo
    // timestamp non garantisce che due fixture abbiano nomi distinti.
    let root = tempfile::Builder::new()
        .prefix("lume_git_branches_")
        .tempdir()
        .unwrap()
        .keep();
    let mut repository = gix::init(&root).unwrap();
    {
        let mut config = repository.config_snapshot_mut();
        config.set_raw_value("user.name", "Lume Test").unwrap();
        config
            .set_raw_value("user.email", "lume@example.test")
            .unwrap();
    }
    let empty_tree = repository
        .write_object(gix::objs::Tree::empty())
        .unwrap()
        .detach();
    let signature =
        gix::actor::SignatureRef::from_bytes(b"Lume Test <lume@example.test> 1700000000 +0100")
            .unwrap();
    let first = repository
        .commit_as(
            signature,
            signature,
            "HEAD",
            "radice",
            empty_tree,
            std::iter::empty::<gix::ObjectId>(),
        )
        .unwrap()
        .detach();

    let blob = repository.write_blob(b"ciao\n").unwrap().detach();
    let tree_with_file = repository
        .write_object(gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "a.txt".into(),
                oid: blob,
            }],
        })
        .unwrap()
        .detach();
    let second = repository
        .commit_as(
            signature,
            signature,
            "HEAD",
            "aggiunge a.txt",
            tree_with_file,
            [first],
        )
        .unwrap()
        .detach();

    repository
        .reference(
            "refs/heads/feat",
            first,
            gix::refs::transaction::PreviousValue::Any,
            "test",
        )
        .unwrap();
    repository
        .reference(
            "refs/remotes/origin/main",
            second,
            gix::refs::transaction::PreviousValue::Any,
            "test",
        )
        .unwrap();
    drop(repository);
    (root, first, second)
}

#[test]
fn elenca_branch_locali_e_remoti_con_le_punte() {
    let (root, first, second) = fixture();
    let repository = GitRepository::discover(&root).unwrap();
    let branches = repository.branches().unwrap();

    let head = branches.head.clone().expect("HEAD su un branch");
    let current = branches
        .local
        .iter()
        .find(|b| b.name == head)
        .expect("branch corrente tra i locali");
    assert_eq!(current.tip, second.to_string());
    let feat = branches.local.iter().find(|b| b.name == "feat").unwrap();
    assert_eq!(feat.tip, first.to_string());
    assert_eq!(branches.remote.len(), 1);
    assert_eq!(branches.remote[0].remote, "origin");
    assert_eq!(branches.remote[0].name, "main");
    assert_eq!(branches.remote[0].tip, second.to_string());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn log_da_riferimento_e_file_modificati_per_commit() {
    let (root, first, second) = fixture();
    let repository = GitRepository::discover(&root).unwrap();

    let from_feat = repository.log_from("feat", 10).unwrap();
    assert_eq!(from_feat.len(), 1);
    assert_eq!(from_feat[0].id, first.to_string());
    assert_eq!(from_feat[0].author_offset, 3600, "offset +0100 dell'autore");

    let from_remote = repository.log_from("origin/main", 10).unwrap();
    assert_eq!(from_remote.len(), 2);
    assert_eq!(
        repository.log_from("HEAD", 1).unwrap()[0].id,
        second.to_string()
    );
    assert!(repository.log_from("non-esiste", 10).is_err());

    let files = repository.commit_files(&second.to_string()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::PathBuf::from("a.txt"));
    assert_eq!(files[0].kind, ChangeKind::Added);
    assert!(
        repository
            .commit_files(&first.to_string())
            .unwrap()
            .is_empty()
    );

    fs::remove_dir_all(root).unwrap();
}
