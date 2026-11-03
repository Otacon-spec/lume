#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Fetch e `has_origin` su repository veri: `origin` è un repository bare in
//! una cartella temporanea, mai la rete e mai il repository di chi lavora.

mod common;

use std::sync::atomic::AtomicBool;

use lume_git::{GitError, GitRepository};

#[test]
fn has_origin_e_falsa_senza_remote() {
    let work = common::init_repo();
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(!repo.has_origin());
}

#[test]
fn has_origin_e_vera_con_un_remote_mai_scaricato() {
    // Il caso che la vecchia `has_origin` sbagliava: «origin» è configurato
    // ma non esiste ancora nessun branch `origin/*`. Senza `has_origin` vera
    // il pulsante Fetch, che serve proprio a scaricarlo, non si disegnerebbe.
    let origin = common::bare_origin();
    let work = common::init_repo();
    common::git(
        work.path(),
        &["remote", "add", "origin", &origin.path().to_string_lossy()],
    );
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(repo.branches().unwrap().remote.is_empty());
    assert!(repo.has_origin());
}

#[test]
fn has_origin_ignora_un_remote_con_un_altro_nome() {
    let other = common::bare_origin();
    let work = common::init_repo();
    common::git(
        work.path(),
        &["remote", "add", "upstream", &other.path().to_string_lossy()],
    );
    let repo = GitRepository::discover(work.path()).unwrap();
    assert!(!repo.has_origin());
}

#[test]
fn fetch_scarica_i_commit_nuovi_e_non_tocca_il_branch_locale() {
    let (origin, work) = common::repo_with_origin();
    let other = common::clone_of(origin.path());
    common::commit_file(other.path(), "b.txt", "due\n", "secondo");
    common::git(other.path(), &["push", "origin", "main"]);
    let published = common::git(other.path(), &["rev-parse", "HEAD"]);
    let local_before = common::git(work.path(), &["rev-parse", "HEAD"]);

    let repo = GitRepository::discover(work.path()).unwrap();
    let name = repo.fetch_origin_with(&AtomicBool::new(false)).unwrap();

    assert_eq!(name, "origin");
    assert_eq!(
        common::git(work.path(), &["rev-parse", "origin/main"]),
        published
    );
    assert_eq!(
        common::git(work.path(), &["rev-parse", "HEAD"]),
        local_before
    );
    assert!(!work.path().join("b.txt").exists());
}

#[test]
fn fetch_origin_senza_flag_si_comporta_come_prima() {
    let (origin, work) = common::repo_with_origin();
    let other = common::clone_of(origin.path());
    common::commit_file(other.path(), "b.txt", "due\n", "secondo");
    common::git(other.path(), &["push", "origin", "main"]);
    let published = common::git(other.path(), &["rev-parse", "HEAD"]);

    let repo = GitRepository::discover(work.path()).unwrap();
    assert_eq!(repo.fetch_origin().unwrap(), "origin");
    assert_eq!(
        common::git(work.path(), &["rev-parse", "origin/main"]),
        published
    );
}

#[test]
fn fetch_annullato_prima_di_partire_non_scarica_niente() {
    let (origin, work) = common::repo_with_origin();
    let other = common::clone_of(origin.path());
    common::commit_file(other.path(), "b.txt", "due\n", "secondo");
    common::git(other.path(), &["push", "origin", "main"]);
    let known = common::git(work.path(), &["rev-parse", "origin/main"]);

    let repo = GitRepository::discover(work.path()).unwrap();
    let errore = repo.fetch_origin_with(&AtomicBool::new(true)).unwrap_err();

    assert!(matches!(errore, GitError::Cancelled), "{errore:?}");
    assert_eq!(
        common::git(work.path(), &["rev-parse", "origin/main"]),
        known
    );
}

#[test]
fn fetch_di_un_remote_inesistente_e_un_errore_di_fetch() {
    let work = common::init_repo();
    let repo = GitRepository::discover(work.path()).unwrap();
    let errore = repo
        .fetch_named("non-esiste", &AtomicBool::new(false))
        .unwrap_err();
    assert!(matches!(errore, GitError::Fetch(_)), "{errore:?}");
}
