//! Consumer headless reale: recipe Iced, update, menu F7 e risposta native.
//! Non rendering GUI, ExitStatus del figlio, std-thread join o budget nativi.
use super::*;
use crate::app::lsp_subscription;
use futures::{FutureExt, StreamExt, stream::BoxStream};
use lume_lsp::{ActorEvent, CommandSource, LspAnswer, ServerCommand};
use std::{error::Error, io, path::PathBuf, time::Duration};

#[path = "native_git_blame70.rs"]
mod blame70;
#[path = "native_lsp_completion65.rs"]
mod completion65;
#[path = "native_lsp_definition68.rs"]
mod definition68;
#[path = "native_lsp_diagnostics66.rs"]
mod diagnostics66;
#[path = "native_lsp_progress75.rs"]
mod progress75;
#[path = "native_lsp_read_close74.rs"]
mod read_close74;
#[path = "native_lsp_read_focus79.rs"]
mod read_focus79;
#[path = "native_lsp_read_revision71.rs"]
mod read_revision71;
#[path = "native_lsp_read_root69.rs"]
mod read_root69;
#[path = "native_lsp_root67.rs"]
mod root67;
#[path = "native_lsp_save_as73.rs"]
mod save_as73;
#[path = "native_lsp_tracker72.rs"]
mod tracker72;

#[path = "native_lsp_formatting.rs"]
mod formatting;

#[path = "native_lsp_signature.rs"]
mod signature;

type Esito<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const ATTESA: Duration = Duration::from_secs(30);
const TESTO: &str = "pub struct Dato { pub valore: i32 }\npub fn usa() { let dato = Dato { valore: 42 }; let _emoji = \"😀\"; dato.valore; }\n";

fn recipe(app: &Lume) -> BoxStream<'static, Message> {
    recipe_prodotta(app).stream(futures::stream::empty().boxed())
}

fn recipe_prodotta(
    app: &Lume,
) -> Box<dyn iced_runtime::futures::subscription::Recipe<Output = Message>> {
    use iced_runtime::futures::subscription::{Hasher, Recipe, into_recipes};
    use std::hash::Hasher as _;
    let hash = |r: &dyn Recipe<Output = Message>| {
        let mut h = Hasher::default();
        r.hash(&mut h);
        h.finish()
    };
    // Identità strutturale per selezionare il ramo: NON autorità/prova GUI.
    let spawn = app.lsp_spawns().pop().unwrap();
    let expected = hash(&*into_recipes(lsp_subscription(spawn)).pop().unwrap());
    let mut matches: Vec<_> = into_recipes(app.subscription())
        .into_iter()
        .filter(|r| hash(&**r) == expected)
        .collect();
    assert_eq!(matches.len(), 1, "un solo ramo realmente prodotto dall'app");
    matches.pop().unwrap()
}

async fn limitato(future: impl std::future::Future<Output = Esito>, fase: &str) -> Esito {
    match tokio::time::timeout(ATTESA, std::panic::AssertUnwindSafe(future).catch_unwind()).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            Err(io::Error::other(format!("{fase}: panic; raccolta/residui da verificare")).into())
        }
        Err(_) => Err(io::Error::other(format!(
            "{fase}: deadline; raccolta/residui da verificare"
        ))
        .into()),
    }
}

async fn evento(stream: &mut BoxStream<'_, Message>) -> Esito<Message> {
    tokio::time::timeout(ATTESA, stream.next())
        .await?
        .ok_or_else(|| io::Error::other("EOF Iced prima dell'evento LSP").into())
}

fn consegna(app: &mut Lume, message: Message) -> Esito {
    match &message {
        Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Failed(_))) => {
            return Err(io::Error::other("server reale Failed; log grezzo non pubblicato").into());
        }
        Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Log(e)))
            if e.starts_with("commiato non riuscito:") =>
        {
            return Err(io::Error::other("commiato reale fallito; raccolta incerta").into());
        }
        _ => {}
    }
    // Solo questo ramo è pollato. Nessun Task iniziale, watcher, DB o PTY.
    let _ = app.update(message);
    Ok(())
}

