//! Genuine Refresh Task cold Drop: gate app/tool e successor native snapshot.
//! Non perdita di un load ammesso o main-close/GUI/runtime loss.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "JJ cold load reale opt-in: test-app-jj-load-cold-real"]
async fn refresh_task_cold_drop_libera_gate_app_tool_per_snapshot_reale() -> Esito80 {
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
        .prefix("lume-jj87-città spazio-")
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
    let scenario=tokio::time::timeout(Duration::from_secs(60),std::panic::AssertUnwindSafe(async {
        app.apply_activity_intent(ActivityBarIntent::Click(tools::jj::ID));
        avvia80(app.drain_tool_requests(),&mut producer,&mut maybe_started,&mut finished)?;
        let messages=raccogli80(&mut producer,&mut finished).await?;snapshot80(&mut app,messages,"",false,None);
        let before=app.jj_gen;
        let cold=app.update(Message::Tool(tools::jj::ID,ToolEvent::new(tools::jj::Event::Refresh)));
        assert!(app.jj_loading);assert_eq!(app.jj_gen,before+1);
        drop(cold); // Non conversione in stream/primo poll: genuine Task.
        assert_eq!(setup80(&program,&home,&root,&["log","--no-graph","-r","@","-T","description"])?,"");
        let task=app.update(Message::Tool(tools::jj::ID,ToolEvent::new(tools::jj::Event::Refresh)));
        let messages=if let Some(source)=iced_runtime::task::into_stream(task) {
            maybe_started=true;finished=false;producer=Some(source.boxed());raccogli80(&mut producer,&mut finished).await?
        } else {Vec::new()};
        assert_eq!(messages.len(),1,"Refresh successore deve produrre una raccolta autentica, non un gate loading ancora bloccato");
        let Message::JjLoadCollected(signal) = &messages[0] else {
            panic!("Refresh successore deve usare il collector owned di produzione");
        };
        let pending = app.jj_pending_load.as_ref().expect("owner del successore");
        assert!(pending.matches(signal));
        assert_eq!(pending.generation(), app.jj_gen);
        assert_eq!(app.jj_gen, before + 2);
        snapshot80(&mut app,messages,"",false,None);
        assert!(!app.jj_loading);assert!(!app.jj_busy);
        // Terza richiesta vera conferma che il gate tool, non solo flag app,
        // è disponibile dopo l'ack del secondo caricamento realmente completato.
        let task=app.update(Message::Tool(tools::jj::ID,ToolEvent::new(tools::jj::Event::Refresh)));
        avvia80(task,&mut producer,&mut maybe_started,&mut finished)?;
        let messages=raccogli80(&mut producer,&mut finished).await?;snapshot80(&mut app,messages,"",false,None);
        assert_eq!(app.jj_gen,before+3);assert!(!app.jj_loading);
        assert_eq!(std::fs::read_to_string(&file)?,"testo 😀\n");
        Ok::<(),Box<dyn std::error::Error+Send+Sync>>(())
    }).catch_unwind()).await;
    let result: Esito80 = match scenario {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err(io::Error::other("scenario JJ87 panic").into()),
        Err(_) => Err(io::Error::other("scenario JJ87 deadline").into()),
    };
    if maybe_started && !finished {
        app.jj_cancel.store(true, Ordering::SeqCst);
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(30),
                raccogli80(&mut producer, &mut finished)
            )
            .await,
            Ok(Ok(_))
        ) || !finished
        {
            let residue = fixture.keep();
            eprintln!(
                "JJ_APP_LOAD_COLD_REALE87:fixture_preservata={}; producer incerto",
                residue.display()
            );
            std::mem::forget((producer, app));
            return Err(io::Error::other("producer JJ87 non raccolto").into());
        }
    }
    if let Err(error) = result {
        let residue = fixture.keep();
        eprintln!(
            "JJ_APP_LOAD_COLD_REALE87:fixture_preservata={}; scenario fallito",
            residue.display()
        );
        return Err(error);
    }
    fixture.close()?;
    eprintln!(
        "JJ_APP_LOAD_COLD_REALE87:sequenza_completa=actual_registry_native_warm_refresh_cold_task_drop_fresh_refresh_loaded_eof_ack_third_refresh"
    );
    Ok(())
}
