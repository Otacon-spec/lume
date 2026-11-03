#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Verifica che il binario jj abbia una sola porta d'ingresso e che i vincoli
//! di costo e dipendenze restino dichiarati e controllati.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

fn all_source_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root().join("crates")).unwrap() {
        let source = entry.unwrap().path().join("src");
        if source.is_dir() {
            rust_files(&source, &mut files);
        }
    }
    files
}

#[test]
fn command_new_compare_solo_in_cli_rs() {
    let mut files = Vec::new();
    rust_files(&root().join("crates/lume-jj/src"), &mut files);
    assert!(files.len() >= 4, "attesi i moduli di lume-jj: {files:?}");
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let is_cli = file.file_name().is_some_and(|name| name == "cli.rs");
        assert_eq!(
            text.contains("Command::new("),
            is_cli,
            "{}: Command::new ammesso solo in cli.rs",
            file.display()
        );
    }
}

#[test]
fn nessun_crate_lancia_jj_a_mano_nel_codice() {
    for file in all_source_files() {
        let text = std::fs::read_to_string(&file).unwrap();
        for literal in ["Command::new(\"jj\")", "Command::new(\"jj.exe\")"] {
            assert!(
                !text.contains(literal),
                "{}: jj passa da JjCli",
                file.display()
            );
        }
    }
}

#[test]
fn niente_timer_ne_sonde_nel_codice_jujutsu() {
    let mut files = Vec::new();
    rust_files(&root().join("crates/lume-jj/src"), &mut files);
    files.push(root().join("crates/lume-app/src/jj_actions.rs"));
    files.push(root().join("crates/lume-app/src/tools/jj.rs"));
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("time::every"),
            "{}: niente timer per jj",
            file.display()
        );
        assert!(
            !text.contains("\"--version\""),
            "{}: niente sonda jj --version",
            file.display()
        );
    }
}

#[test]
fn nessun_crate_dipende_da_jj_lib() {
    let mut manifests = vec![root().join("Cargo.toml")];
    for entry in std::fs::read_dir(root().join("crates")).unwrap() {
        let manifest = entry.unwrap().path().join("Cargo.toml");
        if manifest.is_file() {
            manifests.push(manifest);
        }
    }
    assert!(manifests.len() > 5);
    for manifest in manifests {
        let text = std::fs::read_to_string(&manifest).unwrap();
        for name in ["jj-lib", "jj-cli"] {
            assert!(
                !text.contains(name),
                "{}: vietato {name}",
                manifest.display()
            );
        }
    }
}

#[test]
fn i_documenti_dichiarano_l_eccezione() {
    for name in ["CONTRIBUTING.md", "README.md"] {
        let text = std::fs::read_to_string(root().join(name)).unwrap();
        assert!(
            text.contains("Eccezione: jj CLI"),
            "{name} deve dichiarare l'eccezione"
        );
    }
}
