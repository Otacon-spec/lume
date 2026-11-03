//! Integrazione Jujutsu opzionale tramite il comando `jj`.
//!
//! Se il progetto ha una cartella `.jj`, Lume delega al
//! programma `jj` (mai `jj-lib`). Vedi CONTRIBUTING.md, «Eccezione: jj CLI».

use std::path::{Path, PathBuf};

// Unix CLI cleanup owns the direct child and its process group, plus nonblocking readers.
#[allow(unsafe_code)]
mod cli;
pub use cli::{JjCli, JjOutput, JjQuarantineReport, collect_quarantine, quarantine_has_children};
mod log;
pub use log::{JjLogEntry, LOG_TEMPLATE, parse_log};
mod workspace;
pub use workspace::{JjCommand, JjSnapshot, JjWorkspace};

/// Perché un comando `jj` non ha dato un risultato.
#[derive(Debug, thiserror::Error)]
pub enum JjError {
    /// L'utente ha annullato (il flag di annullamento si è alzato).
    #[error("operazione annullata")]
    Cancelled,
    /// `jj` non è partito (file mancante, non eseguibile, rotto).
    #[error("impossibile eseguire jj: {0}")]
    Cli(String),
    /// `jj` è partito ed è uscito con un errore: il testo è il suo.
    #[error("{0}")]
    Failed(String),
    /// `jj` ha risposto, ma non nel formato che il template chiedeva.
    #[error("output di jj non riconosciuto: {0}")]
    Parse(String),
}

/// La cartella più vicina, fra `start` e i suoi antenati, che contiene una
/// cartella `.jj`: la radice del workspace jj. Solo `stat`; nessun processo.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".jj").is_dir())
        .map(Path::to_path_buf)
}
