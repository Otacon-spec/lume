#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::{
    ffi::OsString,
    fs::OpenOptions,
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
use lume_lsp::{
    CommandSource, FeatureAvailability, FeatureError, FrameDecoder, InitializeSpec, LspSession,
    RpcMessage, ServerCommand, TransportConfig, encode_message,
};
use serde_json::{Value, json};

const SERVER_MODE: &str = "LUME_FEATURE_FAKE_SERVER";
const RECORD_FILE: &str = "LUME_FEATURE_RECORD_FILE";
const CAPABILITIES: &str = "LUME_FEATURE_CAPABILITIES";

fn assert_request<R: Request>(_future: impl Future<Output = Result<R::Result, FeatureError>>) {}

#[allow(dead_code, clippy::too_many_arguments)]
fn typed_contracts_compile(
    session: &LspSession,
    completion: <Completion as Request>::Params,
    completion_item: <ResolveCompletionItem as Request>::Params,
    hover: <HoverRequest as Request>::Params,
    definition: <GotoDefinition as Request>::Params,
    declaration: <GotoDeclaration as Request>::Params,
    implementation: <GotoImplementation as Request>::Params,
    references: <References as Request>::Params,
    document_symbols: <DocumentSymbolRequest as Request>::Params,
    workspace_symbols: <WorkspaceSymbolRequest as Request>::Params,
    prepare_rename: <PrepareRenameRequest as Request>::Params,
    rename: <Rename as Request>::Params,
    formatting: <Formatting as Request>::Params,
    range_formatting: <RangeFormatting as Request>::Params,
    code_actions: <CodeActionRequest as Request>::Params,
    execute_command: <ExecuteCommand as Request>::Params,
    semantic_full: <SemanticTokensFullRequest as Request>::Params,
    semantic_delta: <SemanticTokensFullDeltaRequest as Request>::Params,
    inlay_hints: <InlayHintRequest as Request>::Params,
    signature_help: <SignatureHelpRequest as Request>::Params,
) {
    let timeout = Duration::from_secs(1);
    assert_request::<Completion>(session.completion(completion, timeout));
    assert_request::<ResolveCompletionItem>(session.resolve_completion(completion_item, timeout));
    assert_request::<HoverRequest>(session.hover(hover, timeout));
    assert_request::<GotoDefinition>(session.definition(definition, timeout));
    assert_request::<GotoDeclaration>(session.declaration(declaration, timeout));
    assert_request::<GotoImplementation>(session.implementation(implementation, timeout));
    assert_request::<References>(session.references(references, timeout));
    assert_request::<DocumentSymbolRequest>(session.document_symbols(document_symbols, timeout));
    assert_request::<WorkspaceSymbolRequest>(session.workspace_symbols(workspace_symbols, timeout));
    assert_request::<PrepareRenameRequest>(session.prepare_rename(prepare_rename, timeout));
    assert_request::<Rename>(session.rename(rename, timeout));
    assert_request::<Formatting>(session.format_document(formatting, timeout));
    assert_request::<RangeFormatting>(session.format_range(range_formatting, timeout));
    assert_request::<CodeActionRequest>(session.code_actions(code_actions, timeout));
    assert_request::<ExecuteCommand>(session.execute_command(execute_command, timeout));
    assert_request::<SemanticTokensFullRequest>(
        session.semantic_tokens_full(semantic_full, timeout),
    );
    assert_request::<SemanticTokensFullDeltaRequest>(
        session.semantic_tokens_delta(semantic_delta, timeout),
    );
    assert_request::<InlayHintRequest>(session.inlay_hints(inlay_hints, timeout));
    assert_request::<SignatureHelpRequest>(session.signature_help(signature_help, timeout));
}

fn availability_is_derived_only_from_negotiated_server_capabilities() {
    let capabilities: ServerCapabilities = serde_json::from_value(json!({
        "completionProvider": { "resolveProvider": true },
        "hoverProvider": false,
        "signatureHelpProvider": { "triggerCharacters": ["(", ","] },
        "definitionProvider": true,
        "declarationProvider": { "workDoneProgress": false },
        "implementationProvider": true,
        "referencesProvider": true,
        "documentSymbolProvider": true,
        "workspaceSymbolProvider": false,
        "renameProvider": { "prepareProvider": true },
        "documentFormattingProvider": true,
        "documentRangeFormattingProvider": false,
        "codeActionProvider": true,
        "executeCommandProvider": { "commands": ["fixture.run"] },
        "semanticTokensProvider": {
            "legend": { "tokenTypes": [], "tokenModifiers": [] },
            "full": { "delta": true }
        },
        "inlayHintProvider": true
    }))
    .expect("valid server capabilities");

    let availability = FeatureAvailability::from_capabilities(&capabilities);
    assert!(availability.completion);
    assert!(availability.completion_resolve);
    assert!(!availability.hover);
    assert!(availability.signature_help);
    assert!(availability.definition);
    assert!(availability.declaration);
    assert!(availability.implementation);
    assert!(availability.references);
    assert!(availability.document_symbols);
    assert!(!availability.workspace_symbols);
    assert!(availability.prepare_rename);
    assert!(availability.rename);
    assert!(availability.formatting);
    assert!(!availability.range_formatting);
    assert!(availability.code_actions);
    assert!(availability.execute_command);
    assert!(availability.semantic_tokens_full);
    assert!(availability.semantic_tokens_delta);
    assert!(availability.inlay_hints);
}

