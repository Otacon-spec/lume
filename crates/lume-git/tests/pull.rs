#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::{GitRepository, PullOutcome};
use std::sync::{Arc, atomic::AtomicBool};

fn divergent(conflict: bool) -> (tempfile::TempDir, tempfile::TempDir, String, String) {
    let (origin, work) = common::repo_with_origin();
    common::commit_file(
        work.path(),
        "shared.txt",
        "a\nb\nc\nd\ne\nf\ng\nh\n",
        "shared base",
    );
    common::git(work.path(), &["push", "origin", "main"]);
    let remote = common::clone_of(origin.path());
    let remote_text = "a\nremote-b\nc\nd\ne\nf\ng\nh\n";
    std::fs::write(remote.path().join("shared.txt"), remote_text).unwrap();
    common::git(remote.path(), &["add", "--", "shared.txt"]);
    common::git(remote.path(), &["commit", "-m", "remote change"]);
    common::git(remote.path(), &["push", "origin", "main"]);
    let upstream = common::git(remote.path(), &["rev-parse", "HEAD"]);
    let local_text = if conflict {
        "a\nlocal-b\nc\nd\ne\nf\ng\nh\n"
    } else {
        "a\nb\nc\nd\ne\nf\nlocal-g\nh\n"
    };
    std::fs::write(work.path().join("shared.txt"), local_text).unwrap();
    common::git(work.path(), &["add", "--", "shared.txt"]);
    common::git(work.path(), &["commit", "-m", "local change"]);
    let local = common::git(work.path(), &["rev-parse", "HEAD"]);
    (origin, work, local, upstream)
}

#[test]
fn pull_fast_forward_fetch_checkout_index_ref_e_reflog() {
    let (origin, work) = common::repo_with_origin();
    let remote = common::clone_of(origin.path());
    common::commit_file(remote.path(), "remote.txt", "remoto\n", "remote advance");
    common::git(remote.path(), &["push", "origin", "main"]);
    let before = common::git(work.path(), &["rev-parse", "HEAD"]);
    let outcome = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert!(matches!(outcome, PullOutcome::FastForward { .. }));
    assert_ne!(common::git(work.path(), &["rev-parse", "HEAD"]), before);
    assert_eq!(
        common::git(work.path(), &["show", "HEAD:remote.txt"]),
        "remoto"
    );
    assert_eq!(
        common::git(work.path(), &["write-tree"]),
        common::git(work.path(), &["rev-parse", "HEAD^{tree}"])
    );
    let reflog = std::fs::read_to_string(work.path().join(".git/logs/HEAD")).unwrap();
    assert!(
        reflog
            .lines()
            .last()
            .unwrap()
            .ends_with("\tpull: fast-forward")
    );
}

#[test]
fn pull_fast_forward_applica_i_nuovi_gitattributes_alla_checkout() {
    let (origin, work) = common::repo_with_origin();
    let remote = common::clone_of(origin.path());
    std::fs::write(
        remote.path().join(".gitattributes"),
        b"attrs-target.txt text eol=crlf\n",
    )
    .unwrap();
    std::fs::write(
        remote.path().join("attrs-target.txt"),
        b"riga uno\nriga due\n",
    )
    .unwrap();
    common::git(
        remote.path(),
        &["add", "--", ".gitattributes", "attrs-target.txt"],
    );
    common::git(
        remote.path(),
        &["commit", "-m", "aggiunge attributi e file"],
    );
    common::git(remote.path(), &["push", "origin", "main"]);

    let outcome = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap();

    assert!(matches!(outcome, PullOutcome::FastForward { .. }));
    assert_eq!(
        std::fs::read(work.path().join("attrs-target.txt")).unwrap(),
        b"riga uno\r\nriga due\r\n"
    );
    assert_eq!(
        common::git(work.path(), &["show", "HEAD:attrs-target.txt"]),
        "riga uno\nriga due"
    );
}

