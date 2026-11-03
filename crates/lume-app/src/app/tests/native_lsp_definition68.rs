//! Definizione reale → apertura asincrona/UTF16; risposta tardiva dopo edit.
//! Consumer headless, non renderer/input OS/whole-reader lifecycle/budget.
use super::root67::{Collection67, accensione, raccolta67};
use super::*;
use lume_ui::editor::EditorCommand;
pub(super) const SOURCE: &str =
    "pub mod città;\npub fn usa() { let _emoji = \"😀\"; let _valore = città::dato(); }\n";
pub(super) const TARGET: &str = "/* 😀 */ pub fn dato() -> u64 { 42 }\n";

fn target_native(message: &Message, target: &std::path::Path) -> Esito<lsp_types::Position> {
    let Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Answer(_, answer))) =
        message
    else {
        return Err(io::Error::other("non è un Answer realmente ricevuto").into());
    };
    let Ok(LspAnswer::Definition(Some(response))) = &**answer else {
        return Err(io::Error::other("definizione reale assente").into());
    };
    // Oracle DTO indipendente da definition_targets, che viene chiamato poi
    // soltanto dal consumer produzione. Nessun Location costruito qui.
    let (uri, range) = match response {
        lsp_types::GotoDefinitionResponse::Scalar(location) => (&location.uri, &location.range),
        lsp_types::GotoDefinitionResponse::Array(locations) => {
            if locations.len() != 1 {
                return Err(io::Error::other("mete native non univoche").into());
            }
            (&locations[0].uri, &locations[0].range)
        }
        lsp_types::GotoDefinitionResponse::Link(links) => {
            if links.len() != 1 {
                return Err(io::Error::other("link nativi non univoci").into());
            }
            (&links[0].target_uri, &links[0].target_selection_range)
        }
    };
    assert_eq!(Some(uri), crate::lsp_uri::path_to_uri(target).as_ref());
    assert!(uri.as_str().ends_with("/citt%C3%A0.rs"));
    let prefix = &TARGET[..TARGET.find("dato").unwrap()];
    assert_eq!(
        range.start,
        lsp_types::Position::new(0, prefix.encode_utf16().count() as u32)
    );
    Ok(range.start)
}

pub(super) async fn definizione_reale(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
    target: &std::path::Path,
) -> Esito<Message> {
    tokio::time::timeout(ATTESA, async {
        loop {
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::GotoDefinition));
            let (_, request, _) = app.definition_wait.as_ref().unwrap().clone();
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
                        Ok(LspAnswer::Definition(Some(response)))
                            if matches!(response, lsp_types::GotoDefinitionResponse::Array(a) if a.is_empty())
                                || matches!(response, lsp_types::GotoDefinitionResponse::Link(a) if a.is_empty()) => {}
                        Ok(LspAnswer::Definition(Some(_))) => {
                            let _ = target_native(&message, target)?;
                            return Ok(message);
                        }
                        Ok(LspAnswer::Definition(None)) => {}
                        Err(e) if e.starts_with("server error -32801:") => {}
                        _ => {
                            return Err(
                                io::Error::other("risposta definizione nativa inattesa").into()
                            );
                        }
                    }
                    // None/ContentModified: solo fixture attiva readonly, nuova
                    // richiesta/ID, non politica retry prodotto o writer.
                    consegna(app, message)?;
                    break;
                }
                consegna(app, message)?;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "definizione consumer reale opt-in: test-app-lsp-definition-real"]
