#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lsp_types::{Position, Range, TextDocumentSyncKind, Uri};
use lume_lsp::{
    CommandSource, DocumentChange, DocumentStore, FrameDecoder, InitializeSpec, LspSession,
    RpcMessage, ServerCommand, SessionError, SessionEvent, SessionState, TransportConfig,
    encode_message,
};
use serde_json::{Value, json};

const SERVER_MODE: &str = "LUME_SESSION_FAKE_SERVER";
const RECORD_FILE: &str = "LUME_SESSION_RECORD_FILE";
const SPAWN_FILE: &str = "LUME_SESSION_SPAWN_FILE";
const CRASH_FIRST_CHANGE: &str = "LUME_SESSION_CRASH_FIRST_CHANGE";

fn uri(value: &str) -> Uri {
    value.parse().expect("valid document URI")
}

fn document_versions_are_monotonic_and_incremental_changes_preserve_ranges() {
    let document_uri = uri("file:///workspace/src/main.rs");
    let mut documents = DocumentStore::new(TextDocumentSyncKind::INCREMENTAL);
    let opened = documents
        .open(document_uri.clone(), "rust".into(), "fn main() {}".into())
        .expect("open document");
    assert_eq!(opened.text_document.version, 1);

    let range = Range::new(Position::new(0, 3), Position::new(0, 7));
    let changed = documents
        .change(
            &document_uri,
            DocumentChange::Incremental {
                range,
                range_length: Some(4),
                text: "entry".into(),
                resulting_text: "fn entry() {}".into(),
            },
        )
        .expect("change document");
    assert_eq!(changed.text_document.version, 2);
    assert_eq!(changed.content_changes[0].range, Some(range));

    let reopened = documents.reopen_documents();
    assert_eq!(reopened.len(), 1);
    assert_eq!(reopened[0].text_document.version, 2);
    assert_eq!(reopened[0].text_document.text, "fn entry() {}");
}

fn change_text_emits_the_minimal_utf16_edit_and_preserves_the_full_fallback() {
    let document_uri = uri("file:///workspace/src/unicode.rs");
    let mut documents = DocumentStore::new(TextDocumentSyncKind::INCREMENTAL);
    documents
        .open(document_uri.clone(), "rust".into(), "a😀z\n".into())
        .expect("open Unicode document");

    let replaced = documents
        .change_text(&document_uri, "a漢z\n")
        .expect("replace Unicode scalar");
    assert_eq!(replaced.text_document.version, 2);
    assert_eq!(replaced.content_changes.len(), 1);
    assert_eq!(
        replaced.content_changes[0].range,
        Some(Range::new(Position::new(0, 1), Position::new(0, 3)))
    );
    assert_eq!(replaced.content_changes[0].range_length, Some(2));
    assert_eq!(replaced.content_changes[0].text, "漢");

    let inserted = documents
        .change_text(&document_uri, "a漢😀z\n")
        .expect("insert supplementary scalar");
    assert_eq!(
        inserted.content_changes[0].range,
        Some(Range::new(Position::new(0, 2), Position::new(0, 2)))
    );
    assert_eq!(inserted.content_changes[0].range_length, Some(0));
    assert_eq!(inserted.content_changes[0].text, "😀");

    let mut full_documents = DocumentStore::new(TextDocumentSyncKind::FULL);
    full_documents
        .open(document_uri.clone(), "rust".into(), "a😀z\n".into())
        .expect("open full-sync document");
    let full = full_documents
        .change_text(&document_uri, "a漢z\n")
        .expect("full-sync fallback");
    assert_eq!(full.content_changes[0].range, None);
    assert_eq!(full.content_changes[0].range_length, None);
    assert_eq!(full.content_changes[0].text, "a漢z\n");
}

fn change_text_keeps_character_boundaries_and_multiline_positions() {
    let document_uri = uri("file:///workspace/src/unicode-boundaries.rs");
    let mut documents = DocumentStore::new(TextDocumentSyncKind::INCREMENTAL);
    documents
        .open(document_uri.clone(), "rust".into(), "a🟦\nend".into())
        .expect("open document");

    let changed = documents
        .change_text(&document_uri, "a🟩\nend")
        .expect("replace scalar sharing UTF-8 prefix bytes");
    assert_eq!(
        changed.content_changes[0].range,
        Some(Range::new(Position::new(0, 1), Position::new(0, 3)))
    );
    assert_eq!(changed.content_changes[0].range_length, Some(2));
    assert_eq!(changed.content_changes[0].text, "🟩");

    let multiline = documents
        .change_text(&document_uri, "a🟩\n!end")
        .expect("insert at start of second line");
    assert_eq!(
        multiline.content_changes[0].range,
        Some(Range::new(Position::new(1, 0), Position::new(1, 0)))
    );
    assert_eq!(multiline.content_changes[0].text, "!");
}

