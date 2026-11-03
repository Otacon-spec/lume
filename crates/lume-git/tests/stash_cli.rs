#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Apply e pop con Git reale, in repository temporanei posseduti dal test.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

#[cfg(unix)]
use lume_git::GitCli;
use lume_git::{GitError, GitRepository, StashApplyOutcome};

fn no() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn open(dir: &Path) -> GitRepository {
    GitRepository::discover(dir).unwrap()
}

fn stash_di(work: &Path, contenuto: &str, nome: &str) {
    std::fs::write(work.join("a.txt"), contenuto).unwrap();
    // Questi test verificano apply/pop interoperabili: partono da uno stash
    // Git completo. stash_push dell'API crea invece soltanto uno snapshot.
    common::git(work, &["stash", "push", "-m", nome]);
}

#[test]
fn stash_save_salva_tracked_stage_e_untracked_e_ripristina_head() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    std::fs::write(work.path().join("a.txt"), "modificato\n").unwrap();
    std::fs::write(work.path().join("staged.txt"), "in stage\n").unwrap();
    common::git(work.path(), &["add", "staged.txt"]);
    std::fs::write(work.path().join("b.txt"), "non versionato\n").unwrap();

    let created = open(work.path())
        .stash_save(&common::cli(), Some("salvataggio completo"), &no())
        .unwrap();

    assert!(created);
    assert_eq!(std::fs::read(work.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!work.path().join("staged.txt").exists());
    assert!(!work.path().join("b.txt").exists());
    let entries = open(work.path()).stash_list().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].message.ends_with("salvataggio completo"));
    assert_eq!(
        common::git(work.path(), &["show", "stash@{0}:staged.txt"]),
        "in stage"
    );
    assert_eq!(
        common::git(work.path(), &["stash", "list"]).lines().count(),
        1
    );
}

#[test]
fn stash_save_senza_modifiche_non_crea_una_voce() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    assert!(
        !open(work.path())
            .stash_save(&common::cli(), None, &no())
            .unwrap()
    );
    assert!(open(work.path()).stash_list().unwrap().is_empty());
}

#[test]
#[cfg(unix)]
fn stash_save_rifiuta_il_filtro_clean_selezionato_senza_avviare_helper() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    let helper = tempfile::tempdir().unwrap();
    let marker = helper.path().join("avviato");
    common::git(
        work.path(),
        &[
            "config",
            "filter.cleanup.clean",
            &format!("touch '{}'", marker.display()),
        ],
    );
    common::git(work.path(), &["config", "filter.cleanup.required", "true"]);
    std::fs::write(
        work.path().join(".gitattributes"),
        b"a.txt filter=cleanup\n",
    )
    .unwrap();
    std::fs::write(work.path().join("a.txt"), "modificato\n").unwrap();

    let result = open(work.path()).stash_save(&common::cli(), None, &no());

    assert!(
        matches!(result, Err(GitError::Worktree(ref error)) if error.contains("filtro comando selezionato")),
        "{result:?}"
    );
    assert!(!marker.exists());
    assert_eq!(
        std::fs::read(work.path().join("a.txt")).unwrap(),
        b"modificato\n"
    );
    assert!(open(work.path()).stash_list().unwrap().is_empty());
}

#[test]
fn apply_riapplica_le_modifiche_e_mantiene_la_voce() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "modificato\n", "wip");
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "uno\n"
    );

    let outcome = open(work.path())
        .stash_apply(&common::cli(), 0, no())
        .unwrap();

    assert_eq!(outcome, StashApplyOutcome::Applied);
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "modificato\n"
    );
    assert_eq!(open(work.path()).stash_list().unwrap().len(), 1);
}

#[test]
fn pop_riapplica_le_modifiche_e_toglie_la_voce() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "modificato\n", "wip");

    let outcome = open(work.path())
        .stash_pop(&common::cli(), 0, no())
        .unwrap();

    assert_eq!(outcome, StashApplyOutcome::Applied);
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "modificato\n"
    );
    assert!(open(work.path()).stash_list().unwrap().is_empty());
    assert_eq!(common::git(work.path(), &["stash", "list"]), "");
}

#[test]
fn pop_di_una_voce_in_mezzo_applica_quella_giusta() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "primo\n", "primo");
    stash_di(work.path(), "secondo\n", "secondo");

    open(work.path())
        .stash_pop(&common::cli(), 1, no())
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "primo\n"
    );
    let rimasti = open(work.path()).stash_list().unwrap();
    assert_eq!(rimasti.len(), 1);
    assert!(
        rimasti[0].message.ends_with("secondo"),
        "{}",
        rimasti[0].message
    );
}

#[test]
fn apply_ripristina_anche_cio_che_era_in_stage() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    std::fs::write(work.path().join("b.txt"), "nuovo\n").unwrap();
    common::git(work.path(), &["add", "b.txt"]);
    stash_di(work.path(), "modificato\n", "misto");
    assert!(!work.path().join("b.txt").exists());

    open(work.path())
        .stash_apply(&common::cli(), 0, no())
        .unwrap();

    assert_eq!(
        common::git(work.path(), &["diff", "--cached", "--name-only"]),
        "b.txt"
    );
    assert_eq!(common::git(work.path(), &["diff", "--name-only"]), "a.txt");
}

