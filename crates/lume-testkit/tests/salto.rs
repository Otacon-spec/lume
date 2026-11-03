#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Il comportamento di `richiedi!` visto da fuori: un test che chiede `git`
//! con un `PATH` privo di `git` salta (exit 0) o, in modalità rigida, fallisce
//! con un messaggio che nomina lo strumento.

use std::path::Path;
use std::process::{Command, Output};

use lume_testkit::{Strumento, VARIABILE_RIGIDA};

/// Aiutante: chiede `git`. Ignorato di norma; lo lancia `lancia`.
#[test]
#[ignore = "aiutante lanciato da salta_e_dice_cosa_manca"]
fn helper_richiede_git() {
    lume_testkit::richiedi!(Strumento::Git);
    println!("ESEGUITO");
}

/// Aiutante: chiede `jj`. Ignorato di norma; lo lancia il test della modalità
/// facoltativa o rigida.
#[test]
#[ignore = "aiutante lanciato dai test comportamento jj"]
fn helper_richiede_jj() {
    lume_testkit::richiedi!(Strumento::Jujutsu);
    println!("ESEGUITO");
}

/// Aiutante: chiede solo l'eseguibile corrente, che c'è sempre, con la virgola
/// finale che la macro deve accettare.
#[test]
#[ignore = "aiutante lanciato da procede_se_lo_strumento_c_e"]
fn helper_richiede_l_eseguibile_corrente() {
    lume_testkit::richiedi!(Strumento::EseguibileCorrente,);
    println!("ESEGUITO");
}

fn lancia(nome: &str, path: &Path, rigida: bool) -> Output {
    let mut comando = Command::new(std::env::current_exe().unwrap());
    comando
        .args(["--ignored", "--exact", nome, "--nocapture"])
        .env("PATH", path);
    if rigida {
        comando.env(VARIABILE_RIGIDA, "1");
    } else {
        comando.env_remove(VARIABILE_RIGIDA);
    }
    comando.output().unwrap()
}

#[test]
fn salta_e_dice_cosa_manca() {
    let vuota = tempfile::tempdir().unwrap();
    let uscita = lancia("helper_richiede_git", vuota.path(), false);
    let stderr = String::from_utf8_lossy(&uscita.stderr);
    let stdout = String::from_utf8_lossy(&uscita.stdout);
    assert!(
        uscita.status.success(),
        "un test che salta esce con 0: {stderr}"
    );
    assert!(stderr.contains("SALTATO"), "{stderr}");
    assert!(stderr.contains("`git`"), "{stderr}");
    assert!(
        !stdout.contains("ESEGUITO"),
        "il corpo non doveva girare: {stdout}"
    );
}

#[test]
fn in_modalita_rigida_fallisce_nominando_lo_strumento() {
    let vuota = tempfile::tempdir().unwrap();
    let uscita = lancia("helper_richiede_git", vuota.path(), true);
    let stderr = String::from_utf8_lossy(&uscita.stderr);
    assert!(!uscita.status.success(), "in modalità rigida deve fallire");
    assert!(stderr.contains("`git`"), "{stderr}");
    assert!(stderr.contains(VARIABILE_RIGIDA), "{stderr}");
}

#[test]
fn jj_mancante_salva_o_fallisce_in_modalita_rigida() {
    let vuota = tempfile::tempdir().unwrap();
    let facoltativa = lancia("helper_richiede_jj", vuota.path(), false);
    let stderr = String::from_utf8_lossy(&facoltativa.stderr);
    assert!(facoltativa.status.success(), "{stderr}");
    assert!(stderr.contains("SALTATO"), "{stderr}");
    assert!(stderr.contains("`jj`"), "{stderr}");

    let rigida = lancia("helper_richiede_jj", vuota.path(), true);
    let stderr = String::from_utf8_lossy(&rigida.stderr);
    assert!(!rigida.status.success(), "in modalità rigida deve fallire");
    assert!(stderr.contains("`jj`"), "{stderr}");
    assert!(stderr.contains(VARIABILE_RIGIDA), "{stderr}");
}

#[test]
fn procede_se_lo_strumento_c_e() {
    let vuota = tempfile::tempdir().unwrap();
    let uscita = lancia("helper_richiede_l_eseguibile_corrente", vuota.path(), true);
    let stdout = String::from_utf8_lossy(&uscita.stdout);
    assert!(
        uscita.status.success(),
        "{}",
        String::from_utf8_lossy(&uscita.stderr)
    );
    assert!(stdout.contains("ESEGUITO"), "{stdout}");
}
