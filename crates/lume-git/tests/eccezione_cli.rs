#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! L'eccezione «git CLI» è centralizzata nel client e nel suo supervisor
//! Linux. Questo test impedisce di avviare Git da altri moduli per distrazione.

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
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn command_new_compare_solo_in_cli_rs() {
    let mut files = Vec::new();
    rust_files(&root().join("crates/lume-git/src"), &mut files);
    assert!(files.len() >= 4, "attesi i moduli di lume-git: {files:?}");
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let is_cli = file.file_name().is_some_and(|n| n == "cli.rs");
        let is_supervisor = file
            .file_name()
            .is_some_and(|name| name == "lume-git-supervisor.rs");
        assert_eq!(
            text.contains("Command::new("),
            is_cli || is_supervisor,
            "{}: `Command::new(` ammesso solo nel client Git o nel supervisor",
            file.display()
        );
    }
}

#[test]
fn nessun_crate_lancia_git_a_mano_nel_codice() {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root().join("crates")).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("Command::new(\"git\")"),
            "{}: la CLI git passa da lume_git::GitCli",
            file.display()
        );
    }
}

#[test]
fn i_documenti_dichiarano_l_eccezione() {
    for name in ["CONTRIBUTING.md", "README.md"] {
        let text = std::fs::read_to_string(root().join(name)).unwrap();
        assert!(
            text.contains("Eccezione: git CLI"),
            "{name} deve avere la sezione «Eccezione: git CLI»"
        );
    }
    let contributing = std::fs::read_to_string(root().join("CONTRIBUTING.md")).unwrap();
    assert!(
        !contributing.contains("La CLI si usa solo nei test"),
        "la vecchia regola assoluta va sostituita dall'eccezione"
    );
}
