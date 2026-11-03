//! Completamento reale nel consumer: menu, popup DTO, accetta/undo e replay.
//! No renderer/input nativo, std-thread join, SQL o risposte sintetiche.
use super::*;

const TESTO65: &str = "pub struct Dato { pub valore: i32 }\npub fn usa() { let dato = Dato { valore: 42 }; let _emoji = \"😀\"; dato.val; }\n";

async fn risposta_completamento(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
    request: u64,
) -> Esito<Option<Message>> {
    loop {
        let message = evento(stream).await?;
        if let Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Answer(id, answer))) =
            &message
            && *id == request
        {
            match &**answer {
                Ok(LspAnswer::Completion(Some(response))) => {
                    let items = match response {
                        lsp_types::CompletionResponse::Array(items) => items,
                        lsp_types::CompletionResponse::List(list) => &list.items,
                    };
                    if !items.iter().any(|i| i.label == "valore") {
                        return Err(
                            io::Error::other("campo assente nel completamento reale").into()
                        );
                    }
                    return Ok(Some(message));
                }
                Ok(LspAnswer::Completion(None)) => {}
                Err(e) if e.starts_with("server error -32801:") => {}
                _ => return Err(io::Error::other("risposta completamento reale inattesa").into()),
            }
            consegna(app, message)?;
            return Ok(None);
        }
        consegna(app, message)?;
    }
}

