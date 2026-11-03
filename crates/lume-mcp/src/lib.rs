//! Server MCP di Lume: protocollo, trasporto locale e catalogo dei tool.
//!
//! Il trasporto è una named pipe su Windows e un socket filesystem Unix su
//! Linux/macOS. Il crate entra nel binario solo con la feature `mcp` di
//! lume-app (`optional = true`): senza, non c'è una riga di questo codice in
//! `lume.exe`. È il motivo per cui qui non si dipende né da iced né da
//! lume-store.

pub mod catalog;
pub mod confine;
pub mod discovery;
pub mod edit;
pub mod events;
pub mod exec;
pub mod framing;
pub mod gate;
pub mod id;
pub mod protocol;
pub mod proxy;
pub mod server;
pub mod tools;
#[cfg(unix)]
pub mod unix;

pub use catalog::{Args, Availability, Route, ToolError, ToolName};
pub use confine::Root;
pub use discovery::Token;
pub use events::{AppAnswer, AppQuery, McpCommand, McpEvent};
pub use gate::{Effect, Mode, Refusal, Target, Verdict, decide, is_git_name};
pub use id::McpId;
pub use tools::fs::TextSnapshot;
