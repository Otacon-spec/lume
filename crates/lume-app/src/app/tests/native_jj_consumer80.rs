//! Pannello/controller app → Task/worker → jj reale → DTO/notify/reload.
//! Non renderer/input OS, cancellazione, join dei lettori o tree teardown.
use super::*;
use futures::{FutureExt, StreamExt};
use iced::Task;
use lume_ui::tool::{JjAction, ToolEvent};
use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::Ordering,
    time::Duration,
};

#[cfg(target_os = "linux")]
#[path = "native_jj_admitted86.rs"]
mod admitted86;
#[path = "native_jj_cold84.rs"]
mod cold84;
#[path = "native_jj_cold_load87.rs"]
mod cold_load87;
#[cfg(target_os = "linux")]
#[path = "native_jj_final_owner89.rs"]
mod final_owner89;
#[cfg(target_os = "linux")]
#[path = "native_jj_load_admitted88.rs"]
mod load_admitted88;
#[path = "native_jj_result85.rs"]
mod result85;

type Esito80<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Stream80 = futures::stream::BoxStream<'static, iced_runtime::Action<Message>>;

fn setup80(program: &Path, home: &Path, root: &Path, args: &[&str]) -> Esito80<String> {
    let output = Command::new(program)
        .env_clear()
        .args(["--no-pager", "--color", "never"])
        .args(args)
        .current_dir(root)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("JJ_CONFIG", home.join("jj.toml"))
        .env("JJ_EDITOR", "true")
        .env("GIT_EDITOR", "true")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("git-empty"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C.UTF-8")
        .stdin(std::process::Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(
            io::Error::other("setup/probe jj reale fallito; log grezzo non pubblicato").into(),
        );
    }
    Ok(String::from_utf8(output.stdout)?)
}

async fn raccogli80(stream: &mut Option<Stream80>, finished: &mut bool) -> Esito80<Vec<Message>> {
    let mut messages = Vec::new();
    let source = stream
        .as_mut()
        .ok_or_else(|| io::Error::other("producer app assente"))?;
    while let Some(action) = source.next().await {
        let iced_runtime::Action::Output(message) = action else {
            return Err(io::Error::other("azione non Output nel producer JJ").into());
        };
        messages.push(message);
        if messages.len() > 3 {
            return Err(io::Error::other("producer JJ con output ulteriori").into());
        }
    }
    *finished = true; // output+EOF prima di assertion/update.
    *stream = None; // Drop effettivo del producer già terminato.
    Ok(messages)
}
fn avvia80(
    task: Task<Message>,
    stream: &mut Option<Stream80>,
    maybe: &mut bool,
    finished: &mut bool,
) -> Esito80 {
    assert!(
        stream.is_none(),
        "un producer alla volta, nessuna sostituzione di owner vivo"
    );
    *maybe = true; // conservativo prima del primo poll.
    *finished = false;
    *stream = iced_runtime::task::into_stream(task).map(|source| source.boxed());
    if stream.is_none() {
        return Err(io::Error::other("Task JJ app assente").into());
    }
    Ok(())
}
async fn assenza80(
    task: Task<Message>,
    stream: &mut Option<Stream80>,
    maybe: &mut bool,
    finished: &mut bool,
) -> Esito80 {
    assert!(stream.is_none());
    if let Some(source) = iced_runtime::task::into_stream(task) {
        *maybe = true;
        *finished = false;
        *stream = Some(source.boxed());
        let messages = raccogli80(stream, finished).await?;
        assert!(
            messages.is_empty(),
            "evento che tace: raccolto EOF senza output"
        );
    }
    Ok(())
}
fn snapshot80(
    app: &mut Lume,
    messages: Vec<Message>,
    expected: &str,
    empty: bool,
    notice: Option<u64>,
) {
    let mut loaded = 0;
    let mut expired = 0;
    for message in messages {
        match &message {
            Message::JjLoaded(generation, result) => {
                assert_eq!(*generation, app.jj_gen);
                let snap = result
                    .as_ref()
                    .expect("snapshot nativo positivo, non Err come prova");
                assert_eq!(snap.entries[0].description, expected);
                assert!(snap.entries[0].working_copy);
                assert_eq!(snap.entries[0].empty, empty);
                assert_eq!(snap.entries[0].author, "Lume Native80");
                loaded += 1;
            }
            Message::JjLoadCollected(signal) => {
                let pending = app.jj_pending_load.as_ref().expect("owner del load reale");
                assert!(pending.matches(signal));
                assert_eq!(pending.generation(), app.jj_gen);
                pending.inspect_completion(|result| {
                    let snap = result
                        .expect("completion autentica pubblicata")
                        .as_ref()
                        .expect("snapshot reale positivo");
                    assert_eq!(snap.entries[0].description, expected);
                    assert!(snap.entries[0].working_copy);
                    assert_eq!(snap.entries[0].empty, empty);
                    assert_eq!(snap.entries[0].author, "Lume Native80");
                });
                loaded += 1;
            }
            Message::NoticeExpired(id) => {
                assert_eq!(
                    Some(*id),
                    notice,
                    "scadenza della notice effettivamente creata"
                );
                assert_eq!(app.notice.as_ref().map(|(id, _)| *id), Some(*id));
                expired += 1;
            }
            _ => panic!("output ulteriore inatteso dal producer JJ: {message:?}"),
        }
        let _ = app.update(message);
    }
    assert_eq!(loaded, 1, "un risultato autentico per caricamento");
    assert_eq!(
        expired,
        usize::from(notice.is_some()),
        "followup include la scadenza, non solo reload"
    );
    if notice.is_some() {
        assert!(
            app.notice.is_none(),
            "la consegna della scadenza ha rimosso la notice"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "JJ app reale opt-in: test-app-jj-real"]
async fn pannello_task_jj_reale_snapshot_describe_new_reload() -> Esito80 {
    let program = std::env::var_os("LUME_JJ")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("jj reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("jj reale non assoluto/file").into());
    }
    let cli = lume_jj::JjCli::locate()
        .ok_or_else(|| io::Error::other("jj non trovato dal discovery produzione"))?;
    assert_eq!(
        cli.program(),
        program,
        "nessun locator finto o binario equivalente"
    );
    let home = std::env::var_os("LUME_JJ_TEST_HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("ambiente privato del runner mancante"))?;
    assert_eq!(
        std::env::var_os("HOME"),
        Some(home.clone().into_os_string())
    );
    eprintln!(
        "JJ_APP_REALE80:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-jj80-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("repo");
    std::fs::create_dir(&root)?;
    // Setup sincrono fuori dal deadline scenario: non claim di timeout end-to-end.
    // In caso di errore, conservare la fixture e non nascondere cleanup incerto.
    if let Err(error) = setup80(&program, &home, &root, &["git", "init"]) {
        let _ = fixture.keep();
        return Err(error);
    }
    let text = root.join("città.txt");
    std::fs::write(&text, "uno 😀\n")?;
    let nested = root.join("servizi/api");
    std::fs::create_dir_all(&nested)?;
    let mut app = app();
    app.set_project_root(nested.clone());
    assert_eq!(app.jj_workspace.as_deref(), Some(root.as_path()));
    assert!(app.registry.is_available(tools::jj::ID));
    // Solo bootstrap della fixture: gli altri domini possono aver accodato
    // richieste al cambio root. Conservare i DTO freddi senza drenarli/eseguirli.
    let mut bootstrap_requests = std::mem::take(&mut app.pending_tool_requests);
    bootstrap_requests.extend(app.registry.set_active_panel(None));
    assert!(
        bootstrap_requests
            .iter()
            .all(|(id, _)| *id != tools::jj::ID)
    );
    let mut producer = None;
    let mut maybe_started = false;
    let mut finished = false;
    let scenario = tokio::time::timeout(Duration::from_secs(60), std::panic::AssertUnwindSafe(async {
        app.apply_activity_intent(ActivityBarIntent::Click(tools::jj::ID));
        assert!(app.registry.is_open(tools::jj::ID));
        assert!(app.pending_tool_requests.iter().all(|(id, _)| *id == tools::jj::ID));
        let task = app.drain_tool_requests();
        avvia80(task, &mut producer, &mut maybe_started, &mut finished)?;
        let messages = raccogli80(&mut producer, &mut finished).await?;
        snapshot80(&mut app, messages, "", false, None);
        const DESCRIPTION: &str = "--amend\n\ncorpo reale città";
        let _ = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::Message(DESCRIPTION.into()))));
        let task = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::Describe)));
        assert!(app.jj_busy, "evento del pannello ha prodotto vera richiesta app");
        avvia80(task, &mut producer, &mut maybe_started, &mut finished)?;
        let mut messages = raccogli80(&mut producer, &mut finished).await?;
        assert_eq!(messages.len(), 1);
        let done = messages.pop().unwrap();
        assert!(matches!(&done, Message::JjActionDone(generation, JjAction::Describe(m), Ok(_)) if *generation == app.jj_gen && m == DESCRIPTION));
        app.notice = None;
        let followup = app.update(done);
        assert!(!app.jj_busy);
        assert!(matches!(app.notice.as_ref().unwrap().1.kind, NoticeKind::Info));
        assert!(app.notice.as_ref().unwrap().1.text.starts_with("Jujutsu - Descrizione:"));
        assert_eq!(setup80(&program, &home, &root, &["log", "--no-graph", "-r", "@", "-T", "description"])?, format!("{DESCRIPTION}\n"));
        let notice = app.notice.as_ref().unwrap().0;
        avvia80(followup, &mut producer, &mut maybe_started, &mut finished)?;
        let messages = raccogli80(&mut producer, &mut finished).await?;
        snapshot80(&mut app, messages, "--amend", false, Some(notice));
        // Bozza realmente consumata dal tool: Describe senza nuova bozza tace.
        let generation = app.jj_gen;
        let task = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::Describe)));
        assenza80(task, &mut producer, &mut maybe_started, &mut finished).await?;
        assert_eq!(app.jj_gen, generation);
        let task = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::New)));
        assert!(app.jj_busy);
        avvia80(task, &mut producer, &mut maybe_started, &mut finished)?;
        let mut messages = raccogli80(&mut producer, &mut finished).await?;
        assert_eq!(messages.len(), 1);
        let done = messages.pop().unwrap();
        assert!(matches!(&done, Message::JjActionDone(generation, JjAction::New(None), Ok(_)) if *generation == app.jj_gen));
        app.notice = None;
        let followup = app.update(done);
        assert!(!app.jj_busy);
        assert!(matches!(app.notice.as_ref().unwrap().1.kind, NoticeKind::Info));
        assert!(app.notice.as_ref().unwrap().1.text.starts_with("Jujutsu - Nuovo cambiamento:"));
        let notice = app.notice.as_ref().unwrap().0;
        avvia80(followup, &mut producer, &mut maybe_started, &mut finished)?;
        let messages = raccogli80(&mut producer, &mut finished).await?;
        snapshot80(&mut app, messages, "", true, Some(notice));
        let generation = app.jj_gen;
        let _ = app.update(Message::ClosePanel);
        assert!(!app.registry.is_open(tools::jj::ID));
        let task = app.update(Message::Tool(tools::jj::ID, ToolEvent::new(tools::jj::Event::Refresh)));
        assenza80(task, &mut producer, &mut maybe_started, &mut finished).await?;
        assert_eq!(app.jj_gen, generation, "chiuso non rilancia backend");
        assert_eq!(std::fs::read_to_string(&text)?, "uno 😀\n");
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }).catch_unwind()).await;
    let result: Esito80 = match scenario {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(io::Error::other("scenario JJ panic; raccolta da verificare").into()),
        Err(_) => Err(io::Error::other("scenario JJ deadline; raccolta da verificare").into()),
    };
    if maybe_started && !finished {
        app.jj_cancel.store(true, Ordering::SeqCst);
        // Owner del producer fuori dal future cancellato; raccogliere quello
        // esistente, senza avviare un collector/worker sostitutivo.
        if tokio::time::timeout(
            Duration::from_secs(30),
            raccogli80(&mut producer, &mut finished),
        )
        .await
        .is_err()
            || !finished
        {
            let residue = fixture.keep();
            eprintln!(
                "JJ_APP_REALE80:fixture_preservata={}; worker app incerto",
                residue.display()
            );
            return Err(io::Error::other("producer JJ non raccolto").into());
        }
    }
    if let Err(error) = result {
        let residue = fixture.keep();
        eprintln!(
            "JJ_APP_REALE80:fixture_preservata={}; scenario fallito",
            residue.display()
        );
        return Err(error);
    }
    fixture.close()?;
    eprintln!(
        "JJ_APP_REALE80:sequenza_completa=actual_registry_events_task_native_jj_snapshot_describe_new_notices_reload_close"
    );
    Ok(())
}
