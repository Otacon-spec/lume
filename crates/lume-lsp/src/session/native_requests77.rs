//! Richieste del server reale → gestore privato di produzione → progresso/hover.
//! Caso originale con dequeue esplicito; variante con next_event e osservatore wire.
//! Nessuna prova del routing actor/app/GUI.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::{CommandSource, FeatureError, RpcRequest, ServerReply};
use futures::FutureExt;
use lsp_types::{NumberOrString, Position, ProgressParamsValue, WorkDoneProgress};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    future::Future,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

type Esito<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const ATTESA: Duration = Duration::from_secs(30);
const TESTO: &str = "pub struct Saluto { pub valore: i32 }\npub fn prova() {\n    let saluto = Saluto { valore: 42 };\n    let _emoji = \"😀\"; saluto.valore;\n}\n";

// Nessuna pulizia automatica: un timeout o un panic non dimostrano raccolta.
struct Directory77(PathBuf);
impl Directory77 {
    fn nuova() -> Esito<Self> {
        let path = std::env::temp_dir().join(format!(
            "lume-request77-città spazio-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        #[allow(unused_mut)]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }
    fn cleanup(self) -> Esito {
        std::fs::remove_dir_all(self.0)?;
        Ok(())
    }
}
fn uri77(path: &Path) -> Esito<Uri> {
    if !path.is_absolute() {
        return Err(io::Error::other("URI fixture non assoluta").into());
    }
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::other("URI fixture non UTF8"))?;
    let mut out = String::from("file://");
    #[cfg(windows)]
    out.push('/');
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            #[cfg(windows)]
            if byte == b'\\' {
                out.push('/');
                continue;
            }
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(out.parse()?)
}
async fn limitato77<F, T>(future: F) -> Esito<T>
where
    F: Future<Output = Esito<T>>,
{
    match tokio::time::timeout(ATTESA, std::panic::AssertUnwindSafe(future).catch_unwind()).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            Err(io::Error::other("panic nella prova nativa, owner da raccogliere").into())
        }
        Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "scadenza prova nativa").into()),
    }
}
async fn evento77(session: &mut LspSession) -> Esito<TransportEvent> {
    session
        .process
        .as_mut()
        .ok_or_else(|| io::Error::other("owner processo non disponibile"))?
        .events()
        .recv()
        .await
        .ok_or_else(|| io::Error::other("EOF transport durante la prova").into())
}

#[derive(Default)]
struct Osservazione77 {
    created: Vec<NumberOrString>,
    observed_ids: Vec<crate::RequestId>,
    fetching_create: usize,
    fetching_begin: usize,
    fetching_report: usize,
    fetching_end: usize,
}
fn fetching77() -> NumberOrString {
    NumberOrString::String("rustAnalyzer/Fetching".into())
}
fn registra77(request: &RpcRequest, seen: &mut Osservazione77) -> Esito {
    if request.method == "window/workDoneProgress/create" {
        let token: NumberOrString = serde_json::from_value(
            request
                .params
                .get("token")
                .ok_or_else(|| io::Error::other("create nativo senza token"))?
                .clone(),
        )?;
        assert_eq!(
            crate::reply_to_server_request(request),
            ServerReply::Result(Value::Null),
            "mapping del gestore sul DTO nativo immutato"
        );
        assert!(
            !seen.created.contains(&token),
            "token create nativo duplicato prima della fine"
        );
        if token == fetching77() {
            seen.fetching_create += 1;
        }
        seen.created.push(token);
    }
    Ok(())
}
fn rispondi77(session: &LspSession, request: RpcRequest, seen: &mut Osservazione77) -> Esito {
    registra77(&request, seen)?;
    // La stessa funzione privata chiamata da next_event; nessuna reply di test.
    // Il suo None è ambiguo se manca il transport: qui la disponibilità deve
    // essere verificata esplicitamente, senza trasformare un'assenza in verde.
    session.transport()?;
    if session.answer_server_request(&request).is_some() {
        return Err(
            io::Error::other("gestore produzione non ha accodato una reply positiva").into(),
        );
    }
    Ok(())
}
fn progresso77(notification: RpcNotification, seen: &mut Osservazione77) -> Esito {
    if notification.method != "$/progress" {
        return Ok(());
    }
    let progress: lsp_types::ProgressParams = serde_json::from_value(notification.params)?;
    assert!(
        seen.created.contains(&progress.token),
        "Begin/Report/End legato a un create realmente ricevuto e ancora attivo"
    );
    let end = matches!(
        &progress.value,
        ProgressParamsValue::WorkDone(WorkDoneProgress::End(_))
    );
    if progress.token == fetching77() {
        match progress.value {
            ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(_)) => {
                assert_eq!(seen.fetching_begin, seen.fetching_end);
                seen.fetching_begin += 1;
            }
            ProgressParamsValue::WorkDone(WorkDoneProgress::Report(_)) => {
                assert_eq!(seen.fetching_begin, seen.fetching_end + 1);
                seen.fetching_report += 1;
            }
            ProgressParamsValue::WorkDone(WorkDoneProgress::End(_)) => {
                assert_eq!(seen.fetching_begin, seen.fetching_end + 1);
                seen.fetching_end += 1;
            }
        }
    }
    if end {
        // La stessa stringa è riutilizzabile DOPO End, anche nella stessa
        // sessione. Non confondere identità del token e lifetime dell'attività.
        seen.created.retain(|token| token != &progress.token);
    }
    Ok(())
}
fn consuma77(session: &LspSession, event: TransportEvent, seen: &mut Osservazione77) -> Esito {
    match event {
        TransportEvent::Request(request) => rispondi77(session, request, seen),
        TransportEvent::Notification(notification) => progresso77(notification, seen),
        TransportEvent::Disconnected(_) => {
            Err(io::Error::other("transport nativo disconnesso; log grezzo non pubblicato").into())
        }
    }
}

