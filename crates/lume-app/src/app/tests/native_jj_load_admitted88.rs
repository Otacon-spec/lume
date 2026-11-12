//! Load CLI ammesso, producer perso, collector della recipe app e successore.
//! Varianti runtime loss e ultimo owner; non GUI/main close/full containment.
use super::admitted86::{children86, flock86, hash86, identity86, waiting86, workers86};
use super::*;
use crate::jj_actions;
use iced::Subscription;
use iced_runtime::futures::subscription::{Tracker, into_recipes};
use sha2::{Digest, Sha256};
use std::{fs::OpenOptions, time::Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "load JJ ammesso reale Linux: test-app-jj-load-admitted-real"]
async fn load_ammesso_producer_perso_recipe_raccoglie_snapshot_successore_reali() -> Esito80 {
    scenario88(0).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "load JJ runtime loss reale Linux: test-app-jj-load-runtime-real"]
async fn load_ammesso_runtime_originario_perso_recipe_successore_reali() -> Esito80 {
    scenario88(1).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "load JJ last owner reale Linux: test-app-jj-load-last-owner-real"]
async fn load_ammesso_ultimo_owner_perso_child_lettori_assenti() -> Esito80 {
    scenario88(2).await
}

async fn scenario88(mode: u8) -> Esito80 {
    let program = std::env::var_os("LUME_JJ")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("jj esplicito richiesto, nessuno skip"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("jj non assoluto/file").into());
    }
    eprintln!(
        "JJ_LOAD_REALE88:server_sha256={}",
        crate::aggiornamento::esadecimale(&Sha256::digest(std::fs::read(&program)?))
    );
    let cli =
        lume_jj::JjCli::locate().ok_or_else(|| io::Error::other("locator produzione assente"))?;
    assert_eq!(cli.program(), program);
    let home = std::env::var_os("LUME_JJ_TEST_HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("profilo runner privato richiesto"))?;
    assert_eq!(
        std::env::var_os("HOME"),
        Some(home.clone().into_os_string())
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-jj-load88-città-")
        .tempdir()?;
    let root = fixture.path().join("repo");
    std::fs::create_dir(&root)?;
    if let Err(error) = setup80(&program, &home, &root, &["git", "init"]) {
        let _ = fixture.keep();
        return Err(error);
    }
    let file = root.join("città.txt");
    std::fs::write(&file, "load 😀\n")?;
    let mut app = app();
    app.set_project_root(root.clone());
    let mut bootstrap = std::mem::take(&mut app.pending_tool_requests);
    bootstrap.extend(app.registry.set_active_panel(None));
    assert!(bootstrap.iter().all(|(id, _)| *id != tools::jj::ID));
    let mut producer = None;
    let mut started = false;
    let mut finished = false;
    let mut lock = None;
    let mut origin = None;
    let mut execution: Option<futures::future::BoxFuture<'static, ()>> = None;
    let mut worker_runtime = if mode == 1 {
        Some(tokio::runtime::Builder::new_current_thread().build()?)
    } else {
        None
    };
    let mut tracker = Tracker::new();
    let (sender, mut receiver) = futures::channel::mpsc::channel::<Message>(2);
    let mut sender = Some(sender);
    let scenario = tokio::time::timeout(
        Duration::from_secs(60),
        std::panic::AssertUnwindSafe(async {
            assert_eq!(workers86()?, 0);
            let baseline_children = children86()?;
            app.apply_activity_intent(ActivityBarIntent::Click(tools::jj::ID));
            avvia80(
                app.drain_tool_requests(),
                &mut producer,
                &mut started,
                &mut finished,
            )?;
            let messages = raccogli80(&mut producer, &mut finished).await?;
            snapshot80(&mut app, messages, "", false, None);
            let held = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(root.join(".jj/working_copy/working_copy.lock"))?;
            flock86(&held, libc::LOCK_EX | libc::LOCK_NB)?;
            lock = Some(held);
            let generation = app.jj_gen;
            let task = app.update(Message::Tool(
                tools::jj::ID,
                ToolEvent::new(tools::jj::Event::Refresh),
            ));
            let pending = app.jj_pending_load.as_ref().ok_or("ticket load assente")?;
            let signal = pending.collector();
            let expected =
                Subscription::run_with(signal.clone(), jj_actions::load_admission::completed)
                    .map(Message::JjLoadCollected);
            let expected_hash = hash86(
                &*into_recipes(expected)
                    .pop()
                    .ok_or("recipe attesa assente")?,
            );
            let mut recipes: Vec<_> = into_recipes(app.subscription())
                .into_iter()
                .filter(|r| hash86(&**r) == expected_hash)
                .collect();
            assert_eq!(recipes.len(), 1, "recipe reale per il ticket corrente");
            avvia80(task, &mut producer, &mut started, &mut finished)?;
            let start = Instant::now();
            loop {
                let ended = futures::future::poll_fn(|cx| {
                    let _context = worker_runtime.as_ref().map(tokio::runtime::Runtime::enter);
                    std::task::Poll::Ready(
                        producer.as_mut().unwrap().poll_next_unpin(cx).is_ready(),
                    )
                })
                .await;
                if ended {
                    return Err(io::Error::other("producer finito prima del flock reale").into());
                }
                if let Some(id) = waiting86(lock.as_ref().unwrap(), &program)?
                    && workers86()? == 2
                {
                    origin = Some(id);
                    break;
                }
                if start.elapsed() > Duration::from_secs(10) {
                    return Err(io::Error::other("load/lettori non osservati").into());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            drop(producer.take()); // Invocazione già ammessa, non cold Drop.
            assert!(app.jj_loading);
            assert!(!signal.collected());
            if let Some(runtime) = worker_runtime.take() {
                tokio::task::spawn_blocking(move || runtime.shutdown_timeout(Duration::ZERO))
                    .await?;
                assert_eq!(workers86()?, 2);
                let id = origin.as_ref().unwrap();
                assert_eq!(identity86(id.0)?, Some(id.1.clone()));
                eprintln!("JJ_LOAD_REALE88:origin_runtime_lost_child_two_readers_alive=true");
            }
            if mode == 2 {
                // Nessun clone Collector/recipe del vecchio ticket sopravvive.
                recipes.clear();
                drop(signal);
                drop(std::mem::replace(&mut app, super::app()));
                let id = origin.as_ref().unwrap();
                let wait = Instant::now();
                loop {
                    if workers86()? == 0 && identity86(id.0)? != Some(id.1.clone()) {
                        break;
                    }
                    if wait.elapsed() > Duration::from_secs(10) {
                        return Err(
                            io::Error::other("last owner load: child/reader ancora vivi").into(),
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                assert_eq!(children86()?, baseline_children);
                assert!(!lume_jj::quarantine_has_children());
                let other = std::fs::File::open(root.join(".jj/working_copy/working_copy.lock"))?;
                assert_eq!(
                    flock86(&other, libc::LOCK_EX | libc::LOCK_NB)
                        .unwrap_err()
                        .kind(),
                    io::ErrorKind::WouldBlock
                );
                flock86(lock.as_ref().unwrap(), libc::LOCK_UN)?;
                drop(lock.take());
                assert_eq!(std::fs::read_to_string(&file)?, "load 😀\n");
                eprintln!(
                    "JJ_LOAD_REALE88:last_owner_gone_child_readers_absent_lock_still_held=true"
                );
                return Ok(());
            }
            let mut futures = tracker.update(
                std::iter::once(recipes.pop().unwrap()),
                sender.take().unwrap(),
            );
            assert_eq!(futures.len(), 1);
            execution = futures.pop();
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(30),
                    execution.as_mut().unwrap().as_mut()
                )
                .await
                .is_err()
            );
            assert_eq!(workers86()?, 2);
            let id = origin.as_ref().unwrap();
            assert_eq!(identity86(id.0)?, Some(id.1.clone()));
            assert_eq!(app.jj_gen, generation + 1);
            assert!(
                app.jj_pending_load
                    .as_ref()
                    .is_some_and(|p| p.matches(&signal))
            );
            flock86(lock.as_ref().unwrap(), libc::LOCK_UN)?;
            drop(lock.take());
            execution.as_mut().unwrap().as_mut().await;
            drop(execution.take());
            let message = receiver.next().await.ok_or("output collector assente")?;
            assert!(receiver.next().await.is_none(), "EOF senza duplicati");
            let Message::JjLoadCollected(received) = &message else {
                return Err(io::Error::other("messaggio non collector").into());
            };
            assert!(
                app.jj_pending_load
                    .as_ref()
                    .is_some_and(|p| p.matches(received))
            );
            let replay = received.clone();
            assert_eq!(workers86()?, 0);
            assert_ne!(identity86(id.0)?, Some(id.1.clone()));
            assert_eq!(children86()?, baseline_children);
            snapshot80(&mut app, vec![message], "", false, None);
            assert!(app.jj_pending_load.is_none());
            assert!(!app.jj_loading);
            let _ = app.update(Message::JjLoadCollected(replay));
            assert_eq!(app.jj_gen, generation + 1, "replay non riavvia il loader");
            avvia80(
                app.update(Message::Tool(
                    tools::jj::ID,
                    ToolEvent::new(tools::jj::Event::Refresh),
                )),
                &mut producer,
                &mut started,
                &mut finished,
            )?;
            let messages = raccogli80(&mut producer, &mut finished).await?;
            snapshot80(&mut app, messages, "", false, None);
            assert_eq!(app.jj_gen, generation + 2);
            assert!(!app.jj_loading);
            assert_eq!(workers86()?, 0);
            assert_eq!(children86()?, baseline_children);
            assert_eq!(std::fs::read_to_string(&file)?, "load 😀\n");
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        })
        .catch_unwind(),
    )
    .await;
    let result: Esito80 = match scenario {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(io::Error::other("scenario load88 panic").into()),
        Err(_) => Err(io::Error::other("scenario load88 deadline").into()),
    };
    // Raccolta dello stesso ticket anche se lo scenario perde il producer.
    app.jj_cancel.store(true, Ordering::SeqCst);
    drop(lock.take());
    let mut cleanup = true;
    if let Some(runtime) = worker_runtime.take() {
        cleanup &= tokio::task::spawn_blocking(move || runtime.shutdown_timeout(Duration::ZERO))
            .await
            .is_ok();
    }
    if let Some(job) = app.jj_pending_load.as_ref() {
        job.revoke();
        cleanup &= tokio::time::timeout(Duration::from_secs(30), job.collector().collect())
            .await
            .is_ok();
    }
    if let Some(future) = execution.as_mut() {
        cleanup &= tokio::time::timeout(Duration::from_secs(30), future.as_mut())
            .await
            .is_ok();
    }
    if producer.is_some() && !finished {
        cleanup &= matches!(
            tokio::time::timeout(
                Duration::from_secs(30),
                raccogli80(&mut producer, &mut finished)
            )
            .await,
            Ok(Ok(_))
        );
    }
    cleanup &= workers86().is_ok_and(|count| count == 0);
    if let Some((pid, identity)) = origin {
        cleanup &= identity86(pid).is_ok_and(|observed| observed != Some(identity));
    }
    if !cleanup {
        eprintln!(
            "JJ_LOAD_REALE88:fixture_preservata={}; raccolta incerta",
            fixture.keep().display()
        );
        std::mem::forget((app, producer, execution));
        return Err(io::Error::other("load88 raccolta non provata").into());
    }
    if let Err(error) = result {
        eprintln!(
            "JJ_LOAD_REALE88:fixture_preservata={}; scenario fallito dopo raccolta",
            fixture.keep().display()
        );
        return Err(error);
    }
    fixture.close()?;
    match mode {
        0 => eprintln!(
            "JJ_LOAD_REALE88:sequenza_completa=actual_refresh_native_flock_child_two_readers_producer_drop_app_recipe_output_eof_owner_ack_replay_successor"
        ),
        1 => eprintln!(
            "JJ_LOAD_RUNTIME_REALE88:sequenza_completa=actual_refresh_native_flock_producer_drop_origin_runtime_loss_app_recipe_output_eof_owner_ack_successor"
        ),
        2 => eprintln!(
            "JJ_LOAD_LAST_OWNER_REALE88:sequenza_completa=actual_refresh_native_flock_producer_recipe_ticket_app_drop_child_readers_absent_before_unlock"
        ),
        _ => unreachable!(),
    }
    Ok(())
}
