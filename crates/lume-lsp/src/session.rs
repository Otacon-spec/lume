use std::{process::ExitStatus, time::Duration};

use lsp_types::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, InitializedParams,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    Uri, WorkspaceFolder,
};
use serde_json::{Value, json};

use crate::{
    ClientError, DocumentChange, DocumentError, DocumentStore, RpcNotification, ServerCommand,
    ServerProcess, TransportConfig, TransportEvent, WorkspaceTrust,
};

const DEFAULT_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Starting,
    Initializing,
    Ready,
    ShuttingDown,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct InitializeSpec {
    pub root_uri: Option<Uri>,
    pub workspace_folders: Option<Vec<WorkspaceFolder>>,
    pub initialization_options: Option<Value>,
    pub locale: Option<String>,
    pub timeout: Duration,
}

impl Default for InitializeSpec {
    fn default() -> Self {
        Self {
            root_uri: None,
            workspace_folders: None,
            initialization_options: None,
            locale: None,
            timeout: DEFAULT_INITIALIZE_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NegotiatedCapabilities {
    pub server: ServerCapabilities,
    pub sync_kind: TextDocumentSyncKind,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SessionError {
    #[error("session is not ready ({state:?})")]
    NotReady { state: SessionState },
    #[error("invalid session transition {from:?} -> {to:?}")]
    InvalidTransition {
        from: SessionState,
        to: SessionState,
    },
    #[error("session process is unavailable")]
    ProcessUnavailable,
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Document(#[from] DocumentError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Notification(RpcNotification),
    Diagnostics(PublishDiagnosticsParams),
    Progress(lsp_types::ProgressParams),
    Log(String),
    Exited(SessionError),
    /// Il processo è caduto. Chi riceve decide se e quando chiamare
    /// [`LspSession::restart`] — vedi il commento su `next_event`.
    Disconnected(SessionError),
}

pub struct LspSession {
    state: SessionState,
    command: ServerCommand,
    transport_config: TransportConfig,
    process: Option<ServerProcess>,
    initialize_spec: Option<InitializeSpec>,
    negotiated: Option<NegotiatedCapabilities>,
    documents: DocumentStore,
    restart_attempted: bool,
    workspace_trust: WorkspaceTrust,
}

impl LspSession {
    pub fn spawn(
        command: ServerCommand,
        transport_config: TransportConfig,
    ) -> Result<Self, SessionError> {
        Self::spawn_with_trust(command, transport_config, WorkspaceTrust::Unknown)
    }

    pub fn spawn_with_trust(
        command: ServerCommand,
        transport_config: TransportConfig,
        workspace_trust: WorkspaceTrust,
    ) -> Result<Self, SessionError> {
        let process =
            ServerProcess::spawn_with_trust(command.clone(), transport_config, workspace_trust)?;
        Ok(Self {
            state: SessionState::Starting,
            command,
            transport_config,
            process: Some(process),
            initialize_spec: None,
            negotiated: None,
            documents: DocumentStore::new(TextDocumentSyncKind::NONE),
            restart_attempted: false,
            workspace_trust,
        })
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn negotiated_capabilities(&self) -> Option<&NegotiatedCapabilities> {
        self.negotiated.as_ref()
    }

    pub fn peak_memory_bytes(&self) -> Option<u64> {
        self.process.as_ref()?.peak_memory_bytes()
    }

    pub async fn initialize(
        &mut self,
        spec: InitializeSpec,
    ) -> Result<NegotiatedCapabilities, SessionError> {
        self.transition(SessionState::Initializing)?;
        match self.negotiate(&spec).await {
            Ok(negotiated) => {
                self.documents.set_sync_kind(negotiated.sync_kind);
                self.initialize_spec = Some(spec);
                self.negotiated = Some(negotiated.clone());
                self.transition(SessionState::Ready)?;
                Ok(negotiated)
            }
            Err(error) => {
                self.state = SessionState::Stopped;
                Err(error)
            }
        }
    }

    pub fn open_document(
        &mut self,
        uri: Uri,
        language_id: String,
        text: String,
    ) -> Result<(), SessionError> {
        self.require_ready()?;
        let (params, prepared) = self.documents.prepare_open(uri, language_id, text)?;
        if let Err(error) = self.transport()?.notify("textDocument/didOpen", params) {
            return Err(error.into());
        }
        self.documents.commit_change(prepared);
        Ok(())
    }

    pub async fn open_document_wait(
        &mut self,
        uri: Uri,
        language_id: String,
        text: String,
    ) -> Result<(), SessionError> {
        self.require_ready()?;
        let (params, prepared) = self.documents.prepare_open(uri, language_id, text)?;
        if let Err(error) = self
            .transport()?
            .notify_wait("textDocument/didOpen", params)
            .await
        {
            return Err(error.into());
        }
        self.documents.commit_change(prepared);
        Ok(())
    }

    pub fn change_document(
        &mut self,
        uri: &Uri,
        change: DocumentChange,
    ) -> Result<(), SessionError> {
        self.require_ready()?;
        let (params, prepared) = self.documents.prepare_change(uri, change)?;
        if let Err(error) = self.transport()?.notify("textDocument/didChange", params) {
            return Err(error.into());
        }
        self.documents.commit_change(prepared);
        Ok(())
    }

    pub fn change_document_text(&mut self, uri: &Uri, text: &str) -> Result<(), SessionError> {
        self.require_ready()?;
        let (params, prepared) = self.documents.prepare_change_text(uri, text)?;
        if let Err(error) = self.transport()?.notify("textDocument/didChange", params) {
            return Err(error.into());
        }
        self.documents.commit_change(prepared);
        Ok(())
    }

    pub async fn change_document_text_wait(
        &mut self,
        uri: &Uri,
        text: &str,
    ) -> Result<(), SessionError> {
        self.require_ready()?;
        self.change_document_text_owned_wait(uri, text.to_owned())
            .await
    }

    pub(crate) async fn change_document_text_owned_wait(
        &mut self,
        uri: &Uri,
        text: String,
    ) -> Result<(), SessionError> {
        self.require_ready()?;
        let (params, prepared) = self.documents.prepare_change_text_owned(uri, text)?;
        if let Err(error) = self
            .transport()?
            .notify_wait("textDocument/didChange", params)
            .await
        {
            return Err(error.into());
        }
        self.documents.commit_change(prepared);
        Ok(())
    }

    pub fn save_document(&self, uri: &Uri, include_text: bool) -> Result<(), SessionError> {
        self.require_ready()?;
        let params = self.documents.save(uri, include_text)?;
        self.transport()?
            .notify("textDocument/didSave", params)
            .map_err(Into::into)
    }

    pub fn close_document(&mut self, uri: &Uri) -> Result<(), SessionError> {
        self.require_ready()?;
        let params = self.documents.prepare_close(uri)?;
        if let Err(error) = self.transport()?.notify("textDocument/didClose", params) {
            return Err(error.into());
        }
        self.documents.commit_close(uri);
        Ok(())
    }

    /// Il prossimo evento dal server.
    ///
    /// ATTENZIONE: **È cancel-safe, e deve restarlo.** Chi la consuma la mette in un
    /// `tokio::select!` accanto a una mailbox di comandi, e `select!` droppa
    /// il future perdente. Prima questa funzione, ricevuto un
    /// `Disconnected`, chiamava `restart().await` **al suo interno**:
    /// processo nuovo più `negotiate`, fino a dieci secondi. Bastava un
    /// `didChange` mentre il riavvio era in corso perché `select!` scegliesse
    /// il comando e abbandonasse il riavvio dentro `negotiate` — stato fermo
    /// a `Initializing`, notifica `initialized` mai partita, e da lì in poi
    /// **ogni** `require_ready` fallisce. Il server è vivo e l'IDE lo crede
    /// morto, senza modo di accorgersene se non usandolo.
    ///
    /// Ora restituisce [`SessionEvent::Disconnected`] e basta. Il riavvio lo
    /// fa il chiamante come **passo proprio e non interrompibile**: i comandi
    /// nel frattempo si accodano nella mailbox, che è tutto quel che serve.
    pub async fn next_event(&mut self) -> Option<SessionEvent> {
        loop {
            let event = self.process.as_mut()?.events().recv().await?;
            match event {
                TransportEvent::Notification(notification)
                    if notification.method == "textDocument/publishDiagnostics" =>
                {
                    return match serde_json::from_value(notification.params.clone()) {
                        Ok(diagnostics) => Some(SessionEvent::Diagnostics(diagnostics)),
                        Err(error) => Some(SessionEvent::Log(format!(
                            "invalid publishDiagnostics notification: {error}"
                        ))),
                    };
                }
                TransportEvent::Notification(notification)
                    if notification.method == "$/progress" =>
                {
                    return match serde_json::from_value(notification.params) {
                        Ok(progress) => Some(SessionEvent::Progress(progress)),
                        Err(error) => Some(SessionEvent::Log(format!(
                            "invalid $/progress notification: {error}"
                        ))),
                    };
                }
                TransportEvent::Notification(notification) => {
                    return Some(SessionEvent::Notification(notification));
                }
                TransportEvent::Request(request) => {
                    if let Some(line) = self.answer_server_request(&request) {
                        return Some(SessionEvent::Log(line));
                    }
                }
                TransportEvent::Disconnected(error) => {
                    return Some(SessionEvent::Disconnected(error.into()));
                }
            }
        }
    }

    fn answer_server_request(&self, request: &crate::RpcRequest) -> Option<String> {
        let transport = self.transport().ok()?;
        match crate::reply_to_server_request(request) {
            crate::ServerReply::Result(value) => transport
                .respond(request.id, value)
                .err()
                .map(|error| format!("risposta a {} non inviata: {error}", request.method)),
            crate::ServerReply::Error { code, message } => {
                let _ = transport.respond_error(request.id, code, message);
                Some(format!(
                    "richiesta del server non gestita: {}",
                    request.method
                ))
            }
        }
    }

    pub async fn shutdown(&mut self, timeout: Duration) -> Result<ExitStatus, SessionError> {
        self.require_ready()?;
        self.transition(SessionState::ShuttingDown)?;
        let process = self
            .process
            .take()
            .ok_or(SessionError::ProcessUnavailable)?;
        let result = process.shutdown(timeout).await.map_err(SessionError::from);
        self.state = SessionState::Stopped;
        result
    }

    /// Riavvia il server e rigioca i `didOpen`.
    ///
    /// Pubblica perché il riavvio non è più automatico: lo decide chi consuma
    /// gli eventi, e lo fa **fuori** da qualunque `select!`. Politica
    /// invariata: **un tentativo solo**, poi la sessione resta `Stopped`.
    pub async fn restart(&mut self) -> Result<(), SessionError> {
        if self.restart_attempted {
            self.state = SessionState::Stopped;
            return Err(ClientError::Disconnected.into());
        }
        self.restart_attempted = true;
        self.process.take();
        self.state = SessionState::Starting;
        let spec = self
            .initialize_spec
            .clone()
            .ok_or(SessionError::ProcessUnavailable)?;
        self.process = Some(ServerProcess::spawn_with_trust(
            self.command.clone(),
            self.transport_config,
            self.workspace_trust,
        )?);
        self.transition(SessionState::Initializing)?;
        let negotiated = self.negotiate(&spec).await?;
        self.documents.set_sync_kind(negotiated.sync_kind);
        self.negotiated = Some(negotiated);
        for params in self.documents.reopen_documents() {
            self.transport()?.notify("textDocument/didOpen", params)?;
        }
        self.transition(SessionState::Ready)
    }

    async fn negotiate(
        &mut self,
        spec: &InitializeSpec,
    ) -> Result<NegotiatedCapabilities, SessionError> {
        let capabilities = client_capabilities()?;
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: spec.root_uri.clone(),
            initialization_options: spec.initialization_options.clone(),
            capabilities,
            workspace_folders: spec.workspace_folders.clone(),
            client_info: Some(ClientInfo {
                name: "Lume".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            locale: spec.locale.clone(),
            ..InitializeParams::default()
        };
        let result: InitializeResult = self
            .transport()?
            .request("initialize", params, spec.timeout)
            .await?;
        self.transport()?
            .notify("initialized", InitializedParams {})?;
        let sync_kind = sync_kind(&result.capabilities);
        Ok(NegotiatedCapabilities {
            server: result.capabilities,
            sync_kind,
        })
    }

    fn transport(&self) -> Result<&crate::ClientTransport, SessionError> {
        self.process
            .as_ref()
            .map(ServerProcess::transport)
            .ok_or(SessionError::ProcessUnavailable)
    }

    pub(crate) fn feature_transport(&self) -> Result<&crate::ClientTransport, SessionError> {
        self.require_ready()?;
        self.transport()
    }

    fn require_ready(&self) -> Result<(), SessionError> {
        if self.state == SessionState::Ready {
            Ok(())
        } else {
            Err(SessionError::NotReady { state: self.state })
        }
    }

    fn transition(&mut self, next: SessionState) -> Result<(), SessionError> {
        let legal = matches!(
            (self.state, next),
            (SessionState::Starting, SessionState::Initializing)
                | (SessionState::Initializing, SessionState::Ready)
                | (SessionState::Ready, SessionState::ShuttingDown)
                | (SessionState::ShuttingDown, SessionState::Stopped)
        );
        if !legal {
            return Err(SessionError::InvalidTransition {
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        Ok(())
    }
}

fn sync_kind(capabilities: &ServerCapabilities) -> TextDocumentSyncKind {
    match capabilities.text_document_sync.as_ref() {
        Some(TextDocumentSyncCapability::Kind(kind)) => *kind,
        Some(TextDocumentSyncCapability::Options(options)) => {
            options.change.unwrap_or(TextDocumentSyncKind::NONE)
        }
        None => TextDocumentSyncKind::NONE,
    }
}

fn client_capabilities() -> Result<ClientCapabilities, SessionError> {
    serde_json::from_value(json!({
        "general": { "positionEncodings": ["utf-16"] },
        "textDocument": {
            "completion": {
                "completionItem": {
                    "snippetSupport": false,
                    "insertReplaceSupport": false,
                    "resolveSupport": {
                        "properties": ["documentation", "detail"]
                    }
                }
            },
            "hover": { "contentFormat": ["markdown", "plaintext"] },
            "signatureHelp": { "contextSupport": true },
            "declaration": { "linkSupport": true },
            "definition": { "linkSupport": true },
            "implementation": { "linkSupport": true },
            "references": {},
            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
            "rename": { "prepareSupport": true },
            "formatting": {},
            "rangeFormatting": {},
            "codeAction": {},
            "semanticTokens": {
                "requests": { "range": false, "full": { "delta": true } },
                "tokenTypes": [],
                "tokenModifiers": [],
                "formats": ["relative"],
                "overlappingTokenSupport": false,
                "multilineTokenSupport": false
            },
            "inlayHint": {}
        },
        "workspace": {
            "symbol": {},
            "executeCommand": {}
        },
        "window": { "workDoneProgress": true }
    }))
    .map_err(|error| ClientError::Deserialize(error.to_string()).into())
}

#[cfg(test)]
#[path = "session/native_requests77.rs"]
mod native_requests77;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::client_capabilities;

    #[test]
    fn il_client_non_dichiara_snippet_ne_modifiche_aggiuntive() {
        let json = serde_json::to_value(client_capabilities().unwrap()).unwrap();
        let item = json
            .pointer("/textDocument/completion/completionItem")
            .expect("completionItem dichiarato");
        assert_eq!(item["snippetSupport"], false, "niente tabstop nella v1");
        assert_eq!(item["insertReplaceSupport"], false);
        let properties = item["resolveSupport"]["properties"].as_array().unwrap();
        assert!(
            !properties
                .iter()
                .any(|property| property == "additionalTextEdits"),
            "senza completionItem/resolve non si promettono modifiche aggiuntive"
        );
        assert!(
            properties
                .iter()
                .any(|property| property == "documentation")
        );
        let formats = json
            .pointer("/textDocument/hover/contentFormat")
            .and_then(serde_json::Value::as_array)
            .expect("hover: formati dichiarati");
        assert!(formats.iter().any(|format| format == "markdown"));
    }
}