async fn hover_reale(app: &mut Lume, stream: &mut BoxStream<'_, Message>) -> Esito<Message> {
    tokio::time::timeout(ATTESA, async {
        loop {
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::QuickDoc));
            let request = app
                .hover_wait
                .as_ref()
                .ok_or_else(|| io::Error::other("F7 non produce un'attesa hover"))?
                .request;
            loop {
                let message = evento(stream).await?;
                if let Message::Lsp(
                    _,
                    _,
                    crate::lsp_mode::LspEvent::From(ActorEvent::Answer(id, answer)),
                ) = &message
                    && *id == request
                {
                    match &**answer {
                        Ok(LspAnswer::Hover(Some(h))) => {
                            let text = serde_json::to_string(&h.contents)?;
                            if !text.contains("valore") || !text.contains("i32") {
                                return Err(io::Error::other(
                                    "hover reale non corrisponde al campo",
                                )
                                .into());
                            }
                            return Ok(message);
                        }
                        Ok(LspAnswer::Hover(None)) => {}
                        Err(e) if e.starts_with("server error -32801:") => {}
                        _ => return Err(io::Error::other("risposta hover reale inattesa").into()),
                    }
                    consegna(app, message)?;
                    break;
                }
                consegna(app, message)?;
            }
            // Solo fixture attiva: nuova richiesta readonly/nuovo ID dopo
            // None o ContentModified. NON policy di retry dell'app.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "consumer app reale opt-in: LUME_RUST_ANALYZER, test-app-lsp-real"]
async fn recipe_update_f7_revisione_stop_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE64:binario_sha256={}",
        crate::aggiornamento::esadecimale(&<sha2::Sha256 as sha2::Digest>::digest(std::fs::read(
            &program
        )?))
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-app64-città spazio-")
        .tempdir()?;
    let root = fixture.path();
    std::fs::create_dir(root.join("src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"app64\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    std::fs::write(root.join("src/lib.rs"), "pub mod città;\n")?;
    let mut app = app(); // new_without_store, NON new/store/vault entry.
    let path = write_and_open(&mut app, &root.join("src"), "città.rs", TESTO);
    let doc = app.active_doc().unwrap();
    let anchor = TESTO[..TESTO.rfind("valore;").unwrap()].chars().count();
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor + 1));
    app.project_root = Some(root.to_path_buf());
    let mut env = vec![
        ("CARGO_NET_OFFLINE".into(), "true".into()),
        ("HOME".into(), root.join("home").into_os_string()),
        ("CARGO_HOME".into(), root.join("cargo").into_os_string()),
        (
            "XDG_CONFIG_HOME".into(),
            root.join("config").into_os_string(),
        ),
    ];
    if let Some(home) = std::env::var_os("RUSTUP_HOME").or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rustup").into_os_string())
    }) {
        env.push(("RUSTUP_HOME".into(), home));
    }
    let spec = lume_language_servers::ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let mut lsp = crate::lsp_mode::LspWorkspace::new(spec, root.to_path_buf());
    lsp.command = Some(ServerCommand {
        program,
        args: vec![],
        cwd: Some(root.to_path_buf()),
        env,
        source: CommandSource::SystemPath,
    });
    lsp.state = crate::lsp_mode::LspState::Avvio;
    let _ = lsp.bump();
    app.lsp.insert(lsp);
    // InitializeSpec è QUELLO dell'app, senza modificare spawn/options per
    // ottenere un hash uguale con un payload diverso. Il catalogo spegne
    // build script/proc macro/checkOnSave. Sysroot metadata può fallire
    // offline; non scaricare né installare dipendenze per far passare.
    let mut stream = recipe(&app);
    let scenario = limitato(
        async {
            while !app.lsp.only().unwrap().can_hover() {
                consegna(&mut app, evento(&mut stream).await?)?;
            }
            assert_eq!(
                app.lsp.only().unwrap().opened.get(&doc),
                crate::lsp_uri::path_to_uri(&path).as_ref()
            );
            let answer = hover_reale(&mut app, &mut stream).await?;
            consegna(&mut app, answer)?;
            let shown = app
                .hover
                .as_ref()
                .ok_or_else(|| io::Error::other("update non pubblica HoverShown"))?;
            assert_eq!(shown.doc, doc);
            assert_eq!(shown.card.anchor, anchor);
            assert!(shown.card.lines.iter().any(|l| l.text.contains("i32")));
            // Trattiene una seconda risposta autentica, poi cambia la revisione
            // con il consumer editor: non sintetizza un Answer tardivo.
            let old = hover_reale(&mut app, &mut stream).await?;
            let before_version = app.workspace.get(doc).unwrap().version();
            let before_text = app.workspace.get(doc).unwrap().rope().to_string();
            let _ = app.update(Message::EditorIn(
                crate::editor_groups::GroupId::Primary,
                EditorCommand::Insert(" ".into()),
            ));
            assert!(app.workspace.get(doc).unwrap().version() > before_version);
            let mut expected = before_text;
            let byte = expected.char_indices().nth(anchor + 1).unwrap().0;
            expected.insert(byte, ' ');
            assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), expected);
            consegna(&mut app, old)?;
            assert!(
                app.hover.is_none(),
                "risposta della vecchia revisione scartata"
            );
            Ok(())
        },
        "scenario: stop/raccolta saranno tentati",
    )
    .await;
    // Colleziona Stop anche su Err dello scenario. Lo stream Iced non espone
    // il native ExitStatus/std-thread join e dopo l'inoltro resta Pending.
    let stop = limitato(
        async {
            let _ = app.stop_lsp();
            let mut failure = None;
            loop {
                let m = evento(&mut stream).await?;
                let stopped = matches!(
                    &m,
                    Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Stopped))
                );
                if let Err(error) = consegna(&mut app, m) {
                    // Failed/commiato error possono precedere Stopped: non
                    // abbandonare il drain appena compare il primo errore.
                    failure.get_or_insert(error);
                }
                if stopped {
                    break;
                }
            }
            assert!(app.lsp_spawns().is_empty(), "update rimuove il ramo attivo");
            if let Some(error) = failure {
                return Err(error);
            }
            // Dà anche al join dell'inoltro/attore occasione di essere pollato.
            // No EOF osservato per 30ms: non promette std-thread/native join.
            assert!(
                tokio::time::timeout(Duration::from_millis(30), stream.next())
                    .await
                    .is_err(),
                "dopo Stopped il contenitore resta Pending, non EOF/rinascita"
            );
            Ok(())
        },
        "stop: processo/reaper incerti, nessun rollback",
    )
    .await;
    drop(stream);
    if let Err(error) = stop {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE64:fixture_preservata={}; raccolta incerta",
            residue.display()
        );
        return Err(error);
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE64:sequenza_completa=recipe_update_menu_hover_revision_stopped_drop_stream"
    );
    Ok(())
}

#[path = "native_lsp_multicursore.rs"]
mod multicursore;
