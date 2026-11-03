//! Attore e mailbox produzione contro rust-analyzer reale, opt-in.
//! Non UI, né prova std-thread join/containment/native Windows; nessun fake.
use lsp_types::{
    CompletionResponse, GotoDefinitionResponse, NumberOrString, Position, ProgressParamsValue, Uri,
    WorkDoneProgress, WorkspaceFolder,
};
use lume_lsp::{
    ActorEvent, CommandSource, InitializeSpec, LspAnswer, LspCommand, LspCommandMailbox,
    LspRequest, ServerCommand, actor,
};
use serde_json::json;
use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

type Esito<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const ATTESA: Duration = Duration::from_secs(30);
const TESTO: &str = "pub struct Dato { pub valore: i32 }\npub fn somma(a: i32, b: i32) -> i32 { a + b }\npub fn usa() { let dato = Dato { valore: 42 }; let _emoji = \"😀\"; dato.valore; somma(1, 2); }\n";
const TESTO_PRIMA_DEL_TRIGGER: &str = "pub struct Dato { pub valore: i32 }\npub fn somma(a: i32, b: i32) -> i32 { a + b }\npub fn usa() { let dato = Dato { valore: 42 }; let _emoji = \"😀\"; dato.valore; somma1, 2); }\n";
fn verifica(condizione: bool, testo: &str) -> Esito {
    if condizione {
        Ok(())
    } else {
        Err(io::Error::other(testo).into())
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn nuova() -> Esito<Self> {
        let p = std::env::temp_dir().join(format!(
            "lume-actor63-{}-{}-città spazio",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        builder.create(&p)?;
        Ok(Self(p))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            eprintln!("cleanup fixture attore fallito: {e}; residui da verificare");
        }
    }
}
fn uri(path: &Path) -> Esito<Uri> {
    verifica(path.is_absolute(), "fixture non assoluta")?;
    let testo = path
        .to_str()
        .ok_or_else(|| io::Error::other("fixture non UTF8"))?;
    let mut valore = String::from("file://");
    #[cfg(windows)]
    valore.push('/');
    for b in testo.bytes() {
        #[cfg(windows)]
        if b == b'\\' {
            valore.push('/');
            continue;
        }
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'-' | b'.' | b'_' | b'~') {
            valore.push(char::from(b));
        } else {
            valore.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(valore.parse()?)
}
fn posizione(testo: &str, marcatore: &str, avanti: usize) -> Esito<Position> {
    let offset = testo
        .rfind(marcatore)
        .ok_or_else(|| io::Error::other("marcatore assente"))?
        + avanti;
    let prima = testo
        .get(..offset)
        .ok_or_else(|| io::Error::other("offset non UTF8"))?;
    Ok(Position::new(
        u32::try_from(prima.bytes().filter(|&b| b == b'\n').count())?,
        u32::try_from(
            prima
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .encode_utf16()
                .count(),
        )?,
    ))
}
fn invia(mailbox: &LspCommandMailbox, comando: LspCommand) -> Esito {
    mailbox
        .send(comando)
        .map_err(|e| io::Error::other(format!("mailbox non ammette il comando: {e:?}")).into())
}
async fn evento(events: &mut UnboundedReceiver<ActorEvent>) -> Esito<ActorEvent> {
    tokio::time::timeout(ATTESA, events.recv())
        .await?
        .ok_or_else(|| io::Error::other("EOF attore prima dell'evento atteso").into())
}
async fn ready(events: &mut UnboundedReceiver<ActorEvent>) -> Esito {
    tokio::time::timeout(ATTESA, async {
        let (mut initialized, mut loaded) = (false, false);
        while !(initialized && loaded) {
            match evento(events).await? {
                ActorEvent::Initialized(info) => {
                    verifica(
                        lume_lsp::FeatureAvailability::from_capabilities(&info.capabilities.server)
                            .hover,
                        "hover non negoziato dall'attore reale",
                    )?;
                    initialized = true;
                }
                ActorEvent::Progress(p)
                    if p.token == NumberOrString::String("rustAnalyzer/Fetching".into()) =>
                {
                    loaded = matches!(
                        p.value,
                        ProgressParamsValue::WorkDone(WorkDoneProgress::End(_))
                    );
                }
                ActorEvent::Failed(e) => {
                    return Err(io::Error::other(format!("attore reale failed: {e}")).into());
                }
                ActorEvent::Stopped => {
                    return Err(io::Error::other("attore reale fermo prima di Ready").into());
                }
                _ => {}
            }
        }
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    })
    .await?
}
async fn risposta(
    events: &mut UnboundedReceiver<ActorEvent>,
    id: u64,
) -> Esito<Result<LspAnswer, String>> {
    tokio::time::timeout(ATTESA, async {
        loop {
            match evento(events).await? {
                ActorEvent::Answer(actual, result) if actual == id => {
                    return Ok(*result);
                }
                ActorEvent::Answer(actual, _) => {
                    return Err(
                        io::Error::other(format!("risposta estranea {actual} attesa{id}")).into(),
                    );
                }
                ActorEvent::Failed(e) => {
                    return Err(io::Error::other(format!("attore failed: {e}")).into());
                }
                ActorEvent::Stopped => {
                    return Err(io::Error::other("Stopped prima della risposta").into());
                }
                _ => {}
            }
        }
    })
    .await?
}
async fn ask(
    mailbox: &LspCommandMailbox,
    events: &mut UnboundedReceiver<ActorEvent>,
    id: u64,
    request: LspRequest,
) -> Esito<LspAnswer> {
    // Retry SOLO nella fixture attiva, su richieste LSP di lettura. L'attore
    // pubblico perde il codice strutturato in String: non qualificare questa
    // classificazione test come policy robusta di retry/consenso dell'app.
    tokio::time::timeout(ATTESA, async {
        let mut actual = id;
        loop {
            invia(
                mailbox,
                LspCommand::Ask {
                    id: actual,
                    request: request.clone(),
                },
            )?;
            match risposta(events, actual).await? {
                Ok(
                    LspAnswer::Hover(None)
                    | LspAnswer::Completion(None)
                    | LspAnswer::Definition(None),
                ) => {}
                Ok(answer) => return Ok(answer),
                Err(error) if error.starts_with("server error -32801:") => {
                    eprintln!("ACTOR_REALE63:content_modified_reale,id={actual}");
                }
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "risposta reale {actual} fallita: {error}"
                    ))
                    .into());
                }
            }
            actual = actual
                .checked_add(1000)
                .ok_or_else(|| io::Error::other("id fixture esauriti"))?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}
async fn scenario(
    mailbox: &LspCommandMailbox,
    events: &mut UnboundedReceiver<ActorEvent>,
    file: &Uri,
) -> Esito {
    ready(events).await?;
    eprintln!("ACTOR_REALE63:initialized_workspace=confermato");
    invia(
        mailbox,
        LspCommand::Open {
            uri: file.clone(),
            language: "rust".into(),
            text: TESTO_PRIMA_DEL_TRIGGER.into(),
        },
    )?;
    let pos = posizione(TESTO, "valore;", 1)?;
    let hover = ask(
        mailbox,
        events,
        101,
        LspRequest::Hover {
            uri: file.clone(),
            position: pos,
        },
    )
    .await?;
    match hover {
        LspAnswer::Hover(Some(h)) => {
            let t = serde_json::to_string(&h.contents)?;
            verifica(
                t.contains("valore") && t.contains("i32"),
                "hover attore non corrisponde al campo",
            )?;
        }
        _ => return Err(io::Error::other("hover attore reale assente").into()),
    }
    eprintln!(
        "ACTOR_REALE63:hover_i32_utf16=confermato,col={}",
        pos.character
    );
    invia(
        mailbox,
        LspCommand::ChangeSnapshot {
            uri: file.clone(),
            text: ropey::Rope::from_str(TESTO),
        },
    )?;
    let signature = ask(
        mailbox,
        events,
        105,
        LspRequest::SignatureHelp {
            uri: file.clone(),
            position: posizione(TESTO, "somma(", 6)?,
            trigger_character: Some("(".into()),
        },
    )
    .await?;
    let signature = match signature {
        LspAnswer::SignatureHelp(Some(signature)) => signature,
        _ => return Err(io::Error::other("signature help attore assente").into()),
    };
    let firma = signature
        .signatures
        .first()
        .ok_or_else(|| io::Error::other("firma attore vuota"))?;
    verifica(
        firma.label.contains("somma") && firma.parameters.as_ref().is_some_and(|p| p.len() == 2),
        "signature help non descrive i due parametri",
    )?;
    eprintln!("ACTOR_REALE63:signature_help_somma=confermato");
    let completion = ask(
        mailbox,
        events,
        102,
        LspRequest::Completion {
            uri: file.clone(),
            position: posizione(TESTO, "valore;", 3)?,
        },
    )
    .await?;
    let items = match completion {
        LspAnswer::Completion(Some(CompletionResponse::Array(items))) => items,
        LspAnswer::Completion(Some(CompletionResponse::List(list))) => list.items,
        _ => return Err(io::Error::other("completion attore assente").into()),
    };
    verifica(
        items.iter().any(|i| i.label == "valore"),
        "campo non presente in completion attore",
    )?;
    let definition = ask(
        mailbox,
        events,
        103,
        LspRequest::Definition {
            uri: file.clone(),
            position: pos,
        },
    )
    .await?;
    let corretta = match definition {
        LspAnswer::Definition(Some(GotoDefinitionResponse::Scalar(l))) => {
            l.uri == *file && l.range.start.line == 0
        }
        LspAnswer::Definition(Some(GotoDefinitionResponse::Array(list))) => list
            .iter()
            .any(|l| l.uri == *file && l.range.start.line == 0),
        LspAnswer::Definition(Some(GotoDefinitionResponse::Link(list))) => list
            .iter()
            .any(|l| l.target_uri == *file && l.target_selection_range.start.line == 0),
        _ => false,
    };
    verifica(corretta, "definition attore non punta al modulo locale")?;
    eprintln!("ACTOR_REALE63:completion_definition=confermate");
    let nuovo = TESTO.replace("valore", "altro").replace("i32", "u64");
    let mut snapshot = ropey::Rope::from_str(&nuovo);
    invia(
        mailbox,
        LspCommand::ChangeSnapshot {
            uri: file.clone(),
            text: snapshot.clone(),
        },
    )?;
    // La sorgente cambia dopo l'ammissione, ma Ask deve osservare i byte
    // u64 dello snapshot accodato, non questa revisione locale i16.
    snapshot.remove(..);
    snapshot.insert(0, &nuovo.replace("u64", "i16"));
    let hover = ask(
        mailbox,
        events,
        104,
        LspRequest::Hover {
            uri: file.clone(),
            position: posizione(&nuovo, "altro;", 1)?,
        },
    )
    .await?;
    match hover {
        LspAnswer::Hover(Some(h)) => {
            let t = serde_json::to_string(&h.contents)?;
            verifica(
                t.contains("altro") && t.contains("u64") && !t.contains("i32"),
                "Change/Ask non applica nuova versione",
            )?;
        }
        _ => return Err(io::Error::other("hover dopo Change assente").into()),
    }
    invia(mailbox, LspCommand::Close { uri: file.clone() })?;
    eprintln!("ACTOR_REALE63:snapshot_immutabile_ask_u64=confermato");
    eprintln!("ACTOR_REALE63:change_close=confermati");
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "attore reale opt-in LUME_RUST_ANALYZER, runner test-lsp-actor-real"]
async fn mailbox_hover_completion_definition_change_last_sender_drop_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("manca binario reale esplicito"))?;
    verifica(
        program.is_absolute() && program.is_file(),
        "binario reale non assoluto/file",
    )?;
    eprintln!(
        "ACTOR_REALE63:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = Fixture::nuova()?;
    let src = fixture.0.join("src");
    std::fs::create_dir(&src)?;
    std::fs::write(
        fixture.0.join("Cargo.toml"),
        "[package]\nname = \"actor63\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    std::fs::write(src.join("lib.rs"), "pub mod città;\n")?;
    let path = src.join("città.rs");
    std::fs::write(&path, TESTO)?;
    let file = uri(&path)?;
    let root = uri(&fixture.0)?;
    let mut env = vec![
        ("CARGO_NET_OFFLINE".into(), "true".into()),
        ("HOME".into(), fixture.0.join("home").into_os_string()),
        (
            "CARGO_HOME".into(),
            fixture.0.join("cargo").into_os_string(),
        ),
        (
            "XDG_CONFIG_HOME".into(),
            fixture.0.join("config").into_os_string(),
        ),
    ];
    if let Some(home) = std::env::var_os("RUSTUP_HOME").or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rustup").into_os_string())
    }) {
        env.push(("RUSTUP_HOME".into(), home));
    }
    // Binario locale esplicito della toolchain/PATH; NON un comando Workspace
    // riclassificato per aggirare trust. Non autorizza progetti dell'utente.
    let command = ServerCommand {
        program,
        args: vec![],
        cwd: Some(fixture.0.clone()),
        env,
        source: CommandSource::SystemPath,
    };
    let spec = InitializeSpec {
        root_uri: Some(root.clone()),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: root,
            name: "actor63".into(),
        }]),
        initialization_options: Some(
            json!({"cargo":{"buildScripts":{"enable":false}},"procMacro":{"enable":false},"checkOnSave":false}),
        ),
        ..InitializeSpec::default()
    };
    let (mailbox, rx) = LspCommandMailbox::channel();
    let (tx, mut events) = unbounded_channel();
    let mut owner = tokio::spawn(actor::run(command, spec, rx, tx));
    let risultato = scenario(&mailbox, &mut events, &file).await;
    // La chiusura dell'ultimo sender è il caso reale di drop della recipe:
    // deve far eseguire shutdown protocollo e raccolta senza un comando finale.
    drop(mailbox);
    let raccolta = tokio::time::timeout(ATTESA, async {
        let mut stopped = false;
        let mut process_exit = None;
        while let Some(event) = events.recv().await {
            match event {
                ActorEvent::Stopped => stopped = true,
                ActorEvent::ProcessExited { success, code } => {
                    process_exit = Some((success, code));
                    if !success || code != Some(0) {
                        return Err(io::Error::other(format!(
                            "uscita native LSP non pulita: success={success}, code={code:?}"
                        ))
                        .into());
                    }
                }
                ActorEvent::Failed(e) => {
                    return Err(io::Error::other(format!("cleanup attore failed: {e}")).into());
                }
                ActorEvent::Log(e) if e.starts_with("commiato non riuscito:") => {
                    return Err(io::Error::other(
                        "shutdown attore non riuscito, log non pubblicato",
                    )
                    .into());
                }
                _ => {}
            }
        }
        verifica(stopped, "EOF senza Stopped attore")?;
        verifica(
            process_exit == Some((true, Some(0))),
            "EOF senza ExitStatus nativo pulito (0)",
        )
    })
    .await;
    // Collect anche su esito errore. Abort su timeout è una raccolta Task,
    // NON prova di raccolta nativa del server/reaper o rollback.
    let join = tokio::time::timeout(ATTESA, &mut owner).await;
    match join {
        Ok(value) => value?,
        Err(error) => {
            owner.abort();
            let actual = owner.await;
            eprintln!(
                "Task attore timeout: {error}; raccolta abort={actual:?}; server/reaper da verificare"
            );
            return Err(error.into());
        }
    }
    raccolta??;
    risultato?;
    eprintln!(
        "ACTOR_REALE63:sequenza_completa=mailbox_hover_completion_signature_help_definition_change_close_last_sender_drop_exit0_stopped_owner_join"
    );
    Ok(())
}