type Audit77 = std::sync::Arc<std::sync::Mutex<crate::transport::ServerWireAudit>>;

async fn consuma_evento77(
    session: &mut LspSession,
    seen: &mut Osservazione77,
    audit: Option<&Audit77>,
) -> Esito {
    let Some(audit) = audit else {
        let event = evento77(session).await?;
        return consuma77(session, event, seen);
    };
    // Nessuna lettura diretta events(), risposta o invocazione del gestore.
    let event = session
        .next_event()
        .await
        .ok_or_else(|| io::Error::other("EOF dispatcher"))?;
    // Il reader può avere già decodificato un futuro riuso dello stesso
    // token. Associare solo il create del Begin corrente, non tutto il backlog.
    if let SessionEvent::Progress(progress) = &event
        && matches!(
            progress.value,
            ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(_))
        )
    {
        let audit = audit.lock().unwrap();
        assert!(!audit.overflow, "osservatore wire incompleto");
        let request = audit
            .requests
            .iter()
            .find(|request| {
                request.method == "window/workDoneProgress/create"
                    && !seen.observed_ids.contains(&request.id)
                    && request.params.get("token")
                        == Some(&serde_json::to_value(&progress.token).unwrap())
            })
            .ok_or_else(|| io::Error::other("Begin senza create nativo osservato"))?;
        registra77(request, seen)?;
        seen.observed_ids.push(request.id);
    }
    match event {
        SessionEvent::Progress(progress) => progresso77(
            RpcNotification {
                jsonrpc: crate::JsonRpcVersion::V2,
                method: "$/progress".into(),
                params: serde_json::to_value(progress)?,
            },
            seen,
        ),
        SessionEvent::Notification(_) | SessionEvent::Diagnostics(_) => Ok(()),
        other => Err(io::Error::other(format!("evento dispatcher inatteso: {other:?}")).into()),
    }
}

fn verifica_wire77(audit: &Audit77) {
    let audit = audit.lock().unwrap();
    assert!(!audit.overflow);
    assert!(
        !audit.requests.is_empty(),
        "richieste native obbligatorie, nessuno skip"
    );
    for request in &audit.requests {
        assert_eq!(
            audit.requests.iter().filter(|r| r.id == request.id).count(),
            1,
            "ID server duplicato"
        );
        let responses: Vec<_> = audit
            .responses
            .iter()
            .filter(|r| r.id == request.id)
            .collect();
        assert_eq!(
            responses.len(),
            1,
            "ogni richiesta ha esattamente una risposta scritta"
        );
        let expected = match crate::reply_to_server_request(request) {
            ServerReply::Result(value) => crate::RpcResponsePayload::Result(value),
            ServerReply::Error { .. } => panic!("il caso nativo richiede metodi supportati"),
        };
        assert_eq!(responses[0].payload, expected);
    }
    assert_eq!(audit.responses.len(), audit.requests.len());
    assert!(
        audit
            .requests
            .iter()
            .any(|r| r.method == "window/workDoneProgress/create")
    );
    eprintln!(
        "ROUTING_REALE77:requests={};written_responses={};duplicate_responses=0",
        audit.requests.len(),
        audit.responses.len()
    );
}

