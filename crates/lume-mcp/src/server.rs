//! Il server MCP locale: named pipe su Windows, socket Unix su Linux/macOS.
//!
//! `serve` è un future e basta: nell'app lo possiede lo stream di
//! `Subscription::run_with(McpSpawn)`, e con lui il `JoinSet` delle
//! connessioni, il registro delle domande e la guardia della scoperta.
//! Nessun `tokio::spawn`: quando iced droppa lo stream non resta niente.
//!
//! A riposo (nessun client) il costo è un accept pendente: zero thread,
//! zero timer. Il dispatcher MCP e il protocollo sono comuni; gli adattatori
//! di piattaforma possiedono solo listener, permessi e scoperta.

#[cfg(windows)]
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::catalog::Availability;
use crate::confine::Root;
use crate::discovery::Token;
use crate::events::McpEvent;

pub const MAX_INSTANCES: usize = 4;
/// Connessioni accettate che non hanno ancora superato il preambolo (prove
/// della scoperta e client che stanno mandando la prima riga): non contano
/// contro `MAX_INSTANCES`, ma hanno un tetto di risorse e la deadline del
/// preambolo. Una prova di scoperta non può quindi far rifiutare una
/// connessione reale.
pub const MAX_PENDING_HANDSHAKES: usize = MAX_INSTANCES * 4;
pub const PREAMBLE_TIMEOUT: Duration = Duration::from_secs(5);
pub const PREAMBLE_CAP: usize = 1024;
pub const MAX_INFLIGHT: usize = 16;
pub const ASK_TIMEOUT: Duration = Duration::from_secs(300);

/// Endpoint locale della piattaforma. I percorsi non vengono mai convertiti
/// con perdita: una pipe è un nome, un socket è un percorso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    #[cfg(windows)]
    Pipe {
        name: String,
        /// `None` nei test: nessun file di scoperta.
        discovery_dir: Option<PathBuf>,
    },
    #[cfg(unix)]
    Unix(crate::unix::Endpoint),
}

impl Endpoint {
    pub fn describe(&self) -> String {
        match self {
            #[cfg(windows)]
            Self::Pipe { name, .. } => format!("named pipe {name}"),
            #[cfg(unix)]
            Self::Unix(endpoint) => format!("socket Unix {}", endpoint.socket_path().display()),
        }
    }

    /// Identità pubblica scelta con `--instance`. Su Windows il ponte non
    /// accetta il flag e l'identità resta implicita nel nome della pipe.
    pub fn instance(&self) -> Option<String> {
        match self {
            #[cfg(windows)]
            Self::Pipe { .. } => None,
            #[cfg(unix)]
            Self::Unix(endpoint) => Some(endpoint.identity().to_string()),
        }
    }
}

pub struct ServeConfig {
    pub endpoint: Endpoint,
    pub token: Token,
    pub root: Root,
    pub server_version: String,
    pub availability: Availability,
    pub preamble_timeout: Duration,
    pub line_cap: usize,
    pub ask_timeout: Duration,
}

pub async fn serve(config: ServeConfig, events: mpsc::UnboundedSender<McpEvent>) {
    imp::serve(config, events).await;
}

mod common;

#[cfg(windows)]
mod windows;

#[cfg(unix)]
mod unix;

#[cfg(windows)]
use windows as imp;

#[cfg(unix)]
use unix as imp;
