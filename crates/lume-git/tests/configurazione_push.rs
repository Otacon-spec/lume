#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Configurazioni vere: nessuna destinazione remota indovinata o ampliata.
mod common;
use lume_git::{GitError, GitRepository, validate_ref_name};
use std::sync::atomic::AtomicBool;

#[test]
fn errore_nome_non_riporta_valore_arbitrario_dalla_configurazione() {
    let value = "--valore-privato";
    assert!(
        !validate_ref_name(value)
            .unwrap_err()
            .to_string()
            .contains(value)
    );
}
#[test]
fn upstream_detached_e_locale_non_vengono_spacciati_per_un_remote() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_origin, work) = common::repo_with_origin();
    common::git(work.path(), &["checkout", "--detach"]);
    assert!(
        GitRepository::discover(work.path())
            .unwrap()
            .upstream()
            .unwrap()
            .is_none()
    );
    common::git(work.path(), &["checkout", "main"]);
    common::git(work.path(), &["config", "branch.main.remote", "."]);
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(repo.upstream().unwrap().is_none());
    assert!(matches!(
        repo.push(&common::cli(), &AtomicBool::new(false)),
        Err(GitError::NoUpstream(_))
    ));
}
#[test]
fn configurazione_parziale_o_multi_merge_non_seleziona_silenziosamente_origin() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_origin, work) = common::repo_with_origin();
    common::git(work.path(), &["config", "--unset", "branch.main.merge"]);
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(matches!(repo.upstream(), Err(GitError::NoUpstream(_))));
    assert!(matches!(
        repo.push(&common::cli(), &AtomicBool::new(false)),
        Err(GitError::NoUpstream(_))
    ));
    common::git(
        work.path(),
        &["config", "--add", "branch.main.merge", "refs/heads/main"],
    );
    common::git(
        work.path(),
        &["config", "--add", "branch.main.merge", "refs/heads/altro"],
    );
    assert!(matches!(
        GitRepository::discover(work.path()).unwrap().upstream(),
        Err(GitError::NoUpstream(_))
    ));
}
#[test]
fn push_non_fa_mirror_anche_se_il_remote_lo_configura() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    for remote in ["origin", "origine=pari"] {
        let (origin, work) = common::repo_with_origin();
        if remote != "origin" {
            common::git(work.path(), &["remote", "rename", "origin", remote]);
        }
        common::commit_file(work.path(), "b.txt", "due\n", "secondo");
        common::git(work.path(), &["branch", "altro"]);
        let key = format!("remote.{remote}.mirror");
        common::git(work.path(), &["config", &key, "true"]);
        GitRepository::discover(work.path())
            .unwrap()
            .push(&common::cli(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            common::git(origin.path(), &["branch", "--format=%(refname:short)"]),
            "main"
        );
        assert_eq!(
            common::git(origin.path(), &["rev-parse", "main"]),
            common::git(work.path(), &["rev-parse", "HEAD"])
        );
        assert_eq!(
            common::git(work.path(), &["config", "--get", &key]),
            "true",
            "override non persistente"
        );
    }
}
