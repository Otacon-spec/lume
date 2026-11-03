//! Risultato autentico/EOF poi Drop senza update; recupero al prossimo input.
//! Non perdita della future ammessa durante il worker o lifecycle GUI.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "JJ risultato reale opt-in: test-app-jj-result-real"]
async fn risultato_task_reale_perso_recuperato_prima_del_nuovo_input() -> Esito80 {
    let program = std::env::var_os("LUME_JJ")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("jj reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("jj reale non assoluto/file").into());
    }
    let cli = lume_jj::JjCli::locate()
        .ok_or_else(|| io::Error::other("discovery jj produzione assente"))?;
    assert_eq!(cli.program(), program);
    let home = std::env::var_os("LUME_JJ_TEST_HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("profilo privato runner mancante"))?;
    assert_eq!(
        std::env::var_os("HOME"),
        Some(home.clone().into_os_string())
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-jj85-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("repo");
    std::fs::create_dir(&root)?;
    if let Err(error) = setup80(&program, &home, &root, &["git", "init"]) {
        let _ = fixture.keep();
        return Err(error);
    }
    let file = root.join("città.txt");
    std::fs::write(&file, "testo 😀\n")?;
    let mut app = app();
    app.set_project_root(root.clone());
    let mut bootstrap = std::mem::take(&mut app.pending_tool_requests);
    bootstrap.extend(app.registry.set_active_panel(None));
    assert!(bootstrap.iter().all(|(id, _)| *id != tools::jj::ID));
    let mut producer = None;
    let mut maybe_started = false;
    let mut finished = false;
    let scenario = tokio::time::timeout(
        Duration::from_secs(60),
        std::panic::AssertUnwindSafe(async {
            app.apply_activity_intent(ActivityBarIntent::Click(tools::jj::ID));
            avvia80(
                app.drain_tool_requests(),
                &mut producer,
                &mut maybe_started,
                &mut finished,
            )?;
            let messages = raccogli80(&mut producer, &mut finished).await?;
            snapshot80(&mut app, messages, "", false, None);
            const DESCRIPTION: &str = "descrizione realmente eseguita85 città";
            let _ = app.update(Message::Tool(
                tools::jj::ID,
                ToolEvent::new(tools::jj::Event::Message(DESCRIPTION.into())),
            ));
            let task = app.update(Message::Tool(
                tools::jj::ID,
                ToolEvent::new(tools::jj::Event::Describe),
            ));
            avvia80(task, &mut producer, &mut maybe_started, &mut finished)?;
            let messages = raccogli80(&mut producer, &mut finished).await?;
            assert_eq!(messages.len(), 1);
            assert!(matches!(&messages[0], Message::JjActionDone(g, JjAction::Describe(m), Ok(_)) if *g == app.jj_gen && m == DESCRIPTION));
        let generation = app.jj_gen;
            assert_eq!(
                setup80(
                    &program,
                    &home,
                    &root,
                    &["log", "--no-graph", "-r", "@", "-T", "description"]
                )?,
                format!("{DESCRIPTION}\n")
            );
            // Genuine output già positivo e EOF, non DTO inventato/clone consegnato.
            // La future ha awaited il worker; distruggerne il messaggio non è rollback.
            drop(messages);
            app.notice = None;
            let task = app.update(Message::Tool(
                tools::jj::ID,
                ToolEvent::new(tools::jj::Event::Message(
                    "nuova bozza non eseguita85".into(),
                )),
            ));
            let messages = if let Some(source) = iced_runtime::task::into_stream(task) {
                maybe_started = true;
                finished = false;
                producer = Some(source.boxed());
                raccogli80(&mut producer, &mut finished).await?
            } else {
                Vec::new()
            };
            // Raccolta eventuale followup+EOF prima delle assertion dell'UI.
            assert!(
                !app.jj_busy,
                "risultato JJ già completato ma perso lascia il gate occupato"
            );
            let notice = app
                .notice
                .as_ref()
                .ok_or_else(|| io::Error::other("esito autentico perso non recuperato dall'app"))?;
            assert_eq!(notice.1.kind, NoticeKind::Info);
            assert!(notice.1.text.starts_with("Jujutsu - Descrizione:"));
            let nonce = notice.0;
            snapshot80(&mut app, messages, DESCRIPTION, false, Some(nonce));
            assert_eq!(
                app.jj_gen,
                generation + 1,
                "un solo reload, nessuna nuova Describe o doppia azione"
            );
            assert_eq!(
                setup80(
                    &program,
                    &home,
                    &root,
                    &["log", "--no-graph", "-r", "@", "-T", "description"]
                )?,
                format!("{DESCRIPTION}\n")
            );
            assert_eq!(std::fs::read_to_string(&file)?, "testo 😀\n");
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        })
        .catch_unwind(),
    )
    .await;
    let result: Esito80 = match scenario {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(io::Error::other("scenario JJ85 panic; raccolta da verificare").into()),
        Err(_) => Err(io::Error::other("scenario JJ85 deadline; raccolta da verificare").into()),
    };
    if maybe_started && !finished {
        app.jj_cancel.store(true, Ordering::SeqCst);
        let cleanup = tokio::time::timeout(
            Duration::from_secs(30),
            raccogli80(&mut producer, &mut finished),
        )
        .await;
        if !matches!(cleanup, Ok(Ok(_))) || !finished {
            let residue = fixture.keep();
            eprintln!(
                "JJ_APP_RESULT_REALE85:fixture_preservata={}; producer incerto",
                residue.display()
            );
            return Err(io::Error::other("producer JJ85 non raccolto").into());
        }
    }
    if let Err(error) = result {
        let residue = fixture.keep();
        eprintln!(
            "JJ_APP_RESULT_REALE85:fixture_preservata={}; scenario fallito",
            residue.display()
        );
        return Err(error);
    }
    fixture.close()?;
    eprintln!(
        "JJ_APP_RESULT_REALE85:sequenza_completa=native_describe_auth_done_eof_drop_message_recover_next_input_notice_single_reload_no_reexecute"
    );
    Ok(())
}