async fn hover77(
    session: &mut LspSession,
    uri: &Uri,
    seen: &mut Osservazione77,
    audit: Option<&Audit77>,
) -> Esito {
    let feature = session.feature_client()?;
    let before = &TESTO[..TESTO.rfind("valore;").unwrap() + 1];
    let position = Position::new(
        u32::try_from(before.bytes().filter(|&b| b == b'\n').count())?,
        u32::try_from(before.rsplit('\n').next().unwrap().encode_utf16().count())?,
    );
    for _ in 0..32 {
        let params =
            serde_json::from_value(json!({"textDocument":{"uri":uri},"position":position}))?;
        let future = feature.hover(params, ATTESA);
        tokio::pin!(future);
        let result = loop {
            tokio::select! {
                result = &mut future => break result,
                result = consuma_evento77(session, seen, audit) => result?,
            }
        };
        match result {
            Ok(Some(hover)) => {
                let text = serde_json::to_string(&hover.contents)?;
                assert!(text.contains("valore") && text.contains("i32") && !text.contains("u64"));
                return Ok(());
            }
            Ok(None) | Err(FeatureError::Client(ClientError::Server { code: -32801, .. })) => {
                // Solo una richiesta read-only attiva, con nuovo ID al giro
                // seguente: non polling idle né contratto di retry prodotto.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::other("hover nativo non disponibile dopo 32 richieste attive").into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "richiesta rust-analyzer reale opt-in: test-lsp-server-request-real"]
async fn create_progress_native_handler_token_end_hover_shutdown_reali() -> Esito {
    scenario77(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "routing rust-analyzer reale opt-in: test-lsp-routing-real"]
async fn next_event_routing_wire_reply_progress_hover_shutdown_reali() -> Esito {
    scenario77(true).await
}

async fn scenario77(automatic: bool) -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("LUME_RUST_ANALYZER richiesto, nessuno skip"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(
            io::Error::other("rust-analyzer deve essere un file assoluto esistente").into(),
        );
    }
    eprintln!(
        "REQUEST_REALE77:server_sha256={:x}",
        Sha256::digest(std::fs::read(&program)?)
    );
    let dir = Directory77::nuova()?;
    let root = &dir.0;
    for child in ["src", "home", "cargo", "config"] {
        std::fs::create_dir(root.join(child))?;
    }
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"verifica_request77\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    std::fs::write(root.join("src/lib.rs"), "pub mod città;\n")?;
    let path = root.join("src/città.rs");
    std::fs::write(&path, TESTO)?;
    let file_uri = uri77(&path)?;
    let root_uri = uri77(root)?;
    let mut env = vec![
        ("CARGO_NET_OFFLINE".into(), "true".into()),
        ("HOME".into(), root.join("home").into_os_string()),
        ("CARGO_HOME".into(), root.join("cargo").into_os_string()),
        (
            "XDG_CONFIG_HOME".into(),
            root.join("config").into_os_string(),
        ),
    ];
    if let Some(rustup_home) = std::env::var_os("RUSTUP_HOME").or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rustup").into_os_string())
    }) && PathBuf::from(&rustup_home).is_dir()
    {
        env.push(("RUSTUP_HOME".into(), rustup_home));
    }
    let command = ServerCommand {
        program,
        args: vec![],
        cwd: Some(root.clone()),
        env,
        source: CommandSource::Workspace,
    };
    let mut session =
        LspSession::spawn_with_trust(command, TransportConfig::default(), WorkspaceTrust::Trusted)?;
    let spec = InitializeSpec {
        root_uri: Some(root_uri.clone()),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: root_uri,
            name: "request77".into(),
        }]),
        initialization_options: Some(
            json!({"cargo":{"sysroot":null,"noDeps":true,"buildScripts":{"enable":false}},"procMacro":{"enable":false},"checkOnSave":false}),
        ),
        ..InitializeSpec::default()
    };
    let audit = automatic.then(|| session.transport().unwrap().observe_server_wire_for_test());
    let mut seen = Osservazione77::default();
    let scenario = limitato77(async {
        session.initialize(spec).await?;
        assert_eq!(session.state(), SessionState::Ready);
        while seen.fetching_end == 0 {
            consuma_evento77(&mut session, &mut seen, audit.as_ref()).await?;
        }
        assert_eq!(
            seen.fetching_create, 1,
            "create dello specifico token Fetching nativo"
        );
        assert_eq!(seen.fetching_begin, 1);
        assert!(
            seen.fetching_report > 0,
            "Report dello stesso token realmente osservato"
        );
        eprintln!("REQUEST_REALE77:native_create_fetching={};begin={};report={};end={};production_handler_enqueue=true", seen.fetching_create, seen.fetching_begin, seen.fetching_report, seen.fetching_end);
        session.open_document(file_uri.clone(), "rust".into(), TESTO.into())?;
        hover77(&mut session, &file_uri, &mut seen, audit.as_ref()).await?;
        let before = session.documents.reopen_documents();
        // Pressione deterministica sul budget reale del trasporto. Il server
        // rimane rust-analyzer: non sostituiamo writer o protocollo.
        let permit = session.transport()?.reserve_all_outbound_bytes_for_test().await?;
        {
            let unopened: Uri = "file:///cancelled-open77.rs".parse()?;
            let mut pending = Box::pin(session.open_document_wait(unopened, "rust".into(), TESTO.into()));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            drop(pending);
        }
        assert_eq!(session.documents.reopen_documents(), before);
        {
            let changed = TESTO.replace("i32", "i64");
            let mut pending = Box::pin(session.change_document_text_owned_wait(&file_uri, changed.clone()));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            drop(pending);
        }
        assert_eq!(session.documents.reopen_documents(), before);
        drop(permit);
        session.change_document_text_wait(&file_uri, TESTO).await?;
        let after = session.documents.reopen_documents();
        assert_eq!(after[0].text_document.version, before[0].text_document.version + 1);
        assert_eq!(after[0].text_document.text, TESTO);
        hover77(&mut session, &file_uri, &mut seen, audit.as_ref()).await?;
        eprintln!("REQUEST_REALE77:change_pending_drop_preserva_versione_byte_retry=confermato");
        session.close_document(&file_uri)?;
        assert_eq!(std::fs::read_to_string(&path)?, TESTO);
        Ok(())
    })
    .await;
    // Non abbandonare il graceful shutdown dentro una future cancellabile:
    // il processo viene consumato da shutdown. Deadline interna di produzione;
    // il test non rivendica una deadline finale end-to-end o std-thread join.
    let stop: Esito<std::process::ExitStatus> = if session.state() == SessionState::Ready {
        match std::panic::AssertUnwindSafe(session.shutdown(ATTESA))
            .catch_unwind()
            .await
        {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err(io::Error::other("panic nello shutdown: esito owner incerto").into()),
        }
    } else {
        Err(SessionError::NotReady {
            state: session.state(),
        }
        .into())
    };
    let state = session.state();
    drop(session);
    let status = match stop {
        Ok(status)
            if status.success() && status.code() == Some(0) && state == SessionState::Stopped =>
        {
            status
        }
        _ => {
            eprintln!(
                "REQUEST_REALE77:fixture_preservata={}; shutdown/owner incerto",
                dir.0.display()
            );
            return Err(io::Error::other(
                "shutdown reale non raccolto con successo; fixture conservata",
            )
            .into());
        }
    };
    if let Err(error) = scenario {
        eprintln!(
            "REQUEST_REALE77:fixture_preservata={}; scenario fallito dopo shutdown raccolto",
            dir.0.display()
        );
        return Err(error);
    }
    eprintln!(
        "REQUEST_REALE77:fetching_create={};begin={};report={};end={};hover=i32;direct_exit_success={}",
        seen.fetching_create,
        seen.fetching_begin,
        seen.fetching_report,
        seen.fetching_end,
        status.success()
    );
    if let Some(audit) = &audit {
        verifica_wire77(audit);
        eprintln!(
            "ROUTING_REALE77:sequenza_completa=next_event_native_create_written_reply_progress_hover_close_exitstatus"
        );
    }
    dir.cleanup()?;
    eprintln!(
        "REQUEST_REALE77:sequenza_completa=native_request_create_production_handler_same_token_begin_report_end_hover_close_exitstatus"
    );
    Ok(())
}