pub(super) async fn completamento_reale(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
) -> Esito<Message> {
    tokio::time::timeout(ATTESA, async {
        loop {
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::Complete));
            let request = app
                .completion_wait
                .as_ref()
                .ok_or_else(|| io::Error::other("menu non produce attesa completamento"))?
                .request;
            if let Some(message) = risposta_completamento(app, stream, request).await? {
                return Ok(message);
            }
            // Solo fixture attiva/read-only, non nuova policy prodotto.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "consumer completamento reale opt-in: test-app-lsp-completion-real"]
async fn recipe_completamento_accetta_undo_replay_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE65:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-completion65-città spazio-")
        .tempdir()?;
    let root = fixture.path();
    std::fs::create_dir(root.join("src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"completion65\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    std::fs::write(root.join("src/lib.rs"), "pub mod città;\n")?;
    let mut app = app(); // Percorso neutro senza store/profili SQL utente.
    let path = write_and_open(&mut app, &root.join("src"), "città.rs", TESTO65);
    let doc = app.active_doc().unwrap();
    let anchor = TESTO65[..TESTO65.rfind("val;").unwrap()].chars().count();
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor + 3));
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
    // Riusa il driver64 di recipe effettivamente prodotta dall'app, NON
    // uno spawn con initializer sostituito e hash uguale.
    let mut stream = recipe(&app);
    let scenario = limitato(
        async {
            while !app.lsp.only().unwrap().can_complete() {
                consegna(&mut app, evento(&mut stream).await?)?;
            }
            let version = app.workspace.get(doc).unwrap().version();
            let answer = completamento_reale(&mut app, &mut stream).await?;
            let replay = answer.clone(); // Clone di una risposta davvero ricevuta.
            consegna(&mut app, answer)?;
            let ui = app
                .completion
                .as_ref()
                .ok_or_else(|| io::Error::other("update non pubblica CompletionUi"))?;
            assert_eq!(ui.session.doc, doc);
            assert_eq!(ui.popup.anchor, anchor);
            let index = ui
                .popup
                .rows
                .iter()
                .position(|r| r.label == "valore")
                .unwrap();
            for _ in 0..index {
                let _ = app.update(Message::EditorIn(
                    crate::editor_groups::GroupId::Primary,
                    EditorCommand::PopupDown,
                ));
            }
            let _ = app.update(Message::EditorIn(
                crate::editor_groups::GroupId::Primary,
                EditorCommand::PopupAccept,
            ));
            assert!(app.completion.is_none());
            let document = app.workspace.get(doc).unwrap();
            assert_eq!(
                document.rope().to_string(),
                TESTO65.replace("dato.val;", "dato.valore;")
            );
            assert!(document.version() > version && document.is_dirty());
            assert_eq!(document.selection(), lume_core::Range::point(anchor + 6));
            assert_eq!(
                std::fs::read_to_string(&path)?,
                TESTO65,
                "accetta modifica buffer, non file"
            );
            let _ = app.update(Message::EditorIn(
                crate::editor_groups::GroupId::Primary,
                EditorCommand::Undo,
            ));
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                TESTO65,
                "un solo Undo annulla il completamento"
            );
            assert!(!app.workspace.get(doc).unwrap().is_dirty());
            app.workspace
                .get_mut(doc)
                .unwrap()
                .set_selection(lume_core::Range::point(anchor + 3));
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::Complete));
            let current = app.completion_wait.as_ref().unwrap().clone();
            consegna(&mut app, replay)?;
            assert_eq!(
                app.completion_wait.as_ref(),
                Some(&current),
                "replay ID già consumato non sottrae l'attesa successiva"
            );
            assert!(app.completion.is_none());
            // La domanda corrente riceve la propria risposta reale. Su startup
            // ContentModified/None, la fixture chiede di nuovo tramite il menu.
            let next = match risposta_completamento(&mut app, &mut stream, current.request).await? {
                Some(m) => m,
                None => completamento_reale(&mut app, &mut stream).await?,
            };
            consegna(&mut app, next)?;
            assert!(
                app.completion
                    .as_ref()
                    .unwrap()
                    .popup
                    .rows
                    .iter()
                    .any(|r| r.label == "valore")
            );
            // Distinto dall'Undo singolo sopra: il popup reale segue una
            // battuta e accetta fino al cursore DI ADESSO (Task10).
            let _ = app.update(Message::EditorIn(
                crate::editor_groups::GroupId::Primary,
                EditorCommand::Insert("o".into()),
            ));
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                TESTO65.replace("dato.val;", "dato.valo;")
            );
            assert_eq!(app.completion.as_ref().unwrap().popup.anchor, anchor);
            let index = app
                .completion
                .as_ref()
                .unwrap()
                .popup
                .rows
                .iter()
                .position(|r| r.label == "valore")
                .unwrap();
            for _ in 0..index {
                let _ = app.update(Message::EditorIn(
                    crate::editor_groups::GroupId::Primary,
                    EditorCommand::PopupDown,
                ));
            }
            let _ = app.update(Message::EditorIn(
                crate::editor_groups::GroupId::Primary,
                EditorCommand::PopupAccept,
            ));
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                TESTO65.replace("dato.val;", "dato.valore;")
            );
            assert_eq!(
                app.workspace.get(doc).unwrap().selection(),
                lume_core::Range::point(anchor + 6)
            );
            assert!(app.completion.is_none());
            assert_eq!(
                std::fs::read_to_string(&path)?,
                TESTO65,
                "anche Undo/replay/battuta/accetta lasciano il file originale"
            );
            Ok(())
        },
        "completion65 scenario: stop/raccolta saranno tentati",
    )
    .await;
    let stop = limitato(
        async {
            let _ = app.stop_lsp();
            let mut failure = None;
            loop {
                let message = evento(&mut stream).await?;
                let stopped = matches!(
                    &message,
                    Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Stopped))
                );
                if let Err(e) = consegna(&mut app, message) {
                    failure.get_or_insert(e);
                }
                if stopped {
                    break;
                }
            }
            assert!(app.lsp_spawns().is_empty());
            if let Some(e) = failure {
                return Err(e);
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(30), stream.next())
                    .await
                    .is_err()
            );
            Ok(())
        },
        "completion65 stop: processo/reaper incerti",
    )
    .await;
    drop(stream);
    if let Err(e) = stop {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE65:fixture_preservata={}; raccolta incerta",
            residue.display()
        );
        return Err(e);
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE65:sequenza_completa=menu_completion_popup_accept_undo_replay_stopped_drop"
    );
    Ok(())
}
