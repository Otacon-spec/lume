#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! #12: `lume.exe --prova-crash` lascia un log dei crash nella cartella dati.
//! Lancia il binario vero, con `LUME_DATA_DIR` in una cartella temporanea.

use std::process::Command;

fn cartella_dati(nome: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lume-{nome}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn crash_di_prova(dati: &std::path::Path) -> String {
    let uscita = Command::new(env!("CARGO_BIN_EXE_lume"))
        .arg("--prova-crash")
        .env("LUME_DATA_DIR", dati)
        .output()
        .unwrap();
    assert_eq!(
        uscita.status.code(),
        Some(101),
        "un panic nel thread principale esce con 101; stderr: {}",
        String::from_utf8_lossy(&uscita.stderr)
    );
    let log: Vec<_> = std::fs::read_dir(dati.join("logs"))
        .unwrap()
        .map(|voce| voce.unwrap().path())
        .collect();
    assert_eq!(log.len(), 1, "{log:?}");
    std::fs::read_to_string(&log[0]).unwrap()
}

#[test]
fn il_crash_di_prova_scrive_il_log_nella_cartella_dati() {
    let dati = cartella_dati("prova-crash");
    let testo = crash_di_prova(&dati);
    assert!(
        testo.contains("Messaggio: crash di prova richiesto con --prova-crash"),
        "{testo}"
    );
    assert!(testo.contains("main.rs"), "il punto del panic: {testo}");
    let _ = std::fs::remove_dir_all(&dati);
}

/// In debug il backtrace ha i simboli, e con loro i percorsi di std e del
/// registry di cargo: la cartella dell'utente non deve comparire.
#[test]
fn il_log_non_contiene_la_cartella_utente() {
    let Some(casa) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        return;
    };
    let casa = casa.to_string_lossy().to_ascii_lowercase();
    let dati = cartella_dati("prova-crash-casa");
    let testo = crash_di_prova(&dati).to_ascii_lowercase();
    assert!(!testo.contains(&casa), "{testo}");
    assert!(!testo.contains(&casa.replace('\\', "/")), "{testo}");
    let _ = std::fs::remove_dir_all(&dati);
}
