use std::time::Duration;

use lsp_types::{
    ServerCapabilities,
    request::{
        CodeActionRequest, Completion, DocumentSymbolRequest, ExecuteCommand, Formatting,
        GotoDeclaration, GotoDefinition, GotoImplementation, HoverRequest, InlayHintRequest,
        PrepareRenameRequest, RangeFormatting, References, Rename, Request, ResolveCompletionItem,
        SemanticTokensFullDeltaRequest, SemanticTokensFullRequest, SignatureHelpRequest,
        WorkspaceSymbolRequest,
    },
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{ClientError, LspSession, SessionError};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureAvailability {
    pub completion: bool,
    pub completion_resolve: bool,
    pub hover: bool,
    pub signature_help: bool,
    pub definition: bool,
    pub declaration: bool,
    pub implementation: bool,
    pub references: bool,
    pub document_symbols: bool,
    pub workspace_symbols: bool,
    pub prepare_rename: bool,
    pub rename: bool,
    pub formatting: bool,
    pub range_formatting: bool,
    pub code_actions: bool,
    pub execute_command: bool,
    pub semantic_tokens_full: bool,
    pub semantic_tokens_delta: bool,
    pub inlay_hints: bool,
}

impl FeatureAvailability {
    pub fn from_capabilities(capabilities: &ServerCapabilities) -> Self {
        let completion = capabilities.completion_provider.is_some();
        let completion_resolve = capabilities
            .completion_provider
            .as_ref()
            .and_then(|options| options.resolve_provider)
            .unwrap_or(false);
        let rename_value = serde_json::to_value(&capabilities.rename_provider).ok();
        let semantic_value = serde_json::to_value(&capabilities.semantic_tokens_provider).ok();
        Self {
            completion,
            completion_resolve,
            hover: provider_enabled(&capabilities.hover_provider),
            signature_help: provider_enabled(&capabilities.signature_help_provider),
            definition: provider_enabled(&capabilities.definition_provider),
            declaration: provider_enabled(&capabilities.declaration_provider),
            implementation: provider_enabled(&capabilities.implementation_provider),
            references: provider_enabled(&capabilities.references_provider),
            document_symbols: provider_enabled(&capabilities.document_symbol_provider),
            workspace_symbols: provider_enabled(&capabilities.workspace_symbol_provider),
            prepare_rename: rename_value
                .as_ref()
                .and_then(|value| value.as_object())
                .and_then(|object| object.get("prepareProvider"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            rename: provider_enabled(&capabilities.rename_provider),
            formatting: provider_enabled(&capabilities.document_formatting_provider),
            range_formatting: provider_enabled(&capabilities.document_range_formatting_provider),
            code_actions: provider_enabled(&capabilities.code_action_provider),
            execute_command: capabilities.execute_command_provider.is_some(),
            semantic_tokens_full: semantic_full_enabled(semantic_value.as_ref()),
            semantic_tokens_delta: semantic_delta_enabled(semantic_value.as_ref()),
            inlay_hints: provider_enabled(&capabilities.inlay_hint_provider),
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FeatureError {
    #[error("server capability is unavailable for {method}")]
    Unavailable { method: &'static str },
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Client(#[from] ClientError),
}

/// Un cliente **staccato** dalla sessione: il trasporto (che è `Clone`) e le
/// capability negoziate, che è tutto quel che serve per fare una richiesta.
///
/// ATTENZIONE: Esiste per una ragione precisa. [`LspSession::next_event`] vuole
/// `&mut self`, quindi aspettare una risposta *dentro* il ciclo dell'attore
/// vorrebbe dire smettere di consumare le notifiche nel frattempo — e la coda
/// degli eventi del trasporto **non è paziente**: `send_event` usa
/// `try_send`, e a coda piena non aspetta, *disconnette*
/// (`ClientError::Overloaded`, `send_event` in `transport.rs`). Una risposta lenta
/// mentre il server parla molto ucciderebbe la sessione.
///
/// Con un cliente staccato la richiesta vive in un future suo, il ciclo
/// continua a svuotare le notifiche, e il `didChange` di chi sta scrivendo
/// non aspetta dietro un `references` freddo.
///
/// ATTENZIONE: **Un cliente non sopravvive a un riavvio**: il trasporto che porta
/// dentro punta al processo di prima. Chi lo tiene in caldo deve richiederlo
/// dopo ogni [`LspSession::restart`].
#[derive(Clone)]
pub struct FeatureClient {
    transport: crate::ClientTransport,
    availability: FeatureAvailability,
}

impl std::fmt::Debug for FeatureClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureClient")
            .field("availability", &self.availability)
            .finish_non_exhaustive()
    }
}

impl FeatureClient {
    pub const fn availability(&self) -> &FeatureAvailability {
        &self.availability
    }

    async fn feature_request<R>(
        &self,
        available: bool,
        params: R::Params,
        timeout: Duration,
    ) -> Result<R::Result, FeatureError>
    where
        R: Request,
        R::Params: Serialize,
        R::Result: DeserializeOwned,
    {
        if !available {
            return Err(FeatureError::Unavailable { method: R::METHOD });
        }
        Ok(self.transport.request(R::METHOD, params, timeout).await?)
    }

    pub async fn completion(
        &self,
        params: <Completion as Request>::Params,
        timeout: Duration,
    ) -> Result<<Completion as Request>::Result, FeatureError> {
        self.feature_request::<Completion>(self.availability.completion, params, timeout)
            .await
    }

    pub async fn resolve_completion(
        &self,
        params: <ResolveCompletionItem as Request>::Params,
        timeout: Duration,
    ) -> Result<<ResolveCompletionItem as Request>::Result, FeatureError> {
        self.feature_request::<ResolveCompletionItem>(
            self.availability.completion_resolve,
            params,
            timeout,
        )
        .await
    }
}

impl LspSession {
    pub fn feature_availability(&self) -> FeatureAvailability {
        self.negotiated_capabilities()
            .map(|negotiated| FeatureAvailability::from_capabilities(&negotiated.server))
            .unwrap_or_default()
    }

    /// Il cliente staccato di **questa** sessione, se è pronta a servire.
    pub fn feature_client(&self) -> Result<FeatureClient, SessionError> {
        Ok(FeatureClient {
            transport: self.feature_transport()?.clone(),
            availability: self.feature_availability(),
        })
    }

    pub async fn completion(
        &self,
        params: <Completion as Request>::Params,
        timeout: Duration,
    ) -> Result<<Completion as Request>::Result, FeatureError> {
        self.feature_client()?.completion(params, timeout).await
    }

    pub async fn resolve_completion(
        &self,
        params: <ResolveCompletionItem as Request>::Params,
        timeout: Duration,
    ) -> Result<<ResolveCompletionItem as Request>::Result, FeatureError> {
        self.feature_client()?
            .resolve_completion(params, timeout)
            .await
    }
}

macro_rules! feature_method {
    ($name:ident, $request:ty, $availability:ident) => {
        impl FeatureClient {
            pub async fn $name(
                &self,
                params: <$request as Request>::Params,
                timeout: Duration,
            ) -> Result<<$request as Request>::Result, FeatureError> {
                self.feature_request::<$request>(self.availability.$availability, params, timeout)
                    .await
            }
        }

        // La sessione conserva la sua faccia: chi la usa da fuori — i test di
        // `tests/features.rs` compresi — non si accorge di niente.
        impl LspSession {
            pub async fn $name(
                &self,
                params: <$request as Request>::Params,
                timeout: Duration,
            ) -> Result<<$request as Request>::Result, FeatureError> {
                self.feature_client()?.$name(params, timeout).await
            }
        }
    };
}

feature_method!(hover, HoverRequest, hover);
feature_method!(signature_help, SignatureHelpRequest, signature_help);
feature_method!(definition, GotoDefinition, definition);
feature_method!(declaration, GotoDeclaration, declaration);
feature_method!(implementation, GotoImplementation, implementation);
feature_method!(references, References, references);
feature_method!(document_symbols, DocumentSymbolRequest, document_symbols);
feature_method!(workspace_symbols, WorkspaceSymbolRequest, workspace_symbols);
feature_method!(prepare_rename, PrepareRenameRequest, prepare_rename);
feature_method!(rename, Rename, rename);
feature_method!(format_document, Formatting, formatting);
feature_method!(format_range, RangeFormatting, range_formatting);
feature_method!(code_actions, CodeActionRequest, code_actions);
feature_method!(execute_command, ExecuteCommand, execute_command);
feature_method!(
    semantic_tokens_full,
    SemanticTokensFullRequest,
    semantic_tokens_full
);
feature_method!(
    semantic_tokens_delta,
    SemanticTokensFullDeltaRequest,
    semantic_tokens_delta
);
feature_method!(inlay_hints, InlayHintRequest, inlay_hints);

fn provider_enabled<T: Serialize>(provider: &Option<T>) -> bool {
    match provider
        .as_ref()
        .and_then(|provider| serde_json::to_value(provider).ok())
    {
        Some(serde_json::Value::Bool(value)) => value,
        Some(serde_json::Value::Null) | None => false,
        Some(_) => true,
    }
}

fn semantic_full_enabled(provider: Option<&serde_json::Value>) -> bool {
    match provider
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("full"))
    {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::Object(_)) => true,
        _ => false,
    }
}

fn semantic_delta_enabled(provider: Option<&serde_json::Value>) -> bool {
    provider
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("full"))
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("delta"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