fn full_sync_converts_editor_deltas_to_full_text_and_rejects_duplicate_ownership() {
    let document_uri = uri("file:///workspace/Main.java");
    let mut disabled = DocumentStore::new(TextDocumentSyncKind::NONE);
    assert!(
        disabled
            .open(document_uri.clone(), "java".into(), "class Main {}".into(),)
            .is_err(),
        "a server that negotiated no synchronization must receive no didOpen"
    );
    let mut documents = DocumentStore::new(TextDocumentSyncKind::FULL);
    documents
        .open(document_uri.clone(), "java".into(), "class Main {}".into())
        .expect("open document");
    assert!(
        documents
            .open(document_uri.clone(), "java".into(), String::new())
            .is_err(),
        "one session must not own the same URI twice"
    );

    let changed = documents
        .change(
            &document_uri,
            DocumentChange::Incremental {
                range: Range::new(Position::new(0, 6), Position::new(0, 10)),
                range_length: Some(4),
                text: "App".into(),
                resulting_text: "class App {}".into(),
            },
        )
        .expect("full-sync change");
    assert_eq!(changed.text_document.version, 2);
    assert_eq!(changed.content_changes[0].range, None);
    assert_eq!(changed.content_changes[0].text, "class App {}");

    let closed = documents.close(&document_uri).expect("close document");
    assert_eq!(closed.text_document.uri, document_uri);
    assert!(documents.reopen_documents().is_empty());
}

struct TempFile(PathBuf);

impl TempFile {
    fn unique(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "lume-session-{label}-{}-{nonce}.txt",
            std::process::id()
        )))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn fake_command(record: &Path, spawn: &Path, crash_first_change: bool) -> ServerCommand {
    let mut env = vec![
        (OsString::from(SERVER_MODE), OsString::from("1")),
        (OsString::from(RECORD_FILE), record.as_os_str().to_owned()),
        (OsString::from(SPAWN_FILE), spawn.as_os_str().to_owned()),
    ];
    if crash_first_change {
        env.push((OsString::from(CRASH_FIRST_CHANGE), OsString::from("1")));
    }
    ServerCommand {
        program: std::env::current_exe().expect("session integration-test executable"),
        args: vec![OsString::from("--fake-session-server")],
        cwd: None,
        env,
        source: CommandSource::Bundled,
    }
}

fn append(path: &Path, value: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open fake session record");
    writeln!(file, "{value}").expect("append fake session record");
}

fn fake_server_main() {
    let record = PathBuf::from(std::env::var_os(RECORD_FILE).expect("record file"));
    let spawn_file = PathBuf::from(std::env::var_os(SPAWN_FILE).expect("spawn file"));
    let first_spawn = !spawn_file.exists();
    append(&spawn_file, "spawn");
    let crash_on_change = first_spawn && std::env::var_os(CRASH_FIRST_CHANGE).is_some();
    let mut decoder = FrameDecoder::default();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = input.read(&mut buffer).expect("read session message");
        if read == 0 {
            return;
        }
        for frame in decoder.push(&buffer[..read]).expect("decode session frame") {
            let message: RpcMessage = serde_json::from_slice(&frame).expect("parse session RPC");
            let (method, response_id) = match message {
                RpcMessage::Request(request) => (request.method, Some(request.id)),
                RpcMessage::Notification(notification) => (notification.method, None),
                RpcMessage::Response(_) => continue,
            };
            append(&record, &method);
            if crash_on_change && method == "textDocument/didChange" {
                std::process::exit(41);
            }
            if let Some(id) = response_id {
                let result = if method == "initialize" {
                    json!({ "capabilities": { "textDocumentSync": 2 } })
                } else {
                    Value::Null
                };
                output
                    .write_all(&encode_message(&RpcMessage::success(id, result)).unwrap())
                    .expect("write session response");
                output.flush().expect("flush session response");
            }
            if method == "exit" {
                return;
            }
        }
    }
}

async fn session_negotiates_sync_and_enforces_state_order() {
    let record = TempFile::unique("state-record");
    let spawn = TempFile::unique("state-spawn");
    let mut session = LspSession::spawn(
        fake_command(&record.0, &spawn.0, false),
        TransportConfig::default(),
    )
    .expect("spawn session");
    assert_eq!(session.state(), SessionState::Starting);
    assert!(matches!(
        session.open_document(
            uri("file:///workspace/early.rs"),
            "rust".into(),
            String::new()
        ),
        Err(SessionError::NotReady { .. })
    ));

    let negotiated = session
        .initialize(InitializeSpec::default())
        .await
        .expect("initialize session");
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(negotiated.sync_kind, TextDocumentSyncKind::INCREMENTAL);
    session
        .open_document(
            uri("file:///workspace/src/main.rs"),
            "rust".into(),
            "fn main() {}".into(),
        )
        .expect("send didOpen");
    session
        .change_document(
            &uri("file:///workspace/src/main.rs"),
            DocumentChange::Full("fn main() { println!(\"ok\"); }".into()),
        )
        .expect("send didChange");
    session
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown session");
    assert_eq!(session.state(), SessionState::Stopped);
}

