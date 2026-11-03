//! Errori del nucleo: I/O sui documenti e pattern di ricerca.

use std::path::PathBuf;

/// Errore prodotto dalle operazioni di lume-core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("I/O su {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("pattern di ricerca non valido")]
    InvalidPattern,
}
