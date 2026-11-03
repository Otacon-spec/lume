#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]
//! Fixture dei test: una cartella temporanea con il falso `jj` (`fake-jj`)
//! copiato come `jj`, e i file che ne scrivono le risposte.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use lume_jj::JjCli;
use serde::Deserialize;

static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Evita che il runner lanci più copie temporanee di `jj` simultaneamente:
/// alcuni runner Linux restituiscono `ETXTBSY` con questi spawn concorrenti.
pub fn serial_process_test() -> MutexGuard<'static, ()> {
    PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Una chiamata registrata dal falso `jj`.
#[derive(Debug, Deserialize)]
pub struct Call {
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, Option<String>>,
}

pub struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "jj.exe" } else { "jj" };
        let staged = dir.path().join(format!("{name}.copy"));
        std::fs::copy(env!("CARGO_BIN_EXE_fake-jj"), &staged).unwrap();
        // Pubblica il file solo quando la copia è completa: i test girano in
        // parallelo e alcuni filesystem rifiutano l'exec di un file appena
        // creato mentre la copia è ancora visibile con il nome definitivo.
        std::fs::rename(staged, dir.path().join(name)).unwrap();
        Self { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Il `JjCli` che trova il falso `jj` (e nessun altro).
    pub fn cli(&self) -> JjCli {
        JjCli::locate_in(&[self.path().to_path_buf()]).unwrap()
    }

    /// Risposta di `<chiave>`: stdout e codice di uscita.
    pub fn script(&self, key: &str, stdout: &str, code: i32) {
        std::fs::write(self.path().join(format!("out-{key}.txt")), stdout).unwrap();
        std::fs::write(
            self.path().join(format!("code-{key}.txt")),
            code.to_string(),
        )
        .unwrap();
    }

    /// Lo stderr di `<chiave>`.
    pub fn script_err(&self, key: &str, stderr: &str) {
        std::fs::write(self.path().join(format!("err-{key}.txt")), stderr).unwrap();
    }

    /// `<chiave>` dorme 30 s prima di rispondere (per provare l'annullamento).
    pub fn slow(&self, key: &str) {
        std::fs::write(self.path().join(format!("sleep-{key}.txt")), "").unwrap();
    }

    /// Le chiamate ricevute finora, in ordine.
    pub fn calls(&self) -> Vec<Call> {
        match std::fs::read_to_string(self.path().join("calls.jsonl")) {
            Ok(text) => text
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// Due percorsi sono la stessa cartella (su Windows la cartella temporanea può
/// avere un nome corto `WONDER~1` da una parte e lungo dall'altra).
pub fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