struct TempFile(PathBuf);

impl TempFile {
    fn unique(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "lume-feature-{label}-{}-{nonce}.txt",
            std::process::id()
        )))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn append(path: &Path, method: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open feature record");
    writeln!(file, "{method}").expect("append feature record");
}

fn fake_command(record: &Path, capabilities: Value) -> ServerCommand {
    ServerCommand {
        program: std::env::current_exe().expect("feature test executable"),
        args: vec![OsString::from("--fake-feature-server")],
        cwd: None,
        env: vec![
            (OsString::from(SERVER_MODE), OsString::from("1")),
            (OsString::from(RECORD_FILE), record.as_os_str().to_owned()),
            (
                OsString::from(CAPABILITIES),
                OsString::from(capabilities.to_string()),
            ),
        ],
        source: CommandSource::Bundled,
    }
}

fn feature_fake_server() {
    if std::env::var_os(SERVER_MODE).is_none() {
        return;
    }
    let record = PathBuf::from(std::env::var_os(RECORD_FILE).expect("feature record"));
    let capabilities: Value =
        serde_json::from_str(&std::env::var(CAPABILITIES).expect("feature capabilities"))
            .expect("parse feature capabilities");
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut decoder = FrameDecoder::default();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = input.read(&mut buffer).expect("read feature RPC");
        if read == 0 {
            return;
        }
        for frame in decoder.push(&buffer[..read]).expect("decode feature RPC") {
            let message: RpcMessage = serde_json::from_slice(&frame).expect("parse feature RPC");
            let (method, response_id, params) = match message {
                RpcMessage::Request(request) => (request.method, Some(request.id), request.params),
                RpcMessage::Notification(notification) => {
                    (notification.method, None, notification.params)
                }
                RpcMessage::Response(_) => continue,
            };
            append(&record, &method);
            if method == "initialize"
                && params
                    .pointer("/capabilities/general/positionEncodings")
                    .is_some()
                && params
                    .pointer("/capabilities/textDocument/completion")
                    .is_some()
                && params
                    .pointer("/capabilities/textDocument/semanticTokens")
                    .is_some()
                && params
                    .pointer("/capabilities/textDocument/inlayHint")
                    .is_some()
                && params.pointer("/capabilities/workspace/symbol").is_some()
            {
                append(&record, "clientCapabilitiesOk");
            }
            if let Some(id) = response_id {
                let result = if method == "initialize" {
                    json!({ "capabilities": capabilities })
                } else {
                    Value::Null
                };
                output
                    .write_all(&encode_message(&RpcMessage::success(id, result)).unwrap())
                    .expect("write feature response");
                output.flush().expect("flush feature response");
            }
            if method == "exit" {
                return;
            }
        }
    }
}

async fn unavailable_features_return_without_sending_rpc() {
    let record = TempFile::unique("unavailable");
    let mut session = LspSession::spawn(
        fake_command(&record.0, json!({ "hoverProvider": false })),
        TransportConfig::default(),
    )
    .expect("spawn feature session");
    session
        .initialize(InitializeSpec::default())
        .await
        .expect("initialize feature session");
    let params = serde_json::from_value(json!({
        "textDocument": { "uri": "file:///workspace/main.rs" },
        "position": { "line": 0, "character": 0 }
    }))
    .expect("hover params");
    assert!(matches!(
        session.hover(params, Duration::from_secs(1)).await,
        Err(FeatureError::Unavailable {
            method: "textDocument/hover"
        })
    ));
    let methods = std::fs::read_to_string(&record.0).unwrap_or_default();
    assert!(!methods.lines().any(|method| method == "textDocument/hover"));
    session
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown feature session");
}

async fn available_completion_uses_the_typed_method_and_result() {
    let record = TempFile::unique("completion");
    let mut session = LspSession::spawn(
        fake_command(&record.0, json!({ "completionProvider": {} })),
        TransportConfig::default(),
    )
    .expect("spawn completion session");
    session
        .initialize(InitializeSpec::default())
        .await
        .expect("initialize completion session");
    let params = serde_json::from_value(json!({
        "textDocument": { "uri": "file:///workspace/main.rs" },
        "position": { "line": 0, "character": 0 }
    }))
    .expect("completion params");
    let result = session
        .completion(params, Duration::from_secs(1))
        .await
        .expect("completion response");
    assert_eq!(result, None);
    let methods = std::fs::read_to_string(&record.0).expect("completion record");
    assert!(
        methods
            .lines()
            .any(|method| method == "textDocument/completion")
    );
    assert!(
        methods
            .lines()
            .any(|method| method == "clientCapabilitiesOk")
    );
    session
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown completion session");
}

#[tokio::main]
async fn main() {
    if std::env::var_os(SERVER_MODE).is_some()
        && std::env::args().any(|argument| argument == "--fake-feature-server")
    {
        feature_fake_server();
        return;
    }
    availability_is_derived_only_from_negotiated_server_capabilities();
    eprintln!("features::availability ... ok");
    unavailable_features_return_without_sending_rpc().await;
    eprintln!("features::unavailable_no_traffic ... ok");
    available_completion_uses_the_typed_method_and_result().await;
    eprintln!("features::typed_completion ... ok");
}