#[test]
fn pop_con_conflitto_lascia_lo_stash_e_i_marcatori() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "dallo stash\n", "wip");
    std::fs::write(work.path().join("a.txt"), "dal commit\n").unwrap();
    common::git(work.path(), &["commit", "-a", "-m", "cambia a"]);

    let outcome = open(work.path())
        .stash_pop(&common::cli(), 0, no())
        .unwrap();

    assert_eq!(
        outcome,
        StashApplyOutcome::Conflicts(vec![PathBuf::from("a.txt")])
    );
    let testo = std::fs::read_to_string(work.path().join("a.txt")).unwrap();
    assert!(testo.contains("<<<<<<<"), "{testo}");
    assert_eq!(open(work.path()).stash_list().unwrap().len(), 1);
}

#[test]
fn apply_rifiuta_un_indice_inesistente() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    let errore = open(work.path())
        .stash_apply(&common::cli(), 0, no())
        .unwrap_err();
    assert!(matches!(errore, GitError::Stash(_)), "{errore:?}");
}

#[test]
fn apply_su_modifiche_locali_che_verrebbero_perse_e_un_errore_e_non_le_tocca() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "dallo stash\n", "wip");
    std::fs::write(work.path().join("a.txt"), "lavoro non salvato\n").unwrap();

    let errore = open(work.path())
        .stash_apply(&common::cli(), 0, no())
        .unwrap_err();

    assert!(matches!(errore, GitError::Stash(_)), "{errore:?}");
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "lavoro non salvato\n"
    );
    assert_eq!(open(work.path()).stash_list().unwrap().len(), 1);
}

#[test]
fn apply_annullato_prima_di_partire_non_fa_niente() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "modificato\n", "wip");
    let errore = open(work.path())
        .stash_apply(&common::cli(), 0, Arc::new(AtomicBool::new(true)))
        .unwrap_err();
    assert!(matches!(errore, GitError::Cancelled), "{errore:?}");
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "uno\n"
    );
}

/// Il menu seleziona un OID dal reflog, ma un altro processo può rimuovere una
/// voce prima che parta Git. L'applicazione deve restare legata all'OID letto
/// dal preflight e non reinterpretare lo stesso indice come un'altra voce.
#[test]
#[cfg(unix)]
fn apply_usa_l_oid_selezionato_se_il_reflog_cambia_prima_del_comando() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "selezionato\n", "selezionato");
    stash_di(work.path(), "diventa_indice_zero\n", "diventa indice zero");

    let wrapper_dir = tempfile::tempdir().unwrap();
    let wrapper = wrapper_dir.path().join("git");
    std::fs::write(
        &wrapper,
        b"#!/bin/sh\nif [ \"$1\" = stash ] && { [ \"$2\" = apply ] || [ \"$2\" = pop ]; }; then\n  git stash drop 0 || exit $?\nfi\nexec git \"$@\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cli = GitCli::locate_in(&[wrapper_dir.path().to_path_buf()]).unwrap();

    let result = open(work.path()).stash_apply(&cli, 0, no()).unwrap();

    assert_eq!(result, StashApplyOutcome::Applied);
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "diventa_indice_zero\n",
        "la voce inizialmente selezionata deve essere applicata anche se l'indice cambia"
    );
    assert_eq!(open(work.path()).stash_list().unwrap().len(), 1);
}

/// `pop` deve applicare e rimuovere l'OID selezionato, mai il nuovo indice 0
/// dopo che un altro processo ha tolto la voce più recente.
#[test]
#[cfg(unix)]
fn pop_non_rimuove_un_altro_stash_se_il_reflog_cambia_prima_del_comando() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "selezionato\n", "selezionato");
    stash_di(work.path(), "diventa_indice_zero\n", "diventa indice zero");

    let wrapper_dir = tempfile::tempdir().unwrap();
    let wrapper = wrapper_dir.path().join("git");
    std::fs::write(
        &wrapper,
        b"#!/bin/sh\nif [ \"$1\" = stash ] && { [ \"$2\" = apply ] || [ \"$2\" = pop ]; }; then\n  git stash drop 0 || exit $?\nfi\nexec git \"$@\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cli = GitCli::locate_in(&[wrapper_dir.path().to_path_buf()]).unwrap();

    let result = open(work.path()).stash_pop(&cli, 0, no()).unwrap();

    assert_eq!(result, StashApplyOutcome::Applied);
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "diventa_indice_zero\n"
    );
    let remaining = open(work.path()).stash_list().unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].message.ends_with("selezionato"));
}

/// Se l'altro processo rimuove l'unica voce, pop applica comunque l'OID
/// selezionato e tratta la rimozione già avvenuta come completata.
#[test]
#[cfg(unix)]
fn pop_accetta_che_l_unico_stash_sia_gia_stato_rimosso() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    stash_di(work.path(), "selezionato\n", "selezionato");

    let wrapper_dir = tempfile::tempdir().unwrap();
    let wrapper = wrapper_dir.path().join("git");
    std::fs::write(
        &wrapper,
        b"#!/bin/sh\nif [ \"$1\" = stash ] && { [ \"$2\" = apply ] || [ \"$2\" = pop ]; }; then\n  git stash drop 0 || exit $?\nfi\nexec git \"$@\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cli = GitCli::locate_in(&[wrapper_dir.path().to_path_buf()]).unwrap();

    let result = open(work.path()).stash_pop(&cli, 0, no()).unwrap();

    assert_eq!(result, StashApplyOutcome::Applied);
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "selezionato\n"
    );
    assert!(open(work.path()).stash_list().unwrap().is_empty());
}
