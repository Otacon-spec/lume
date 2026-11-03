#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod common;
use lume_git::{GitError, GitRepository, Upstream};

#[test]
fn upstream_legge_remote_e_branch_con_barra_dalla_configurazione() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_origin, work) = common::repo_with_origin();
    common::git(work.path(), &["checkout", "-b", "feature/x"]);
    common::git(work.path(), &["push", "-u", "origin", "feature/x"]);
    assert_eq!(
        GitRepository::discover(work.path())
            .unwrap()
            .upstream()
            .unwrap(),
        Some(Upstream {
            remote: "origin".into(),
            branch: "feature/x".into(),
            tracking_ref: "refs/remotes/origin/feature/x".into()
        })
    );
}
#[test]
fn upstream_e_none_senza_configurazione_e_con_head_staccato() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    assert_eq!(
        GitRepository::discover(work.path())
            .unwrap()
            .upstream()
            .unwrap(),
        None
    );
    common::git(work.path(), &["checkout", "--detach"]);
    assert_eq!(
        GitRepository::discover(work.path())
            .unwrap()
            .upstream()
            .unwrap(),
        None
    );
}
#[test]
fn upstream_rifiuta_refspec_personalizzato_invece_di_inventare_tracking() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_origin, work) = common::repo_with_origin();
    common::git(
        work.path(),
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/main:refs/custom/main",
        ],
    );
    assert!(matches!(
        GitRepository::discover(work.path()).unwrap().upstream(),
        Err(GitError::NoUpstream(_))
    ));
}
#[test]
fn upstream_rifiuta_configurazione_ostile_prima_della_cli() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    common::git(
        work.path(),
        &["config", "branch.main.remote", "--upload-pack=calc"],
    );
    common::git(
        work.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    assert!(matches!(
        GitRepository::discover(work.path()).unwrap().upstream(),
        Err(GitError::InvalidBranchName(_))
    ));
}
