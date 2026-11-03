//! Appunti nativi di Lume: file Markdown con intestazione TOML e collegamenti al codice.
//!
//! Ogni appunto è un file `.md` nella libreria, letto e scritto da
//! [`NoteStore`]. Se il file cambia su disco mentre è aperto, il salvataggio
//! fallisce con [`NoteError::Conflict`] invece di sovrascrivere. Il crate non
//! conosce Iced: l'interfaccia vive in `lume-app`.

mod format;
mod links;
mod model;
// `SetFileInformationByHandle`: FFI Windows.
#[allow(unsafe_code)]
mod store;

pub use format::{ParsedNote, parse, serialize, title_from_markdown};
pub use links::{LinkIndex, LinkedNote, clamp_lines, relative_path, resolve_link};
pub use model::{CodeLink, Note, NoteError, NoteHeader, NoteId, NoteSummary};
pub use store::{
    DeleteDisposition, FileIdentity, LoadedNote, MAX_NOTE_BYTES, NoteStore, RetitledNote,
    SCAN_PREFIX_BYTES, SaveIsolation, filter_summaries,
};