#[cfg(unix)]
#[test]
fn pull_fast_forward_applica_executable_git_senza_allargare_permessi() {
    use std::os::unix::fs::PermissionsExt;

    let (origin, work) = common::repo_with_origin();
    let remote = common::clone_of(origin.path());
    std::fs::write(remote.path().join("script.sh"), b"#!/bin/sh\necho ok\n").unwrap();
    common::git(remote.path(), &["add", "--", "script.sh"]);
    common::git(remote.path(), &["update-index", "--chmod=+x", "script.sh"]);
    common::git(
        remote.path(),
        &["commit", "-m", "aggiunge script eseguibile"],
    );
    common::git(remote.path(), &["push", "origin", "main"]);

    let outcome = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap();

    assert!(matches!(outcome, PullOutcome::FastForward { .. }));
    let mode = std::fs::metadata(work.path().join("script.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o700,
        0o700,
        "il bit Git eseguibile usa il bit utente"
    );
    assert_eq!(mode & 0o077, 0, "il pull non deve allargare l'accesso");
    assert_eq!(
        common::git(work.path(), &["ls-tree", "HEAD", "script.sh"])
            .split_whitespace()
            .next(),
        Some("100755")
    );
}

#[test]
fn pull_fast_forward_pubblica_transizione_file_a_directory() {
    let (origin, work) = common::repo_with_origin();
    let remote = common::clone_of(origin.path());
    std::fs::remove_file(remote.path().join("a.txt")).unwrap();
    std::fs::create_dir(remote.path().join("a.txt")).unwrap();
    std::fs::write(remote.path().join("a.txt/figlio.txt"), b"figlio remoto\n").unwrap();
    common::git(remote.path(), &["add", "--all"]);
    common::git(remote.path(), &["commit", "-m", "file diventa directory"]);
    common::git(remote.path(), &["push", "origin", "main"]);

    let outcome = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap();

    let PullOutcome::FastForward { receipt, .. } = outcome else {
        panic!("fast-forward atteso: {outcome:?}");
    };
    assert_eq!(receipt.updated_files(), 1);
    assert_eq!(receipt.removed_files(), 1);
    assert_eq!(receipt.created_directories(), 1);
    assert!(!work.path().join("a.txt").is_file());
    assert_eq!(
        std::fs::read(work.path().join("a.txt/figlio.txt")).unwrap(),
        b"figlio remoto\n"
    );
    assert_eq!(
        common::git(work.path(), &["write-tree"]),
        common::git(work.path(), &["rev-parse", "HEAD^{tree}"])
    );
}

#[test]
fn pull_fast_forward_pubblica_transizione_directory_a_file() {
    let (origin, work) = common::repo_with_origin();
    std::fs::remove_file(work.path().join("a.txt")).unwrap();
    std::fs::create_dir(work.path().join("a.txt")).unwrap();
    std::fs::write(work.path().join("a.txt/figlio.txt"), b"figlio base\n").unwrap();
    common::git(work.path(), &["add", "--all"]);
    common::git(work.path(), &["commit", "-m", "base directory"]);
    common::git(work.path(), &["push", "origin", "main"]);
    let remote = common::clone_of(origin.path());
    std::fs::remove_file(remote.path().join("a.txt/figlio.txt")).unwrap();
    std::fs::remove_dir(remote.path().join("a.txt")).unwrap();
    std::fs::write(remote.path().join("a.txt"), b"file remoto\n").unwrap();
    common::git(remote.path(), &["add", "--all"]);
    common::git(remote.path(), &["commit", "-m", "directory diventa file"]);
    common::git(remote.path(), &["push", "origin", "main"]);

    let outcome = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap();

    let PullOutcome::FastForward { receipt, .. } = outcome else {
        panic!("fast-forward atteso: {outcome:?}");
    };
    assert_eq!(receipt.updated_files(), 1);
    assert_eq!(receipt.removed_files(), 1);
    assert_eq!(receipt.created_directories(), 0);
    assert!(work.path().join("a.txt").is_file());
    assert_eq!(
        std::fs::read(work.path().join("a.txt")).unwrap(),
        b"file remoto\n"
    );
    assert_eq!(
        common::git(work.path(), &["write-tree"]),
        common::git(work.path(), &["rev-parse", "HEAD^{tree}"])
    );
}

#[test]
fn pull_merge_pulito_unisce_modifiche_disgiunte_sullo_stesso_file() {
    let (_origin, work, local, upstream) = divergent(false);
    let outcome = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap();
    let PullOutcome::Merged { commit, .. } = outcome else {
        panic!("merge pulito atteso: {outcome:?}");
    };
    assert_eq!(
        std::fs::read(work.path().join("shared.txt")).unwrap(),
        b"a\nremote-b\nc\nd\ne\nf\nlocal-g\nh\n"
    );
    let parents = common::git(work.path(), &["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(parents, format!("{commit} {local} {upstream}"));
    assert_eq!(
        common::git(work.path(), &["write-tree"]),
        common::git(work.path(), &["rev-parse", "HEAD^{tree}"])
    );
    let reflog = std::fs::read_to_string(work.path().join(".git/logs/HEAD")).unwrap();
    assert!(reflog.lines().last().unwrap().ends_with("\tpull: merge"));
}

#[test]
fn pull_conflitto_non_pubblica_checkout_index_head_o_reflog() {
    let (_origin, work, local, upstream) = divergent(true);
    let head_log = std::fs::read(work.path().join(".git/logs/HEAD")).unwrap();
    let index = std::fs::read(work.path().join(".git/index")).unwrap();
    let tree = common::git(work.path(), &["write-tree"]);
    let outcome = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap();
    let PullOutcome::NeedsMerge {
        upstream_commit,
        conflicts,
    } = outcome
    else {
        panic!("conflitto atteso: {outcome:?}");
    };
    assert_eq!(upstream_commit, upstream);
    assert!(conflicts.contains(&std::path::PathBuf::from("shared.txt")));
    assert_eq!(common::git(work.path(), &["rev-parse", "HEAD"]), local);
    assert_eq!(common::git(work.path(), &["write-tree"]), tree);
    assert_eq!(
        std::fs::read(work.path().join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/logs/HEAD")).unwrap(),
        head_log
    );
    assert_eq!(
        std::fs::read(work.path().join("shared.txt")).unwrap(),
        b"a\nlocal-b\nc\nd\ne\nf\ng\nh\n"
    );
    assert_eq!(
        common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]),
        upstream
    );
}

#[test]
fn pull_distingue_up_to_date_e_branch_locale_ahead() {
    let (_origin, work) = common::repo_with_origin();
    assert!(matches!(
        GitRepository::discover(work.path())
            .unwrap()
            .pull_with(Arc::new(AtomicBool::new(false)))
            .unwrap(),
        PullOutcome::UpToDate { .. }
    ));
    common::commit_file(work.path(), "locale.txt", "locale\n", "local only");
    assert!(matches!(
        GitRepository::discover(work.path())
            .unwrap()
            .pull_with(Arc::new(AtomicBool::new(false)))
            .unwrap(),
        PullOutcome::LocalAhead { .. }
    ));
}

#[test]
fn pull_dirty_toccato_dopo_merge_preserva_worktree_index_head() {
    let (_origin, work, local, upstream) = divergent(false);
    let dirty = b"modifica locale da preservare\n";
    std::fs::write(work.path().join("shared.txt"), dirty).unwrap();
    let index = std::fs::read(work.path().join(".git/index")).unwrap();
    let head_log = std::fs::read(work.path().join(".git/logs/HEAD")).unwrap();
    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();
    assert_eq!(error.stage, lume_git::PullFailureStage::Prepare);
    assert_eq!(error.fetch, lume_git::PullFetchState::Completed);
    assert_eq!(common::git(work.path(), &["rev-parse", "HEAD"]), local);
    assert_eq!(
        common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]),
        upstream
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/logs/HEAD")).unwrap(),
        head_log
    );
    assert_eq!(
        std::fs::read(work.path().join("shared.txt")).unwrap(),
        dirty
    );
}

