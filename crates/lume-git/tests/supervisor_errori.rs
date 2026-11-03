#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Errori pre-spawn sul supervisor reale, senza sostituire il backend.
use std::process::{Command, Output};

fn supervisor() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lume-git-supervisor"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("LUME_INTERNAL_GIT_") {
            command.env_remove(key);
        }
    }
    command
}

fn errore(output: Output, atteso: &str) {
    assert_eq!(output.status.code(), Some(127));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Lume Git supervisor:"), "{stderr}");
    assert!(stderr.contains(atteso), "{stderr}");
}

#[test]
fn configurazione_incompleta_non_esegue_il_programma() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("eseguito");
    let program = root.path().join("programma");
    std::fs::write(&program, "#!/bin/sh\ntouch eseguito\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Controllo positivo: lo stesso eseguibile crea davvero il marker.
    let valid = supervisor()
        .env("LUME_INTERNAL_GIT_MODE", "1")
        .env("LUME_INTERNAL_GIT_PROGRAM", &program)
        .env("LUME_INTERNAL_GIT_ARG_COUNT", "0")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(valid.status.success());
    assert!(marker.is_file());
    std::fs::remove_file(&marker).unwrap();
    let output = supervisor()
        .env("LUME_INTERNAL_GIT_MODE", "1")
        .env("LUME_INTERNAL_GIT_PROGRAM", &program)
        .env("LUME_INTERNAL_GIT_ARG_COUNT", "2")
        .env("LUME_INTERNAL_GIT_ARG_0", "-c")
        // Il secondo argomento obbligatorio manca: il programma non deve partire.
        .current_dir(root.path())
        .output()
        .unwrap();
    errore(output, "missing supervisor data");
    assert!(!marker.exists());
}

#[test]
fn conteggio_invalido_non_esegue_il_programma() {
    for count in ["-1", "non-numero", "16385"] {
        let output = supervisor()
            .env("LUME_INTERNAL_GIT_MODE", "1")
            .env("LUME_INTERNAL_GIT_PROGRAM", "/bin/sh")
            .env("LUME_INTERNAL_GIT_ARG_COUNT", count)
            .output()
            .unwrap();
        errore(output, "invalid Git argument count");
    }
}

#[test]
fn programma_inesistente_restituisce_errore_esplicito() {
    let root = tempfile::tempdir().unwrap();
    let output = supervisor()
        .env("LUME_INTERNAL_GIT_MODE", "1")
        .env("LUME_INTERNAL_GIT_PROGRAM", root.path().join("inesistente"))
        .env("LUME_INTERNAL_GIT_ARG_COUNT", "0")
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&output.stderr).contains("supervisor data"));
    errore(output, "No such file or directory");
}
