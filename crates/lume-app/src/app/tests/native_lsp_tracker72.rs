//! Tracker Iced reale elimina la recipe dopo rootchange: drop e reaper pubblico.
//! Non renderer/app main exit, stderr-task await, exitcode o process-tree proof.
use super::root67::accensione;
use super::*;
use futures::channel::mpsc;
use iced_runtime::futures::subscription::{Tracker, into_recipes};
use std::task::Poll;

type DrainJob72 = Option<tokio::task::JoinHandle<io::Result<usize>>>;

async fn drain72(job: &mut DrainJob72) -> Esito<usize> {
    // L'handle resta fuori dal future cancellabile: il teardown raccoglie
    // l'invocazione già avviata, non crea un secondo drain in concorrenza.
    if job.is_none() {
        *job = Some(tokio::task::spawn_blocking(|| {
            lume_lsp::drain_process_reaper(Duration::from_secs(5))
        }));
    }
    let result = job.as_mut().unwrap().await;
    drop(job.take()); // collection prima di propagare JoinError/errore IO.
    Ok(result??)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Tracker LSP reale opt-in: test-app-lsp-tracker-real"]
async fn tracker_root_elimina_recipe_drop_reaper_worker_join_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE72:binario_sha256={}",
        crate::aggiornamento::esadecimale(&<sha2::Sha256 as sha2::Digest>::digest(std::fs::read(
            &program
        )?))
    );
    let mut drain_job: DrainJob72 = None;
    assert_eq!(
        drain72(&mut drain_job).await?,
        0,
        "singleton quiescente prima della recipe"
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-tracker72-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("prima");
    let other = fixture.path().join("seconda");
    let src = root.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::create_dir(&other)?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"native_tracker72\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    std::fs::write(src.join("lib.rs"), "pub mod città;\n")?;
    let mut app = app();
    let path = write_and_open(&mut app, &src, "città.rs", TESTO);
    let doc = app.active_doc().unwrap();
    let anchor = TESTO[..TESTO.rfind("valore;").unwrap()].chars().count();
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor + 1));
    app.project_root = Some(root.clone());
    let _ = accensione(&mut app, &root, fixture.path(), &program);
    let mut tracker = Tracker::new();
    let (sender, mut receiver) = mpsc::channel::<Message>(16);
    // Esattamente la recipe estratta dalla subscription app, non un initializer
    // ricostruito o tutte le subscription/watchers/DB/PTY.
    let recipe = recipe_prodotta(&app);
    let hash = |recipe: &dyn iced_runtime::futures::subscription::Recipe<Output = Message>| {
        use std::hash::Hasher as _;
        let mut hasher = iced_runtime::futures::subscription::Hasher::default();
        recipe.hash(&mut hasher);
        hasher.finish()
    };
    let old_hash = hash(&*recipe);
    let mut executions = tracker.update(std::iter::once(recipe), sender.clone());
    assert_eq!(executions.len(), 1);
    let mut execution = executions.pop();
    let mut execution_finished = false;
    let mut drain_finished = false;
    let mut maybe_started = false;
    let mut unexpected_finish = false;
    let scenario = limitato(async {
        {
            let mut events = futures::stream::poll_fn(|cx| {
                if execution.as_mut().unwrap().as_mut().poll(cx).is_ready() {
                    execution_finished = true;
                    unexpected_finish = true;
                    return Poll::Ready(None);
                }
                receiver.poll_next_unpin(cx)
            }).boxed();
            maybe_started = true;
            while !app.lsp.only().unwrap().can_hover() {
                consegna(&mut app, evento(&mut events).await?)?;
            }
            let answer = hover_reale(&mut app, &mut events).await?;
            let _ = app.update(answer);
            assert_eq!(app.hover.as_ref().unwrap().doc, doc);
        }
        assert!(!unexpected_finish, "execution vive prima del cambio root");
        // Nessun await/interleave tra clear delle mailbox e aggiornamento
        // Tracker: non tratteniamo il vecchio stream come collector67.
        let _ = app.update(Message::FolderPicked(Some(other.clone())));
        assert!(app.lsp_spawns().is_empty());
        let remaining: Vec<_> = into_recipes(app.subscription()).into_iter()
            .filter(|recipe| hash(&**recipe) == old_hash).collect();
        assert!(remaining.is_empty(), "app non produce più la recipe vecchia");
        assert!(tracker.update(remaining.into_iter(), sender.clone()).is_empty());
        tokio::time::timeout(ATTESA, execution.as_mut().unwrap()).await?;
        execution_finished = true;
        drop(execution.take());
        let joined_workers = drain72(&mut drain_job).await?;
        drain_finished = true;
        assert_eq!(joined_workers, 1, "join del worker reaper finale, NON conteggio figli");
        drain_finished = false;
        let repeated = drain72(&mut drain_job).await?;
        drain_finished = true;
        assert_eq!(repeated, 0, "drain idempotente dopo collection quiescente");
        assert_eq!(std::fs::read_to_string(&path)?, TESTO);
        eprintln!("APP_REALE72:tracker_execution_collected=true;reaper_workers_joined={joined_workers}");
        Ok(())
    }, "tracker72 scenario: teardown indipendente sarà tentato").await;
    let teardown = limitato(
        async {
            if !execution_finished {
                let _ = app.stop_lsp();
                let empty = std::iter::empty::<
                    Box<dyn iced_runtime::futures::subscription::Recipe<Output = Message>>,
                >();
                let _ = tracker.update(empty, sender.clone());
                tokio::time::timeout(ATTESA, execution.as_mut().unwrap()).await?;
                execution_finished = true;
            }
            drop(execution.take());
            Ok(())
        },
        "tracker72 cancellazione/collection execution incerta",
    )
    .await;
    // Anche se l'await dell'execution è incerto, Drop dell'owner precede il
    // drain indipendente: la consegna al reaper non arriva dopo il suo await.
    drop(execution.take());
    drop(tracker);
    let drain = limitato(
        async {
            if !drain_finished {
                let _ = drain72(&mut drain_job).await?;
                drain_finished = true;
            }
            Ok(())
        },
        "tracker72 drain incerto",
    )
    .await;
    drop(execution);
    drop(receiver);
    drop(sender);
    if let Some(error) = teardown.err().or(drain.err()) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE72:fixture_preservata={}; execution/reaper incerti",
            residue.display()
        );
        return Err(error);
    }
    if maybe_started && (!execution_finished || !drain_finished) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE72:fixture_preservata={}; collection non attestata",
            residue.display()
        );
        scenario?;
        return Err(io::Error::other("execution/reaper forse avviati non raccolti").into());
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE72:sequenza_completa=actual_recipe_tracker_native_hover_root_cancel_execution_drop_reaper_worker_join"
    );
    Ok(())
}