#[test]
fn pull_preserva_modifiche_worktree_su_un_file_non_toccato() {
    let (_origin, work, _local, _upstream) = divergent(false);
    let local_bytes = b"contenuto locale non coinvolto\n";
    std::fs::write(work.path().join("a.txt"), local_bytes).unwrap();

    let outcome = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap();

    assert!(matches!(outcome, PullOutcome::Merged { .. }));
    assert_eq!(
        std::fs::read(work.path().join("a.txt")).unwrap(),
        local_bytes
    );
    assert_eq!(
        std::fs::read(work.path().join("shared.txt")).unwrap(),
        b"a\nremote-b\nc\nd\ne\nf\nlocal-g\nh\n"
    );
    assert!(common::git(work.path(), &["status", "--porcelain"]).contains("a.txt"));
}

#[test]
fn pull_rifiuta_staged_sul_percorso_toccato_e_preserva_index() {
    let (_origin, work, local, upstream) = divergent(false);
    std::fs::write(work.path().join("shared.txt"), b"staged locale\n").unwrap();
    common::git(work.path(), &["add", "--", "shared.txt"]);
    let staged_index = std::fs::read(work.path().join(".git/index")).unwrap();
    let staged_worktree = std::fs::read(work.path().join("shared.txt")).unwrap();
    let head_log = std::fs::read(work.path().join(".git/logs/HEAD")).unwrap();

    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();

    assert_eq!(error.stage, lume_git::PullFailureStage::Prepare);
    assert_eq!(error.fetch, lume_git::PullFetchState::Completed);
    assert_eq!(common::git(work.path(), &["rev-parse", "HEAD"]), local);
    assert_eq!(
        common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]),
        upstream
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/index")).unwrap(),
        staged_index
    );
    assert_eq!(
        std::fs::read(work.path().join("shared.txt")).unwrap(),
        staged_worktree
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/logs/HEAD")).unwrap(),
        head_log
    );
}