async fn recipe_definizione_edit_stale_apertura_file_posizione_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE68:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-definition68-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("progetto");
    let src = root.join("src");
    std::fs::create_dir(&root)?;
    std::fs::create_dir(&src)?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"native_definition68\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    let target = src.join("città.rs");
    std::fs::write(&target, TARGET)?;
    let mut app = app();
    let source = write_and_open(&mut app, &src, "lib.rs", SOURCE);
    let doc = app.active_doc().unwrap();
    let anchor = SOURCE[..SOURCE.find("dato()").unwrap()].chars().count();
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor + 1));
    app.project_root = Some(root.clone());
    let _ = accensione(&mut app, &root, fixture.path(), &program);
    let mut stream = recipe(&app);
    let mut collected = Collection67::default();
    let mut reader_maybe_started = false;
    let mut reader_finished = false;
    let mut stale_task_maybe_started = false;
    let mut stale_task_finished = false;
    let scenario = limitato(
        async {
            while !app.lsp.only().unwrap().can_define() {
                consegna(&mut app, evento(&mut stream).await?)?;
            }
            assert!(app.workspace.get_by_path(&target).is_none());
            let old = definizione_reale(&mut app, &mut stream, &target).await?;
            let wait = app.definition_wait.clone();
            let version = app.workspace.get(doc).unwrap().version();
            let group = app.groups.group_of(doc).unwrap();
            let _ = app.update(Message::EditorIn(group, EditorCommand::Insert(" ".into())));
            assert_ne!(app.workspace.get(doc).unwrap().rope().to_string(), SOURCE);
            assert!(app.workspace.get(doc).unwrap().version() > version);
            // Diverso da hover64: l'attesa resta. La notice specifica comprova
            // l'ingresso nel ramo revision guard, non solo una domanda cancellata.
            assert_eq!(app.definition_wait, wait);
            let stale_task = app.update(old);
            assert!(app.definition_wait.is_none());
            assert!(app.workspace.get_by_path(&target).is_none());
            assert_eq!(app.active_doc(), Some(doc));
            assert_eq!(
                app.notice.as_ref().unwrap().1.text,
                "Il testo è cambiato mentre il server rispondeva"
            );
            // notify restituisce il timer NoticeExpired, non Task::none.
            // Pollare l'output vero: non deve contenere un reader tardivo.
            let notice_id = app.notice.as_ref().unwrap().0;
            let mut timer = iced_runtime::task::into_stream(stale_task)
                .ok_or_else(|| io::Error::other("Task timer stale assente"))?;
            let mut expirations = 0;
            // Anche una regressione con reader nel ramo stale deve lasciare
            // la fixture se un poll fallisce prima di raccogliere l'output.
            stale_task_maybe_started = true;
            while let Some(action) = tokio::time::timeout(ATTESA, timer.next()).await? {
                let iced_runtime::Action::Output(message @ Message::NoticeExpired(id)) = action
                else {
                    return Err(
                        io::Error::other("Task stale non è soltanto scadenza notice").into(),
                    );
                };
                assert_eq!(id, notice_id);
                expirations += 1;
                let _ = app.update(message);
            }
            stale_task_finished = true;
            assert_eq!(expirations, 1, "timer reale obbligatorio, non stream vuoto");
            assert!(app.workspace.get_by_path(&target).is_none());
            let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
            assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), SOURCE);
            assert!(!app.workspace.get(doc).unwrap().is_dirty());
            app.workspace
                .get_mut(doc)
                .unwrap()
                .set_selection(lume_core::Range::point(anchor + 1));
            let answer = definizione_reale(&mut app, &mut stream, &target).await?;
            let position = target_native(&answer, &target)?;
            let task = app.update(answer);
            assert!(
                app.workspace.get_by_path(&target).is_none(),
                "Task di apertura ancora freddo"
            );
            let mut reading = iced_runtime::task::into_stream(task)
                .ok_or_else(|| io::Error::other("definizione non produce Task lettura"))?;
            // Conservativo: da questo poll la lettura può essere ammessa.
            // Un timeout/panic non è prova che spawn_blocking sia raccolto.
            reader_maybe_started = true;
            let action = tokio::time::timeout(ATTESA, reading.next())
                .await?
                .ok_or_else(|| io::Error::other("Task lettura senza output"))?;
            let iced_runtime::Action::Output(message) = action else {
                return Err(io::Error::other("azione non output lettura").into());
            };
            let Message::FileReadFinished(_, _, result) = &message else {
                return Err(io::Error::other("output non FileReadFinished").into());
            };
            // Il producer ha realmente awaitato il JoinHandle del reader;
            // registrare prima delle assertion/update potenzialmente fallibili.
            reader_finished = true;
            assert_eq!(result.path, target);
            assert_eq!(&**result.esito.as_ref().unwrap(), TARGET);
            // Risultato prodotto dall'app spawn_blocking reale: non sintetizzato.
            let _ = app.update(message);
            assert!(
                tokio::time::timeout(ATTESA, reading.next())
                    .await?
                    .is_none()
            );
            let opened = app.workspace.get_by_path(&target).unwrap();
            assert_ne!(opened, doc);
            assert_eq!(app.active_doc(), Some(opened));
            let document = app.workspace.get(opened).unwrap();
            assert_eq!(document.path(), Some(target.as_path()));
            assert_eq!(document.rope().to_string(), TARGET);
            assert_eq!(
                document.selection().head,
                TARGET[..TARGET.find("dato").unwrap()].chars().count()
            );
            assert!(!document.is_dirty());
            assert_eq!(
                app.lsp.only().unwrap().opened.get(&opened),
                crate::lsp_uri::path_to_uri(&target).as_ref()
            );
            assert_eq!(std::fs::read_to_string(&source)?, SOURCE);
            assert_eq!(std::fs::read_to_string(&target)?, TARGET);
            eprintln!(
                "APP_REALE68:range_utf16={}:{},caret_char={}",
                position.line,
                position.character,
                document.selection().head
            );
            Ok(())
        },
        "definition68 scenario: raccolta attore sarà tentata",
    )
    .await;
    let shutdown = limitato(
        async {
            let _ = app.stop_lsp();
            Ok(())
        },
        "definition68 invio stop incerto",
    )
    .await;
    let stop = limitato(
        raccolta67(&mut app, &mut stream, &mut collected),
        "definition68 raccolta incerta",
    )
    .await;
    drop(stream);
    if let Some(e) = shutdown.err().or(stop.err()) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE68:fixture_preservata={}; raccolta incerta",
            residue.display()
        );
        return Err(e);
    }
    if (reader_maybe_started && !reader_finished)
        || (stale_task_maybe_started && !stale_task_finished)
    {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE68:fixture_preservata={}; reader/Task stale non raccolto",
            residue.display()
        );
        scenario?;
        return Err(io::Error::other("reader forse avviato, risultato non raccolto").into());
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE68:sequenza_completa=definition_native_edit_stale_undo_native_read_file_utf16_exitstatus_drop"
    );
    Ok(())
}
