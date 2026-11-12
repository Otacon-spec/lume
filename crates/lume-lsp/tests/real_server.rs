//! Test esplicito su rust-analyzer reale già installato, non fake server.
//! Non si installa nulla: LUME_RUST_ANALYZER deve indicare il binario assoluto.
//! Fixture Cargo locale offline; sysroot, dipendenze, build script e proc-macro
//! disabilitati. Non prova sandbox OS, GUI/MCP/native Windows o std-thread join.
use lsp_types::{
    CompletionResponse, GotoDefinitionResponse, NumberOrString, Position, ProgressParamsValue, Uri,
    WorkDoneProgress, WorkspaceFolder,
};
use lume_lsp::{
    ClientError, CommandSource, FeatureError, InitializeSpec, LspSession, ServerCommand,
    SessionEvent, SessionState, TransportConfig, WorkspaceTrust,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    future::Future,
    io,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

type Esito<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const ATTESA: Duration = Duration::from_secs(20);
const TESTO: &str = "pub struct Saluto { pub valore: i32 }\npub fn somma(a: i32, b: i32) -> i32 { a + b }\npub fn prova() {\n    let saluto = Saluto { valore: 42 };\n    let _risultato = somma(1, 2);\n    let _emoji = \"😀\"; saluto.valore;\n}\n";

fn richiedi(condizione: bool, messaggio: &str) -> Esito {
    if condizione {
        Ok(())
    } else {
        Err(io::Error::other(messaggio).into())
    }
}
struct DirectoryPosseduta(Option<PathBuf>);
impl DirectoryPosseduta {
    fn nuova() -> Esito<Self> {
        let nome = format!(
            "lume-lsp-reale-{}-{}-città",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let path = std::env::temp_dir().join(nome);
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        builder.create(&path)?; // nessun riuso/rimozione di una fixture altrui
        Ok(Self(Some(path)))
    }
    fn path(&self) -> Esito<&std::path::Path> {
        self.0
            .as_deref()
            .ok_or_else(|| io::Error::other("directory fixture già raccolta").into())
    }
    fn cleanup(mut self) -> Esito {
        if let Some(path) = self.0.take() {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}
impl Drop for DirectoryPosseduta {
    fn drop(&mut self) {
        if let Some(path) = self.0.take()
            && let Err(error) = std::fs::remove_dir_all(path)
        {
            eprintln!("cleanup fixture LSP fallito: {error}; residui da verificare");
        }
    }
}
fn uri(path: &std::path::Path) -> Esito<Uri> {
    richiedi(path.is_absolute(), "fixture URI non assoluta")?;
    let testo = path
        .to_str()
        .ok_or_else(|| io::Error::other("fixture URI non UTF8"))?;
    let mut out = String::from("file://");
    #[cfg(windows)]
    out.push('/');
    for byte in testo.bytes() {
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
fn posizione(testo: &str, marcatore: &str, avanti: usize) -> Esito<Position> {
    let offset = testo
        .rfind(marcatore)
        .ok_or_else(|| io::Error::other("marcatore fixture assente"))?
        + avanti;
    let prima = testo
        .get(..offset)
        .ok_or_else(|| io::Error::other("posizione fixture non sul confine UTF8"))?;
    let riga = u32::try_from(prima.bytes().filter(|&b| b == b'\n').count())?;
    let colonna = u32::try_from(
        prima
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count(),
    )?;
    Ok(Position::new(riga, colonna))
}
async fn richiesta<T, F>(sessione: &mut LspSession, future: F) -> Result<T, FeatureError>
where
    F: Future<Output = Result<T, FeatureError>>,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            risultato=&mut future=>return risultato,
            evento=sessione.next_event()=>match evento {
                Some(SessionEvent::Disconnected(error)|SessionEvent::Exited(error))=>return Err(error.into()),
                Some(_)=>{},
                None=>return Err(ClientError::Disconnected.into()),
            }
        }
    }
}
async fn richiesta_disponibile<T, F, Fut>(sessione: &mut LspSession, mut crea: F) -> Esito<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<T>, FeatureError>>,
{
    let scadenza = tokio::time::Instant::now() + ATTESA;
    loop {
        let ultimo = match richiesta(sessione, crea()).await {
            Ok(Some(risultato)) => return Ok(risultato),
            Ok(None) => "risposta nulla dal server",
            Err(FeatureError::Client(ClientError::Server { code: -32801, .. })) => {
                "richiesta invalidata mentre il server caricava il workspace"
            }
            Err(error) => return Err(error.into()),
        };
        if tokio::time::Instant::now() >= scadenza {
            return Err(io::Error::new(io::ErrorKind::TimedOut, ultimo).into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn attendi_workspace(sessione: &mut LspSession) -> Esito {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match sessione.next_event().await {
                Some(SessionEvent::Progress(progress))
                    if progress.token == NumberOrString::String("rustAnalyzer/Fetching".into()) =>
                {
                    if matches!(
                        progress.value,
                        ProgressParamsValue::WorkDone(WorkDoneProgress::End(_))
                    ) {
                        eprintln!("LSP_REALE:workspace_metadata=caricato");
                        return Ok(());
                    }
                }
                Some(SessionEvent::Disconnected(error) | SessionEvent::Exited(error)) => {
                    return Err(error.into());
                }
                Some(_) => {}
                None => return Err(io::Error::other("EOF durante caricamento workspace").into()),
            }
        }
    })
    .await?
}
async fn scenario(sessione: &mut LspSession, uri: &Uri) -> Esito {
    let feature = sessione.feature_client()?;
    let disponibili = feature.availability();
    richiedi(
        disponibili.hover
            && disponibili.completion
            && disponibili.definition
            && disponibili.signature_help,
        "server reale senza capability necessarie",
    )?;
    sessione.open_document(uri.clone(), "rust".into(), TESTO.into())?;
    let posizione_hover = posizione(TESTO, "valore;", 1)?;
    let hover_params: lsp_types::HoverParams =
        serde_json::from_value(json!({"textDocument":{"uri":uri},"position":posizione_hover}))?;
    let hover = richiesta_disponibile(sessione, || {
        let feature = feature.clone();
        let params = hover_params.clone();
        async move { feature.hover(params, ATTESA).await }
    })
    .await?;
    let hover_json = serde_json::to_string(&hover.contents)?;
    richiedi(
        hover_json.contains("valore") && hover_json.contains("i32"),
        "hover reale non corrisponde al campo i32",
    )?;
    eprintln!(
        "LSP_REALE:hover_i32=confermato,utf16_colonna={}",
        posizione_hover.character
    );
    let completion_params: lsp_types::CompletionParams = serde_json::from_value(
        json!({"textDocument":{"uri":uri},"position":posizione(TESTO,"valore;",3)?}),
    )?;
    let risposta = richiesta_disponibile(sessione, || {
        let feature = feature.clone();
        let params = completion_params.clone();
        async move { feature.completion(params, ATTESA).await }
    })
    .await?;
    let voci = match risposta {
        CompletionResponse::Array(voci) => voci,
        CompletionResponse::List(lista) => lista.items,
    };
    richiedi(
        voci.iter().any(|voce| voce.label == "valore"),
        "campo valore non presente nella completion reale",
    )?;
    eprintln!("LSP_REALE:completion_valore=confermato,voci={}", voci.len());
    let signature_position = posizione(TESTO, "somma(1", 6)?;
    let signature_params = lsp_types::SignatureHelpParams {
        text_document_position_params: lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
            position: signature_position,
        },
        work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        context: Some(lsp_types::SignatureHelpContext {
            trigger_kind: lsp_types::SignatureHelpTriggerKind::INVOKED,
            trigger_character: None,
            is_retrigger: false,
            active_signature_help: None,
        }),
    };
    let signature = richiesta_disponibile(sessione, || {
        let feature = feature.clone();
        let params = signature_params.clone();
        async move { feature.signature_help(params, ATTESA).await }
    })
    .await?;
    richiedi(
        signature
            .signatures
            .iter()
            .any(|item| item.label.contains("somma"))
            && signature.active_parameter == Some(0),
        "signature help reale non identifica somma e il primo parametro",
    )?;
    eprintln!("LSP_REALE:signature_help_somma=confermato");
    let definition_params: lsp_types::GotoDefinitionParams =
        serde_json::from_value(json!({"textDocument":{"uri":uri},"position":posizione_hover}))?;
    let definizione = richiesta_disponibile(sessione, || {
        let feature = feature.clone();
        let params = definition_params.clone();
        async move { feature.definition(params, ATTESA).await }
    })
    .await?;
    let corretta = match definizione {
        GotoDefinitionResponse::Scalar(location) => {
            location.uri == *uri && location.range.start.line == 0
        }
        GotoDefinitionResponse::Array(locations) => locations
            .iter()
            .any(|l| l.uri == *uri && l.range.start.line == 0),
        GotoDefinitionResponse::Link(links) => links
            .iter()
            .any(|l| l.target_uri == *uri && l.target_selection_range.start.line == 0),
    };
    richiedi(
        corretta,
        "definition reale non punta alla dichiarazione locale",
    )?;
    eprintln!("LSP_REALE:definition_locale=confermato");
    // didChange reale: non una risposta iniettata nel modello.
    let nuovo = TESTO.replace("valore", "altro").replace("i32", "u64");
    sessione.change_document_text(uri, &nuovo)?;
    let hover_params: lsp_types::HoverParams = serde_json::from_value(
        json!({"textDocument":{"uri":uri},"position":posizione(&nuovo,"altro;",1)?}),
    )?;
    let hover = richiesta_disponibile(sessione, || {
        let feature = feature.clone();
        let params = hover_params.clone();
        async move { feature.hover(params, ATTESA).await }
    })
    .await?;
    let hover_json = serde_json::to_string(&hover.contents)?;
    richiedi(
        hover_json.contains("altro") && hover_json.contains("u64") && !hover_json.contains("i32"),
        "hover reale non aggiornato alla versione2",
    )?;
    eprintln!("LSP_REALE:hover_u64_versione2=confermato");
    sessione.close_document(uri)?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "server reale opt-in: LUME_RUST_ANALYZER assoluto; runner test-lsp-real"]
async fn initialize_hover_completion_definition_change_close_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::other("manca LUME_RUST_ANALYZER assoluto: nessun server avviato")
        })?;
    richiedi(
        program.is_absolute() && program.is_file(),
        "LUME_RUST_ANALYZER non è un file assoluto",
    )?;
    eprintln!(
        "LSP_REALE:binario_sha256={}",
        lume_lsp::hex::hex(&Sha256::digest(std::fs::read(&program)?))
    );
    let dir = DirectoryPosseduta::nuova()?;
    let sorgenti = dir.path()?.join("src");
    std::fs::create_dir(&sorgenti)?;
    let path = sorgenti.join("città.rs");
    std::fs::write(
        dir.path()?.join("Cargo.toml"),
        "[package]\nname = \"verifica_lume\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    std::fs::write(sorgenti.join("lib.rs"), "pub mod città;\n")?;
    std::fs::write(&path, TESTO)?;
    let file_uri = uri(&path)?;
    let log_path = dir.path()?.join("rust-analyzer.log");
    let mut env = vec![
        ("CARGO_NET_OFFLINE".into(), "true".into()),
        ("HOME".into(), dir.path()?.join("home").into_os_string()),
        (
            "CARGO_HOME".into(),
            dir.path()?.join("cargo").into_os_string(),
        ),
        (
            "XDG_CONFIG_HOME".into(),
            dir.path()?.join("config").into_os_string(),
        ),
    ];
    // HOME/CARGO_HOME restano privati, ma rust-analyzer deve poter scoprire
    // il sysroot del toolchain che ha fornito il binario reale.
    if let Some(rustup_home) = std::env::var_os("RUSTUP_HOME").or_else(|| {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup").into_os_string())
    }) && PathBuf::from(&rustup_home).is_dir()
    {
        env.push(("RUSTUP_HOME".into(), rustup_home));
    }
    let command = ServerCommand {
        program,
        args: vec![
            "-v".into(),
            "-v".into(),
            "--log-file".into(),
            log_path.as_os_str().to_owned(),
            "--no-log-buffering".into(),
        ],
        cwd: Some(dir.path()?.to_owned()),
        env,
        source: CommandSource::Workspace,
    };
    // Trust SOLO della fixture posseduta; non progetto dell'utente o grants MCP.
    let mut sessione =
        LspSession::spawn_with_trust(command, TransportConfig::default(), WorkspaceTrust::Trusted)?;
    let root_uri = uri(dir.path()?)?;
    let spec = InitializeSpec {
        root_uri: Some(root_uri.clone()),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: root_uri,
            name: "verifica_lume".into(),
        }]),
        initialization_options: Some(json!({
            "cargo": {
                "sysroot": null,
                "noDeps": true,
                "buildScripts": { "enable": false }
            },
            "procMacro": { "enable": false },
            "checkOnSave": false
        })),
        ..InitializeSpec::default()
    };
    let risultato = async {
        let negotiated = sessione.initialize(spec).await?;
        eprintln!(
            "LSP_REALE:sync={:?},hover={}",
            negotiated.sync_kind,
            sessione.feature_availability().hover
        );
        eprintln!("LSP_REALE:initialize=confermato");
        attendi_workspace(&mut sessione).await?;
        scenario(&mut sessione, &file_uri).await
    }
    .await;
    if risultato.is_err()
        && let Ok(log) = std::fs::read_to_string(&log_path)
    {
        eprintln!(
            "LSP_REALE:server_log_non_pubblicato,byte={},sha256={}",
            log.len(),
            lume_lsp::hex::hex(&Sha256::digest(log.as_bytes()))
        );
    }
    if sessione.state() != SessionState::Ready {
        eprintln!(
            "cleanup non graceful: stato sessione {:?}; Drop non equivale a join",
            sessione.state()
        );
        return Err(risultato.err().unwrap_or_else(|| {
            io::Error::other("sessione non Ready: shutdown non raccolto da questa prova").into()
        }));
    }
    let status = sessione.shutdown(ATTESA).await?;
    eprintln!(
        "LSP_REALE:shutdown_status={status},stato={:?}",
        sessione.state()
    );
    richiedi(
        status.success() && sessione.state() == SessionState::Stopped,
        "server reale shutdown non riuscito",
    )?;
    risultato?;
    dir.cleanup()?;
    eprintln!(
        "LSP_REALE:sequenza_completa=initialize_hover_completion_signature_help_definition_change_close_shutdown"
    );
    Ok(())
}
