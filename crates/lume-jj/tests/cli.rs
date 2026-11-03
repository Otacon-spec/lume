#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lume_jj::{JjCli, JjError, JjOutput};

fn nome() -> &'static str {
    if cfg!(windows) { "jj.exe" } else { "jj" }
}

#[test]
fn locate_in_una_cartella_senza_jj_da_none() {
    let vuota = tempfile::tempdir().unwrap();
    assert!(JjCli::locate_in(&[vuota.path().to_path_buf()]).is_none());
}

#[test]
fn locate_in_scarta_le_voci_vuote_e_relative() {
    let voci = [
        PathBuf::new(),
        PathBuf::from("relativa"),
        PathBuf::from("."),
    ];
    assert!(JjCli::locate_in(&voci).is_none());
}

#[test]
fn locate_in_sceglie_la_prima_cartella_che_contiene_jj() {
    // Un file col nome giusto basta: `locate_in` non esegue niente (nessuna
    // sonda `--version`, G3); l'esecuzione la provano gli altri test.
    let prima = tempfile::tempdir().unwrap();
    let seconda = tempfile::tempdir().unwrap();
    std::fs::write(seconda.path().join(nome()), b"").unwrap();
    let trovato =
        JjCli::locate_in(&[prima.path().to_path_buf(), seconda.path().to_path_buf()]).unwrap();
    assert_eq!(trovato.program(), seconda.path().join(nome()));
}

#[test]
fn run_antepone_le_opzioni_globali_e_imposta_l_ambiente() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("status", "ciao\n", 0);
    let lavoro = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    fake.cli().run(lavoro.path(), &["status"], &no).unwrap();

    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args, ["--no-pager", "--color", "never", "status"]);
    assert_eq!(calls[0].env["JJ_EDITOR"].as_deref(), Some("true"));
    assert_eq!(calls[0].env["GIT_TERMINAL_PROMPT"].as_deref(), Some("0"));
    assert_eq!(calls[0].env["GIT_EDITOR"].as_deref(), Some("true"));
    assert!(
        common::same_dir(&calls[0].cwd, lavoro.path()),
        "{:?}",
        calls[0].cwd
    );
}

#[test]
fn run_restituisce_stdout_stderr_e_codice() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("status", "fuori", 0);
    fake.script_err("status", "avviso");
    let lavoro = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let out = fake.cli().run(lavoro.path(), &["status"], &no).unwrap();

    assert!(out.success());
    assert_eq!(out.stdout, "fuori");
    assert_eq!(out.stderr, "avviso");
    assert_eq!(out.code, Some(0));
}

#[test]
fn run_con_comando_fallito_riporta_stderr_e_codice() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("status", "", 1);
    fake.script_err("status", "Error: There is no jj repo in \".\"\n");
    let lavoro = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let out = fake.cli().run(lavoro.path(), &["status"], &no).unwrap();

    assert!(!out.success());
    assert_eq!(out.code, Some(1));
    assert!(out.failure_text().contains("There is no jj repo"));
}

#[test]
fn failure_text_senza_testo_riporta_il_codice() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("status", "", 3);
    let lavoro = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let out = fake.cli().run(lavoro.path(), &["status"], &no).unwrap();

    assert_eq!(out.failure_text(), "jj è uscito con codice 3");
}

#[test]
fn message_unisce_stdout_e_stderr_non_vuoti() {
    let solo_out = JjOutput {
        stdout: "a\n".into(),
        stderr: String::new(),
        code: Some(0),
    };
    let entrambi = JjOutput {
        stdout: "a\n".into(),
        stderr: "b\n".into(),
        code: Some(0),
    };
    let nessuno = JjOutput {
        stdout: " \n".into(),
        stderr: String::new(),
        code: Some(0),
    };
    assert_eq!(solo_out.message(), "a");
    assert_eq!(entrambi.message(), "a\nb");
    assert_eq!(nessuno.message(), "");
}

#[test]
fn run_annullato_prima_di_partire_non_lancia_niente() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    let lavoro = tempfile::tempdir().unwrap();
    let si = AtomicBool::new(true);

    let errore = fake.cli().run(lavoro.path(), &["status"], &si).unwrap_err();

    assert!(matches!(errore, JjError::Cancelled), "{errore:?}");
    assert!(fake.calls().is_empty(), "nessun processo doveva partire");
}

#[test]
fn run_annullato_durante_l_esecuzione_uccide_il_figlio() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.slow("status");
    let lavoro = tempfile::tempdir().unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let alzatore = {
        let flag = Arc::clone(&flag);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::Relaxed);
        })
    };
    let inizio = Instant::now();

    let risultato = fake.cli().run(lavoro.path(), &["status"], &flag);

    alzatore.join().unwrap();
    assert!(
        matches!(risultato, Err(JjError::Cancelled)),
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
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("status", &"x".repeat(200_000), 0);
    let lavoro = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let out = fake.cli().run(lavoro.path(), &["status"], &no).unwrap();

    assert!(out.success(), "{}", out.failure_text());
    assert!(out.stdout.len() <= 64 * 1024, "{}", out.stdout.len());
    assert!(out.stdout.len() > 1000);
}

#[test]
fn un_jj_rotto_da_un_errore_di_avvio_e_non_un_panic() {
    let _serial = common::serial_process_test();
    // Un file vuoto col nome di `jj`: `locate_in` lo trova (nessuna sonda), ma
    // non si può eseguire. L'errore si vede al primo uso, col suo messaggio.
    let rotto = tempfile::tempdir().unwrap();
    std::fs::write(rotto.path().join(nome()), b"").unwrap();
    let cli = JjCli::locate_in(&[rotto.path().to_path_buf()]).unwrap();
    let lavoro = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let errore = cli.run(lavoro.path(), &["status"], &no).unwrap_err();

    assert!(matches!(errore, JjError::Cli(_)), "{errore:?}");
    assert!(errore.to_string().starts_with("impossibile eseguire jj: "));
}
