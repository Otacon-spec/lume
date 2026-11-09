//! Client Debug Adapter Protocol di Lume (design `debugger-dap.md`, piano 17).
//!
//! Nessuna dipendenza UI: protocollo, client su stdio, processo
//! dell'adattatore, sessione, storia degli arresti e configurazione di lancio.

pub mod adapters;
pub mod client;
pub mod history;
pub mod launch;
pub mod process;
pub mod protocol;
pub mod session;

pub use adapters::AdapterId;
pub use client::{AdapterEvent, DapClient, DapError};
pub use history::{FrameSummary, StopHistory, StopRecord};
pub use launch::{LAUNCH_FILE, LaunchConfig, LaunchError};
pub use process::ShutdownOutcome;
pub use protocol::{
    Breakpoint, BreakpointEvent, Capabilities, EvaluateResult, Event, ExitedEvent, OutputEvent,
    Scope, Source, SourceBreakpoint, StackFrame, StoppedEvent, Thread, Variable,
};
pub use session::{DebugSession, FileBreakpoints, SessionError, StartedSession};
