#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use lume_git::{ChangeKind, GitError, GitRepository};

fn configure_identity(root: &Path) {
    let mut config = OpenOptions::new()
        .append(true)
        .open(root.join(".git").join("config"))
        .unwrap();
    writeln!(
        config,
        "\n[user]\n\tname = Lume Test\n\temail = lume@example.test"
    )
    .unwrap();
}

#[test]
fn commits_staged_index_with_repository_identity_and_updates_head() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_git_commit_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    gix::init(&root).unwrap();
    configure_identity(&root);
    fs::write(root.join("file.txt"), "prima versione\n").unwrap();

    let repository = GitRepository::discover(&root).unwrap();
    repository.stage(["file.txt"]).unwrap();
    let first = repository
        .commit("primo commit\n\ncreato dal backend nativo")
        .unwrap();

    assert_eq!(first.len(), 40);
    assert!(repository.status().unwrap().is_empty());
    let log = repository.log(10).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].id, first);
    assert_eq!(log[0].subject, "primo commit");
    assert_eq!(log[0].author_name, "Lume Test");

    assert!(matches!(
        repository.commit("nessuna modifica"),
        Err(GitError::NothingStaged)
    ));
    assert!(matches!(
        repository.commit("   "),
        Err(GitError::EmptyMessage)
    ));

    fs::write(root.join("file.txt"), "seconda versione\n").unwrap();
    repository.stage(["file.txt"]).unwrap();
    let staged = repository.status().unwrap().remove(0);
    assert_eq!(staged.index, Some(ChangeKind::Modified));
    assert_eq!(staged.worktree, None);

    repository.unstage(["file.txt"]).unwrap();
    let unstaged = repository.status().unwrap().remove(0);
    assert_eq!(unstaged.index, None);
    assert_eq!(unstaged.worktree, Some(ChangeKind::Modified));

    repository.stage(["file.txt"]).unwrap();
    let second = repository.commit("secondo commit").unwrap();
    let log = repository.log(10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].id, second);
    assert_eq!(log[0].parent_ids, vec![first]);
    assert_eq!(log[1].subject, "primo commit");

    fs::remove_dir_all(root).unwrap();
}

/// Amend **sostituisce** HEAD: stesso parent, autore conservato, il vecchio
/// commit non e' piu' raggiungibile.
#[test]
fn amend_riscrive_head_conservando_parent_e_autore() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_git_amend_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    gix::init(&root).unwrap();
    configure_identity(&root);
    let repository = GitRepository::discover(&root).unwrap();

    fs::write(root.join("a.txt"), "uno\n").unwrap();
    repository.stage(["a.txt"]).unwrap();
    let first = repository.commit("primo").unwrap();
    fs::write(root.join("b.txt"), "due\n").unwrap();
    repository.stage(["b.txt"]).unwrap();
    let second = repository.commit("secondo con refuso").unwrap();

    assert_eq!(
        repository.head_message().unwrap().as_deref().map(str::trim),
        Some("secondo con refuso"),
        "il messaggio di HEAD serve a precompilare la casella quando si attiva Amend"
    );
    let author_before = repository.log(1).unwrap().remove(0).author_name;

    let amended = repository.commit_with("secondo, corretto", true).unwrap();

    assert_ne!(amended, second, "amend crea un oggetto nuovo");
    let log = repository.log(10).unwrap();
    assert_eq!(log.len(), 2, "amend sostituisce, non aggiunge");
    assert_eq!(log[0].id, amended);
    assert_eq!(log[0].subject, "secondo, corretto");
    assert_eq!(
        log[0].parent_ids,
        vec![first.clone()],
        "il parent e' quello del commit riscritto, non il commit riscritto"
    );
    assert_eq!(
        log[0].author_name, author_before,
        "riscrivere il messaggio non cambia chi ha scritto il codice"
    );
    assert_eq!(log[1].id, first);

    fs::remove_dir_all(root).unwrap();
}

/// Correggere il solo messaggio e' l'uso piu' comune di Amend: senza nulla in
/// stage un commit normale fallisce, un amend no.
#[test]
fn amend_del_solo_messaggio_non_richiede_nulla_in_stage() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_git_amend_msg_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    gix::init(&root).unwrap();
    configure_identity(&root);
    let repository = GitRepository::discover(&root).unwrap();
    fs::write(root.join("a.txt"), "uno\n").unwrap();
    repository.stage(["a.txt"]).unwrap();
    repository.commit("refsuo").unwrap();

    assert!(matches!(
        repository.commit("un altro"),
        Err(GitError::NothingStaged)
    ));
    let amended = repository.commit_with("refuso", true).unwrap();

    let log = repository.log(10).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].id, amended);
    assert_eq!(log[0].subject, "refuso");

    fs::remove_dir_all(root).unwrap();
}
