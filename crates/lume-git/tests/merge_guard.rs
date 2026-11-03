#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Protezioni già collegate a commit/stage: Git e conflitti reali, nessun pull
//! ancora simulato. Questi test non certificano Task 5 o Task 6 completi.
mod common;
use lume_git::{GitError, GitRepository};
use std::sync::atomic::AtomicBool;

fn conflict() -> (tempfile::TempDir, GitRepository) {
    let work = common::init_repo();
    common::commit_file(work.path(), "b.txt", "base\n", "base b");
    let repo = GitRepository::discover(work.path()).unwrap();
    let _ = repo.status().unwrap();
    common::git(work.path(), &["checkout", "-b", "altro"]);
    std::fs::write(work.path().join("a.txt"), "altrui\n").unwrap();
    std::fs::write(work.path().join("b.txt"), "altrui\n").unwrap();
    common::git(work.path(), &["commit", "-a", "-m", "altrui"]);
    common::git(work.path(), &["checkout", "main"]);
    std::fs::write(work.path().join("a.txt"), "mio\n").unwrap();
    std::fs::write(work.path().join("b.txt"), "mio\n").unwrap();
    common::git(work.path(), &["commit", "-a", "-m", "mio"]);
    let out = common::cli()
        .run(
            work.path(),
            &["merge", "--no-edit", "altro"],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(!out.success());
    (work, repo)
}
#[test]
fn commit_e_amend_rifiutano_merge_senza_scrivere_index_ref_o_marker() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (work, repo) = conflict();
    // Anche dopo risoluzione completa via Git il merge non può diventare
    // un commit ordinario con un solo genitore.
    for path in ["a.txt", "b.txt"] {
        std::fs::write(work.path().join(path), "risolto\n").unwrap();
    }
    common::git(work.path(), &["add", "a.txt", "b.txt"]);
    let files = [
        ".git/index",
        ".git/HEAD",
        ".git/refs/heads/main",
        ".git/MERGE_HEAD",
        ".git/MERGE_MSG",
        ".git/logs/HEAD",
        "a.txt",
        "b.txt",
    ];
    let before: Vec<_> = files
        .iter()
        .map(|p| std::fs::read(work.path().join(p)).unwrap())
        .collect();
    assert!(repo.merge_in_progress());
    for amend in [false, true] {
        assert!(matches!(
            repo.commit_with("non appiattire", amend),
            Err(GitError::MergeInProgress)
        ));
    }
    for (path, bytes) in files.into_iter().zip(before) {
        assert_eq!(
            std::fs::read(work.path().join(path)).unwrap(),
            bytes,
            "{path} non deve cambiare"
        );
    }
}
#[test]
fn stage_risolto_rimuove_solo_i_conflitti_di_quel_file_anche_con_cache_precedente() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (work, repo) = conflict();
    std::fs::write(work.path().join("a.txt"), "risolto\n").unwrap();
    repo.stage(["a.txt"]).unwrap();
    let unmerged = common::git(work.path(), &["ls-files", "-u"]);
    assert!(!unmerged.contains("a.txt"), "{unmerged}");
    assert!(unmerged.contains("b.txt"), "{unmerged}");
    std::fs::write(work.path().join("b.txt"), "risolto\n").unwrap();
    repo.stage(["b.txt"]).unwrap();
    assert_eq!(common::git(work.path(), &["ls-files", "-u"]), "");
    assert!(matches!(
        repo.commit("ancora merge"),
        Err(GitError::MergeInProgress)
    ));
}
#[test]
fn stage_di_una_risoluzione_per_cancellazione_elimina_gli_stage_1_2_3() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (work, repo) = conflict();
    std::fs::remove_file(work.path().join("a.txt")).unwrap();
    repo.stage(["a.txt"]).unwrap();
    let unmerged = common::git(work.path(), &["ls-files", "-u"]);
    assert!(!unmerged.contains("a.txt"));
    assert!(unmerged.contains("b.txt"));
}
#[test]
fn senza_merge_commit_ordinario_e_amend_restano_operativi() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(!repo.merge_in_progress());
    std::fs::write(work.path().join("a.txt"), "nuovo\n").unwrap();
    repo.stage(["a.txt"]).unwrap();
    repo.commit("ordinario").unwrap();
    repo.commit_with("corretto", true).unwrap();
    assert_eq!(
        common::git(work.path(), &["log", "-1", "--format=%s"]),
        "corretto"
    );
}
#[test]
fn marker_malformato_e_directory_fanno_rifiutare_commit() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    let repo = GitRepository::discover(work.path()).unwrap();
    std::fs::create_dir(work.path().join(".git/MERGE_HEAD")).unwrap();
    assert!(repo.merge_in_progress());
    assert!(matches!(
        repo.commit_with("", true),
        Err(GitError::MergeInProgress)
    ));
    #[cfg(unix)]
    {
        std::fs::remove_dir(work.path().join(".git/MERGE_HEAD")).unwrap();
        std::os::unix::fs::symlink("non-esiste", work.path().join(".git/MERGE_HEAD")).unwrap();
        assert!(repo.merge_in_progress());
        assert!(matches!(
            repo.commit("bloccato"),
            Err(GitError::MergeInProgress)
        ));
    }
}
#[test]
fn merge_marker_di_un_worktree_collegato_non_e_quello_del_principale() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    let linked = tempfile::tempdir().unwrap();
    common::git(
        work.path(),
        &[
            "worktree",
            "add",
            "-b",
            "collegato",
            &linked.path().to_string_lossy(),
        ],
    );
    let marker = common::git(linked.path(), &["rev-parse", "--git-path", "MERGE_HEAD"]);
    std::fs::write(marker, common::git(linked.path(), &["rev-parse", "HEAD"])).unwrap();
    let repo = GitRepository::discover(linked.path()).unwrap();
    assert!(repo.merge_in_progress());
    assert!(
        !GitRepository::discover(work.path())
            .unwrap()
            .merge_in_progress()
    );
    assert!(matches!(
        repo.commit("non appiattire"),
        Err(GitError::MergeInProgress)
    ));
}
