#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use lume_git::{ChangeKind, GitError, GitRepository};

#[test]
fn discovers_repository_and_reports_untracked_file_without_git_cli() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_git_status_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    gix::init(&root).unwrap();
    fs::write(root.join("nuovo.txt"), "contenuto\n").unwrap();

    let repository = GitRepository::discover(root.join("subdir").join("missing"))
        .or_else(|_| GitRepository::discover(&root))
        .unwrap();
    let statuses = repository.status().unwrap();

    let file = statuses
        .iter()
        .find(|status| status.path == std::path::Path::new("nuovo.txt"))
        .expect("nuovo.txt presente nello status");
    assert_eq!(file.index, None);
    assert_eq!(file.worktree, Some(ChangeKind::Added));
    assert_eq!(repository.work_dir(), root.as_path());

    repository.stage(["nuovo.txt"]).unwrap();
    let statuses = repository.status().unwrap();
    let file = statuses
        .iter()
        .find(|status| status.path == std::path::Path::new("nuovo.txt"))
        .expect("nuovo.txt presente nello status dopo lo staging");
    assert_eq!(file.index, Some(ChangeKind::Added));
    assert_eq!(file.worktree, None);
    assert!(root.join(".git").join("index").is_file());

    repository.unstage(["nuovo.txt"]).unwrap();
    let statuses = repository.status().unwrap();
    let file = statuses
        .iter()
        .find(|status| status.path == std::path::Path::new("nuovo.txt"))
        .expect("nuovo.txt torna non tracciato dopo unstage");
    assert_eq!(file.index, None);
    assert_eq!(file.worktree, Some(ChangeKind::Added));

    repository.stage(["nuovo.txt"]).unwrap();
    let file = repository.status().unwrap().remove(0);
    assert_eq!(file.index, Some(ChangeKind::Added));
    assert_eq!(file.worktree, None);

    fs::write(root.join("nuovo.txt"), "modificato\n").unwrap();
    let statuses = repository.status().unwrap();
    let file = statuses
        .iter()
        .find(|status| status.path == std::path::Path::new("nuovo.txt"))
        .expect("nuovo.txt modificato presente nello status");
    assert_eq!(file.index, Some(ChangeKind::Added));
    assert_eq!(file.worktree, Some(ChangeKind::Modified));

    fs::remove_file(root.join("nuovo.txt")).unwrap();
    let statuses = repository.status().unwrap();
    let file = statuses
        .iter()
        .find(|status| status.path == std::path::Path::new("nuovo.txt"))
        .expect("nuovo.txt cancellato presente nello status");
    assert_eq!(file.index, Some(ChangeKind::Added));
    assert_eq!(file.worktree, Some(ChangeKind::Removed));

    repository.stage(["nuovo.txt"]).unwrap();
    assert!(
        repository.status().unwrap().is_empty(),
        "staging della cancellazione rimuove il file nuovo dall'index"
    );

    let outside = root
        .parent()
        .expect("la temp dir ha un parent")
        .join("fuori.txt");
    assert!(matches!(
        repository.stage([outside]),
        Err(GitError::InvalidPath(_))
    ));

    fs::remove_dir_all(root).unwrap();
}

/// «File non versionati» non e' «aggiunto»: mettere in stage un file nuovo lo
/// sposta di gruppo, perche' da quel momento e' tracciato.
#[test]
fn mettere_in_stage_un_file_nuovo_lo_toglie_dai_non_versionati() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_git_untracked_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    gix::init(&root).unwrap();
    fs::write(root.join("nuovo.txt"), "contenuto\n").unwrap();
    let repository = GitRepository::discover(&root).unwrap();

    let file = repository.status().unwrap().remove(0);
    assert!(file.is_untracked(), "appena creato: non versionato");
    assert!(!file.is_staged());

    repository.stage(["nuovo.txt"]).unwrap();
    let file = repository.status().unwrap().remove(0);
    assert!(
        !file.is_untracked(),
        "in stage e' un file nuovo tracciato: va nel gruppo «Modifiche», non fra i non versionati"
    );
    assert!(file.is_staged());

    fs::remove_dir_all(root).unwrap();
}