async fn unexpected_crash_restarts_once_and_reopens_documents() {
    let record = TempFile::unique("restart-record");
    let spawn = TempFile::unique("restart-spawn");
    let document_uri = uri("file:///workspace/src/lib.rs");
    let mut session = LspSession::spawn(
        fake_command(&record.0, &spawn.0, true),
        TransportConfig::default(),
    )
    .expect("spawn restart session");
    session
        .initialize(InitializeSpec::default())
        .await
        .expect("initialize restart session");
    session
        .open_document(document_uri.clone(), "rust".into(), "pub fn a() {}".into())
        .expect("open restart document");
    session
        .change_document(&document_uri, DocumentChange::Full("pub fn b() {}".into()))
        .expect("trigger first-server crash");

    // `next_event` **non** riavvia più da sé: dirlo e basta è ciò che la
    // rende cancel-safe, e quindi utilizzabile dentro un `select!`.
    let event = tokio::time::timeout(Duration::from_secs(5), session.next_event())
        .await
        .expect("disconnect event deadline")
        .expect("disconnect event");
    assert!(matches!(event, SessionEvent::Disconnected(_)));
    // il riavvio è un passo del chiamante, e non è interrompibile
    session.restart().await.expect("restart after disconnect");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let methods = std::fs::read_to_string(&record.0).unwrap_or_default();
        if methods.lines().filter(|line| *line == "initialize").count() >= 2
            && methods
                .lines()
                .filter(|line| *line == "textDocument/didOpen")
                .count()
                >= 2
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "restart did not replay documents"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    session
        .shutdown(Duration::from_secs(2))
        .await
        .expect("shutdown restarted session");
}

/// ATTENZIONE: La politica «un tentativo solo» non è cambiata col riavvio esplicito:
/// il secondo `restart` fallisce, e la sessione resta ferma. Senza questo,
/// spostare il riavvio fuori da `next_event` avrebbe potuto trasformarlo in
/// un ciclo di rinascite a spese della macchina dell'utente.
async fn a_crash_is_reported_not_hidden() {
    let record = TempFile::unique("disconnect-record");
    let spawn = TempFile::unique("disconnect-spawn");
    let document_uri = uri("file:///workspace/src/lib.rs");
    let mut session = LspSession::spawn(
        fake_command(&record.0, &spawn.0, true),
        TransportConfig::default(),
    )
    .expect("spawn disconnect session");
    session
        .initialize(InitializeSpec::default())
        .await
        .expect("initialize disconnect session");
    session
        .open_document(document_uri.clone(), "rust".into(), "pub fn a() {}".into())
        .expect("open disconnect document");
    session
        .change_document(&document_uri, DocumentChange::Full("pub fn b() {}".into()))
        .expect("trigger crash");

    let event = tokio::time::timeout(Duration::from_secs(5), session.next_event())
        .await
        .expect("disconnect deadline")
        .expect("disconnect event");
    assert!(matches!(event, SessionEvent::Disconnected(_)));
    session.restart().await.expect("first restart");
    assert!(
        session.restart().await.is_err(),
        "un tentativo solo: il secondo riavvio deve rifiutare"
    );
}

#[tokio::main]
async fn main() {
    if std::env::var_os(SERVER_MODE).is_some()
        && std::env::args().any(|argument| argument == "--fake-session-server")
    {
        fake_server_main();
        return;
    }
    document_versions_are_monotonic_and_incremental_changes_preserve_ranges();
    eprintln!("session::documents_incremental ... ok");
    change_text_emits_the_minimal_utf16_edit_and_preserves_the_full_fallback();
    change_text_keeps_character_boundaries_and_multiline_positions();
    eprintln!("session::change_text_utf16 ... ok");
    full_sync_converts_editor_deltas_to_full_text_and_rejects_duplicate_ownership();
    eprintln!("session::documents_full ... ok");
    session_negotiates_sync_and_enforces_state_order().await;
    eprintln!("session::state_and_sync ... ok");
    unexpected_crash_restarts_once_and_reopens_documents().await;
    eprintln!("session::restart_replay ... ok");
    a_crash_is_reported_not_hidden().await;
    eprintln!("session::disconnect_is_reported ... ok");
}
