#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Merge CLI su repository Git temporanei. Nessuna fixture tocca il checkout utente.
mod common;
use lume_git::{ChangeKind, FileStatus, GitError, GitRepository, MergeOutcome};
use std::sync::atomic::AtomicBool;

fn no() -> AtomicBool {
    AtomicBool::new(false)
}

fn conflitto() -> tempfile::TempDir {
    let work = common::init_repo();
    common::git(work.path(), &["checkout", "-b", "incoming"]);
    std::fs::write(work.path().join("a.txt"), "remoto\n").unwrap();
    common::git(work.path(), &["add", "--", "a.txt"]);
    common::git(work.path(), &["commit", "-m", "incoming"]);
    common::git(work.path(), &["checkout", "main"]);
    std::fs::write(work.path().join("a.txt"), "locale\n").unwrap();
    common::git(work.path(), &["add", "--", "a.txt"]);
    common::git(work.path(), &["commit", "-m", "local"]);
    work
}

#[test]
fn merge_conflittuale_mantiene_index_reale_e_continue_crea_due_parent() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = conflitto();
    let repo = GitRepository::discover(work.path()).unwrap();
    let outcome = repo
        .merge_branch(&common::cli(), "incoming", &no())
        .unwrap();
    let MergeOutcome::Conflicts { paths, .. } = outcome else {
        panic!("conflitto atteso: {outcome:?}");
    };
    assert_eq!(paths, vec![std::path::PathBuf::from("a.txt")]);
    let unresolved_file = FileStatus {
        path: "a.txt".into(),
        index: Some(ChangeKind::Conflict),
        worktree: Some(ChangeKind::Conflict),
    };
    assert!(!unresolved_file.is_staged());
    assert!(!unresolved_file.is_partially_staged());
    assert_eq!(repo.merge_conflicts().unwrap(), paths);
    assert!(repo.merge_in_progress());
    assert!(common::git(work.path(), &["ls-files", "-u"]).contains("a.txt"));
    assert!(
        std::fs::read_to_string(work.path().join("a.txt"))
            .unwrap()
            .contains("<<<<<<<")
    );

    std::fs::write(work.path().join("a.txt"), "risolto\n").unwrap();
    repo.stage(["a.txt"]).unwrap();
    assert!(!common::git(work.path(), &["ls-files", "-u"]).contains("a.txt"));
    assert!(repo.merge_conflicts().unwrap().is_empty());
    let merge_commit = repo.merge_continue(&common::cli(), &no()).unwrap();
    assert!(!repo.merge_in_progress());
    let parents = common::git(work.path(), &["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(
        parents.split_whitespace().next(),
        Some(merge_commit.as_str())
    );
    assert_eq!(
        parents.split_whitespace().count(),
        3,
        "merge commit con due parent"
    );
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "risolto\n"
    );
}

#[test]
fn merge_abort_ripristina_head_e_worktree_pre_merge() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = conflitto();
    let head = common::git(work.path(), &["rev-parse", "HEAD"]);
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(matches!(
        repo.merge_branch(&common::cli(), "incoming", &no())
            .unwrap(),
        MergeOutcome::Conflicts { .. }
    ));
    repo.merge_abort(&common::cli(), &no()).unwrap();
    assert!(!repo.merge_in_progress());
    assert_eq!(common::git(work.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "locale\n"
    );
    assert_eq!(common::git(work.path(), &["ls-files", "-u"]), "");
}

#[test]
fn merge_rifiuta_worktree_sporco_prima_di_creare_marker() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = conflitto();
    common::git(work.path(), &["checkout", "--detach"]);
    common::git(work.path(), &["checkout", "-B", "main", "HEAD"]);
    std::fs::write(work.path().join("a.txt"), "bozza da tenere\n").unwrap();
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(matches!(
        repo.merge_branch(&common::cli(), "incoming", &no()),
        Err(GitError::PullDirty(_))
    ));
    assert!(!repo.merge_in_progress());
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "bozza da tenere\n"
    );
}

#[test]
fn merge_fast_forward_e_branch_gia_integrato_sono_distinti() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    common::git(work.path(), &["checkout", "-b", "incoming"]);
    common::commit_file(work.path(), "b.txt", "nuovo\n", "incoming");
    let tip = common::git(work.path(), &["rev-parse", "HEAD"]);
    common::git(work.path(), &["checkout", "main"]);
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(matches!(
        repo.merge_branch(&common::cli(), "incoming", &no()).unwrap(),
        MergeOutcome::FastForward { commit } if commit == tip
    ));
    assert!(matches!(
        repo.merge_branch(&common::cli(), "incoming", &no()).unwrap(),
        MergeOutcome::UpToDate { commit } if commit == tip
    ));
}
