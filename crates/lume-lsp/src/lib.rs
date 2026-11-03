//! Client Language Server Protocol generico, indipendente dal server.

pub mod actor;
mod diagnostics;
mod documents;
mod features;
mod framing;
// Job Object di Windows e gruppi di processi Unix: FFI, quindi `unsafe`.
#[allow(unsafe_code)]
mod process;
#[doc(hidden)]
pub mod process_watchdog;
mod progress;
mod rpc;
mod server_requests;
mod session;
#[cfg(feature = "testing")]
pub mod testing;
mod transport;
mod trust;

pub use actor::{
    ActorEvent, COMMAND_CAPACITY, DEFERRED_COMMAND_CAPACITY, LspAnswer, LspCommand,
    LspCommandMailbox, LspCommandReceiver, LspRequest, MAILBOX_CAPACITY, MAILBOX_TEXT_BUDGET_BYTES,
    MailboxError, SessionReady,
};
pub use diagnostics::{DiagnosticStore, Problem, ProblemId, PublishResult, ServerId};
pub use documents::{DocumentChange, DocumentError, DocumentStore};
pub use features::{FeatureAvailability, FeatureClient, FeatureError};
pub use framing::{FrameDecoder, MAX_BODY_BYTES, MAX_HEADER_BYTES, ProtocolError, encode_message};
pub use process::{
    CommandSource, STDERR_CAPACITY, ServerCommand, ServerProcess, drain_process_reaper,
};
pub use progress::ProgressThrottle;
pub use rpc::{
    JsonRpcVersion, RequestId, RpcError, RpcMessage, RpcNotification, RpcRequest, RpcResponse,
    RpcResponsePayload,
};
pub use server_requests::{METHOD_NOT_FOUND, ServerReply, reply_to_server_request};
pub use session::{
    InitializeSpec, LspSession, NegotiatedCapabilities, SessionError, SessionEvent, SessionState,
};
pub use transport::{
    ClientError, ClientTransport, EVENT_CAPACITY, OUTBOUND_BYTES_CAPACITY, OUTBOUND_CAPACITY,
    QueueKind, RequestHandle, TransportConfig, TransportEvent, TransportEvents,
};
pub use trust::{ConfigurationFingerprint, TrustRequired, WorkspaceTrust};
