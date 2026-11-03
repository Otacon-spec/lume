//! Il ponte fra lo stream del server e l'app.
//!
//! `McpEvent` va dallo stream all'app dentro `Message::Mcp(epoca, evento)`:
//! `Message` deriva `Clone` (`crates/lume-app/src/app.rs:531`), quindi qui non
//! viaggia nessuna `oneshot`. Le domande hanno un `id: u64`; lo stream tiene
//! la `HashMap<u64, oneshot::Sender>` e l'app risponde con
//! `McpCommand::Answer` sulla mailbox consegnata con `Ready`.

use std::path::PathBuf;

use tokio::sync::mpsc::UnboundedSender;

use crate::catalog::Availability;
use crate::tools::fs::TextSnapshot;

/// Quel che solo l'app sa. I percorsi sono già confinati e **relativi** alla
/// radice: l'app li unisce alla sua `project_root`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppQuery {
    OpenFiles,
    ActiveFile,
    Buffer {
        relative: PathBuf,
    },
    Diagnostics {
        relative: Option<PathBuf>,
        max_severity: u8,
    },
    Hover {
        relative: PathBuf,
        line: u32,
        column: u32,
    },
    Definition {
        relative: PathBuf,
        line: u32,
        column: u32,
    },
    References {
        relative: PathBuf,
        line: u32,
        column: u32,
    },
    DocumentSymbols {
        relative: PathBuf,
    },
    OpenInEditor {
        relative: PathBuf,
        line: Option<usize>,
    },
    WriteFile {
        relative: PathBuf,
        content: String,
    },
    ReplaceText {
        relative: PathBuf,
        old_text: String,
        new_text: String,
        all: bool,
    },
    RunCommand {
        program: String,
        args: Vec<String>,
        cwd: PathBuf,
        timeout_secs: u64,
    },
    Rename {
        relative: PathBuf,
        line: u32,
        column: u32,
        new_name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppAnswer {
    Json(serde_json::Value),
    /// `None`: il file non è aperto, lo stream lo legge dal disco.
    Buffer(Option<TextSnapshot>),
    Granted,
}

#[derive(Debug)]
pub enum McpCommand {
    Answer {
        id: u64,
        answer: Result<AppAnswer, String>,
    },
    ToolsChanged(Availability),
}

#[derive(Debug, Clone)]
pub enum McpEvent {
    /// Il primo evento: il trasporto esiste, il record di scoperta è
    /// pubblicato.
    Ready(UnboundedSender<McpCommand>),
    /// Dopo `initialize`, col nome già ripulito.
    ClientConnected {
        conn: u64,
        name: String,
    },
    ClientGone(u64),
    Ask {
        id: u64,
        conn: u64,
        query: AppQuery,
    },
    AskGone(u64),
    /// Il server non parte (endpoint occupato, cartella dati non sicura o
    /// non scrivibile): lo stream non manda altro.
    Failed(String),
}