#[test]
fn pull_rifiuta_staged_globale_fuori_dal_delta_e_preserva_tutto() {
    let (_origin, work, local, upstream) = divergent(false);
    let staged = b"staged locale su file indipendente\n";
    std::fs::write(work.path().join("a.txt"), staged).unwrap();
    common::git(work.path(), &["add", "--", "a.txt"]);
    let index = std::fs::read(work.path().join(".git/index")).unwrap();
    let head_log = std::fs::read(work.path().join(".git/logs/HEAD")).unwrap();

    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();

    assert_eq!(error.stage, lume_git::PullFailureStage::Prepare);
    assert_eq!(error.fetch, lume_git::PullFetchState::Completed);
    assert_eq!(common::git(work.path(), &["rev-parse", "HEAD"]), local);
    assert_eq!(
        common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]),
        upstream
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/logs/HEAD")).unwrap(),
        head_log
    );
    assert_eq!(std::fs::read(work.path().join("a.txt")).unwrap(), staged);
    assert_eq!(
        std::fs::read(work.path().join("shared.txt")).unwrap(),
        b"a\nb\nc\nd\ne\nf\nlocal-g\nh\n"
    );
}

#[test]
fn pull_rifiuta_untracked_che_verrebbe_sovrascritto_e_lo_conserva() {
    let (origin, work, local, _upstream) = divergent(false);
    let remote = common::clone_of(origin.path());
    common::commit_file(
        remote.path(),
        "untracked-from-remote.txt",
        "remoto\n",
        "aggiunge file",
    );
    common::git(remote.path(), &["push", "origin", "main"]);
    let upstream = common::git(remote.path(), &["rev-parse", "HEAD"]);
    let local_bytes = b"contenuto non tracciato locale\n";
    std::fs::write(work.path().join("untracked-from-remote.txt"), local_bytes).unwrap();
    let head_log = std::fs::read(work.path().join(".git/logs/HEAD")).unwrap();
    let index = std::fs::read(work.path().join(".git/index")).unwrap();

    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();

    assert_eq!(error.stage, lume_git::PullFailureStage::Prepare);
    assert_eq!(error.fetch, lume_git::PullFetchState::Completed);
    assert_eq!(common::git(work.path(), &["rev-parse", "HEAD"]), local);
    assert_eq!(
        common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]),
        upstream
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/logs/HEAD")).unwrap(),
        head_log
    );
    assert_eq!(
        std::fs::read(work.path().join("untracked-from-remote.txt")).unwrap(),
        local_bytes
    );
}

