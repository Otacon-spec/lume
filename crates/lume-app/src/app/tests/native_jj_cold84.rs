//! Task azione genuino cold Drop → seconda azione dal tool realmente eseguita.
//! Non ammissione/risultato Drop, GUI/OS input o cleanup process-tree.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "JJ app cold reale opt-in: test-app-jj-cold-real"]
async fn cold_task_drop_rilascia_azione_tool_e_ammette_successore_reale() -> Esito80 {
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
        .prefix("lume-jj84-città spazio-")
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
    let scenario = tokio::time::timeout(Duration::from_secs(60), std::panic::AssertUnwindSafe(async {
        app.apply_activity_intent(ActivityBarIntent::Click(tools::jj::ID));
        avvia80(app.drain_tool_requests(), &mut producer, &mut maybe_started, &mut finished)?;
        let messages = raccogli80(&mut producer, &mut finished).await?;
        snapshot80(&mut app, messages, "", false, None);
        const ABANDONED: &str = "NON eseguire questa descrizione84";
        const FRESH: &str = "dopo cold Drop84 città";
        let _ = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::Message(ABANDONED.into()))));
        let cold = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::Describe)));
        assert!(app.jj_busy);
        let generation = app.jj_gen;
        // Task originale mai convertita/pollata: nessuna ammissione worker.
        // Drop reale, non sostituzione/cancelDTO. Stato UI può riconciliare al
        // prossimo input esplicito; non polling/timer idle per questa revoca.
        drop(cold);
        assert_eq!(setup80(&program, &home, &root, &["log","--no-graph","-r","@","-T","description"])?, "");
        let _ = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::Message(FRESH.into()))));
        let task = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::Describe)));
        avvia80(task, &mut producer, &mut maybe_started, &mut finished)?;
        let mut messages = raccogli80(&mut producer, &mut finished).await?;
        assert_eq!(messages.len(), 1, "il successore deve produrre authentic ActionDone, non un tool ancora bloccato");
        let done = messages.pop().unwrap();
        assert!(matches!(&done, Message::JjActionDone(g, JjAction::Describe(m), Ok(_)) if *g == app.jj_gen && *g > generation && m == FRESH));
        app.notice = None;
        let followup = app.update(done);
        assert!(!app.jj_busy);
        assert_eq!(app.notice.as_ref().unwrap().1.kind, NoticeKind::Info);
        let nonce = app.notice.as_ref().unwrap().0;
        assert_eq!(setup80(&program, &home, &root, &["log","--no-graph","-r","@","-T","description"])?, format!("{FRESH}\n"));
        avvia80(followup, &mut producer, &mut maybe_started, &mut finished)?;
        let messages = raccogli80(&mut producer, &mut finished).await?;
        snapshot80(&mut app, messages, FRESH, false, Some(nonce));
        assert_eq!(std::fs::read_to_string(&file)?, "testo 😀\n");
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }).catch_unwind()).await;
    let result: Esito80 = match scenario {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(io::Error::other("scenario JJ84 panic; raccolta da verificare").into()),
        Err(_) => Err(io::Error::other("scenario JJ84 deadline; raccolta da verificare").into()),
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
                "JJ_APP_COLD_REALE84:fixture_preservata={}; producer incerto",
                residue.display()
            );
            return Err(io::Error::other("producer JJ84 non raccolto").into());
        }
    }
    if let Err(error) = result {
        let residue = fixture.keep();
        eprintln!(
            "JJ_APP_COLD_REALE84:fixture_preservata={}; scenario fallito",
            residue.display()
        );
        return Err(error);
    }
    fixture.close()?;
    eprintln!(
        "JJ_APP_COLD_REALE84:sequenza_completa=actual_tool_native_load_cold_describe_task_drop_no_effect_fresh_describe_done_reload_notice"
    );
    Ok(())
}
