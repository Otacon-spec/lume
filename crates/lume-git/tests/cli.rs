#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lume_git::{GitCli, GitError};

#[test]
fn locate_in_una_cartella_senza_git_da_none() {
    let vuota = tempfile::tempdir().unwrap();
    assert!(GitCli::locate_in(&[vuota.path().to_path_buf()]).is_none());
}

#[test]
fn locate_in_scarta_le_voci_vuote_e_relative() {
    let voci = [
        PathBuf::new(),
        PathBuf::from("relativa"),
        PathBuf::from("."),
    ];
    assert!(GitCli::locate_in(&voci).is_none());
}

#[test]
fn locate_in_sceglie_la_prima_cartella_che_contiene_git() {
    // Un file col nome giusto basta: `locate_in` non esegue niente (nessuna
    // sonda, decisione G2); l'esecuzione la prova `run_restituisce_stdout`.
    let prima = tempfile::tempdir().unwrap();
    let seconda = tempfile::tempdir().unwrap();
    let nome = if cfg!(windows) { "git.exe" } else { "git" };
    std::fs::write(seconda.path().join(nome), b"").unwrap();
    let trovato =
        GitCli::locate_in(&[prima.path().to_path_buf(), seconda.path().to_path_buf()]).unwrap();
    assert_eq!(trovato.program(), seconda.path().join(nome));
}

#[test]
fn locate_trova_il_git_del_sistema() {
    assert!(GitCli::locate().is_some(), "serve git su PATH (P3)");
}

#[test]
fn run_restituisce_stdout_e_codice() {
    let repo = common::init_repo();
    let no = AtomicBool::new(false);
    let out = common::cli()
        .run(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"], &no)
        .unwrap();
    assert!(out.success());
    assert_eq!(out.stdout.trim(), "main");
    assert_eq!(out.code, Some(0));
}

#[test]
fn run_con_comando_fallito_riporta_stderr_e_codice() {
    let repo = common::init_repo();
    let no = AtomicBool::new(false);
    let out = common::cli()
        .run(repo.path(), &["checkout", "--", "non-esiste.txt"], &no)
        .unwrap();
    assert!(!out.success());
    assert!(out.code.is_some_and(|c| c != 0));
    assert!(!out.failure_text().trim().is_empty());
}

#[test]
fn run_annullato_prima_di_partire_non_lancia_niente() {
    let repo = common::init_repo();
    let si = AtomicBool::new(true);
    let errore = common::cli()
        .run(repo.path(), &["status"], &si)
        .unwrap_err();
    assert!(matches!(errore, GitError::Cancelled), "{errore:?}");
}

#[test]
fn run_annullato_durante_l_esecuzione_uccide_il_figlio() {
    let repo = common::init_repo();
    let flag = Arc::new(AtomicBool::new(false));
    let alzatore = {
        let flag = Arc::clone(&flag);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::Relaxed);
        })
    };
    let inizio = Instant::now();
    // Un alias di shell che dorme 30 s: e' `git` vero a lanciarlo.
    let risultato = common::cli().run(
        repo.path(),
        &["-c", "alias.attendi=!sleep 30", "attendi"],
        &flag,
    );
    alzatore.join().unwrap();
    assert!(
        matches!(risultato, Err(GitError::Cancelled)),
        "{risultato:?}"
    );
    assert!(
        inizio.elapsed() < Duration::from_secs(10),
        "l'annullamento deve interrompere l'attesa, non aspettarla: {:?}",
        inizio.elapsed()
    );
}

#[test]
fn run_limita_l_output_catturato() {
    let repo = common::init_repo();
    let no = AtomicBool::new(false);
    // 200000 caratteri su stdout: ne restano al massimo 64 KiB.
    let out = common::cli()
        .run(
            repo.path(),
            &["-c", "alias.tanto=!printf '%0200000d' 0", "tanto"],
            &no,
        )
        .unwrap();
    assert!(out.success(), "{}", out.failure_text());
    assert!(out.stdout.len() <= 64 * 1024, "{}", out.stdout.len());
    assert!(out.stdout.len() > 1000);
}