#[test]
fn pull_rifiuta_driver_merge_esterno_prima_di_avviarlo() {
    let (_origin, work, local, upstream) = divergent(false);
    let marker = work.path().join("driver-eseguito");
    common::git(
        work.path(),
        &[
            "config",
            "merge.sospetto.driver",
            &format!("touch {}", marker.display()),
        ],
    );
    let head_log = std::fs::read(work.path().join(".git/logs/HEAD")).unwrap();
    let index = std::fs::read(work.path().join(".git/index")).unwrap();
    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();
    assert_eq!(error.stage, lume_git::PullFailureStage::Merge);
    assert_eq!(error.fetch, lume_git::PullFetchState::Completed);
    assert!(!marker.exists());
    assert_eq!(common::git(work.path(), &["rev-parse", "HEAD"]), local);
    assert_eq!(
        common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]),
        upstream
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/logs/HEAD")).unwrap(),
        head_log
    );
}

#[test]
fn pull_detached_rifiuta_prima_del_fetch_e_non_avanza_tracking_ref() {
    let (origin, work) = common::repo_with_origin();
    common::git(work.path(), &["checkout", "--detach", "HEAD"]);
    let head = common::git(work.path(), &["rev-parse", "HEAD"]);
    let tracking_before = common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]);
    let head_log = std::fs::read(work.path().join(".git/logs/HEAD")).unwrap();
    let remote = common::clone_of(origin.path());
    common::commit_file(remote.path(), "remote.txt", "remoto\n", "remote advance");
    common::git(remote.path(), &["push", "origin", "main"]);

    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();

    assert_eq!(error.stage, lume_git::PullFailureStage::Configuration);
    assert_eq!(error.fetch, lume_git::PullFetchState::NotStarted);
    assert_eq!(common::git(work.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]),
        tracking_before
    );
    assert_eq!(
        std::fs::read(work.path().join(".git/logs/HEAD")).unwrap(),
        head_log
    );
    assert!(!work.path().join("remote.txt").exists());
}

#[test]
fn pull_senza_upstream_merge_attivo_o_cancelled_non_avvia_fetch() {
    let no_remote = common::init_repo();
    let error = GitRepository::discover(no_remote.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();
    assert_eq!(error.stage, lume_git::PullFailureStage::Configuration);
    assert_eq!(error.fetch, lume_git::PullFetchState::NotStarted);

    let (_origin, work) = common::repo_with_origin();
    std::fs::write(work.path().join(".git/MERGE_HEAD"), b"merge-in-progress\n").unwrap();
    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();
    assert_eq!(error.fetch, lume_git::PullFetchState::NotStarted);
    std::fs::remove_file(work.path().join(".git/MERGE_HEAD")).unwrap();

    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(true)))
        .unwrap_err();
    assert_eq!(error.fetch, lume_git::PullFetchState::NotStarted);
}

#[test]
fn pull_non_avvia_ssh_o_remote_helper_fuori_worker_confinato() {
    let (_origin, work) = common::repo_with_origin();
    common::git(
        work.path(),
        &["remote", "set-url", "origin", "ssh://example.invalid/repo"],
    );
    let error = GitRepository::discover(work.path())
        .unwrap()
        .pull_with(Arc::new(AtomicBool::new(false)))
        .unwrap_err();
    assert_eq!(error.stage, lume_git::PullFailureStage::Configuration);
    assert_eq!(error.fetch, lume_git::PullFetchState::NotStarted);
    assert_eq!(
        common::git(work.path(), &["rev-parse", "refs/remotes/origin/main"]),
        common::git(work.path(), &["rev-parse", "HEAD"])
    );
}
