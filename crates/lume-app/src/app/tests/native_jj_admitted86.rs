//! Future azione realmente ammessa/child JJ flockblocked; producer Drop.
//! Collector subscription e gate uscita reali; finestra non eseguita,
//! lastOwner/runtime loss non qualificati.
use super::*;
use iced_runtime::futures::subscription::{Recipe, Tracker, into_recipes};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    time::Instant,
};

pub(super) fn workers86() -> io::Result<usize> {
    let mut n = 0;
    for entry in std::fs::read_dir("/proc/self/task")? {
        match std::fs::read_to_string(entry?.path().join("comm")) {
            Ok(name) if name.trim() == "lume-jj-pipe" => n += 1,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(n)
}
pub(super) fn children86() -> io::Result<HashSet<u32>> {
    let mut pids = HashSet::new();
    for entry in std::fs::read_dir("/proc/self/task")? {
        match std::fs::read_to_string(entry?.path().join("children")) {
            Ok(text) => {
                for pid in text.split_whitespace() {
                    pids.insert(pid.parse().map_err(io::Error::other)?);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(pids)
}
pub(super) fn identity86(pid: u32) -> io::Result<Option<String>> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => {
            // Dopo l'ultima ')' field3; starttime è field22. Non parsing del
            // nome/argomenti né azioni su PID ricostruiti.
            let (_, rest) = text
                .rsplit_once(')')
                .ok_or_else(|| io::Error::other("stat processo owned senza delimitatore"))?;
            Ok(Some(
                rest.split_whitespace()
                    .nth(19)
                    .ok_or_else(|| io::Error::other("starttime processo owned assente"))?
                    .to_owned(),
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
pub(super) fn waiting86(file: &File, program: &Path) -> io::Result<Option<(u32, String)>> {
    let metadata = file.metadata()?;
    let dev = metadata.dev();
    let children = children86()?;
    // Non registrare /proc/locks grezzo: selezionare solo l'inode posseduto e
    // il child di questo processo col vero exe atteso.
    for line in std::fs::read_to_string("/proc/locks")?.lines() {
        let mut tokens = line.split_whitespace();
        if tokens.next().is_none()
            || tokens.next() != Some("->")
            || tokens.next() != Some("FLOCK")
            || tokens.next() != Some("ADVISORY")
            || tokens.next() != Some("WRITE")
        {
            continue;
        }
        let Some(pid) = tokens.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(device) = tokens.next() else {
            continue;
        };
        let mut address = device.split(':');
        let major = address.next().and_then(|s| u32::from_str_radix(s, 16).ok());
        let minor = address.next().and_then(|s| u32::from_str_radix(s, 16).ok());
        let inode = address.next().and_then(|s| s.parse::<u64>().ok());
        if major != Some(libc::major(dev))
            || minor != Some(libc::minor(dev))
            || inode != Some(metadata.ino())
            || !children.contains(&pid)
        {
            continue;
        }
        match std::fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(exe) if exe == program => {
                if let Some(start) = identity86(pid)? {
                    return Ok(Some((pid, start)));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}
pub(super) fn flock86(file: &File, operation: i32) -> io::Result<()> {
    #[allow(unsafe_code)]
    // SAFETY: il fd appartiene alla File viva della fixture; `operation` usa
    // soltanto i flag flock definiti da libc, senza puntatori o memoria grezza.
    let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub(super) fn hash86(recipe: &dyn Recipe<Output = Message>) -> u64 {
    use std::hash::Hasher;
    let mut h = iced_runtime::futures::subscription::Hasher::default();
    recipe.hash(&mut h);
    h.finish()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "JJ admitted producer Drop reale Linux: test-app-jj-admitted-real"]
async fn producer_ammesso_perso_collector_app_raccoglie_stessa_azione_reale() -> Esito80 {
    scenario86(0).await?;
    scenario86(1).await?;
    scenario86(2).await?;
    scenario86(3).await
}
async fn scenario86(mode: u8) -> Esito80 {
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
        .prefix("lume-jj86-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("repo");
    std::fs::create_dir(&root)?;
    if let Err(e) = setup80(&program, &home, &root, &["git", "init"]) {
        let _ = fixture.keep();
        return Err(e);
    }
    let file = root.join("città.txt");
    std::fs::write(&file, "prima 😀\n")?;
    let mut app = app();
    app.set_project_root(root.clone());
    let mut bootstrap = std::mem::take(&mut app.pending_tool_requests);
    bootstrap.extend(app.registry.set_active_panel(None));
    assert!(bootstrap.iter().all(|(id, _)| *id != tools::jj::ID));
    let mut producer = None;
    let mut maybe_started = false;
    let mut finished = false;
    let mut collector: Option<futures::future::BoxFuture<'static, ()>> = None;
    let mut tracker = Tracker::new();
    let (sender, mut receiver) = futures::channel::mpsc::channel::<Message>(2);
    let mut sender = Some(sender);
    let mut collector_finished = false;
    let mut lock = None;
    let mut origin = None;
    let mut worker_runtime = if mode == 3 {
        Some(tokio::runtime::Builder::new_current_thread().build()?)
    } else {
        None
    };
    let scenario = tokio::time::timeout(
        Duration::from_secs(60),
        std::panic::AssertUnwindSafe(async {
            assert_eq!(workers86()?, 0);
            app.apply_activity_intent(ActivityBarIntent::Click(tools::jj::ID));
            avvia80(
                app.drain_tool_requests(),
                &mut producer,
                &mut maybe_started,
                &mut finished,
            )?;
            let messages = raccogli80(&mut producer, &mut finished).await?;
            snapshot80(&mut app, messages, "", false, None);
            const DESCRIPTION: &str = "azione ammessa senza producer86 città";
            let _ = app.update(Message::Tool(
                tools::jj::ID,
                ToolEvent::new(tools::jj::Event::Message(DESCRIPTION.into())),
            ));
            let baseline: HashSet<_> = into_recipes(app.subscription())
                .iter()
                .map(|r| hash86(&**r))
                .collect();
            let path = root.join(".jj/working_copy/working_copy.lock");
            let held = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)?;
            #[allow(unsafe_code)]
            // SAFETY: `held` possiede il fd valido per tutta la chiamata;
            // `F_GETFD` legge i flag senza mutare descriptor o memoria.
            let flags = unsafe { libc::fcntl(held.as_raw_fd(), libc::F_GETFD) };
            assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
            flock86(&held, libc::LOCK_EX | libc::LOCK_NB)?;
            lock = Some(held);
            let task = app.update(Message::Tool(
                tools::jj::ID,
                ToolEvent::new(tools::jj::Event::Describe),
            ));
            // Recipe sampled PRIMA dell'ammissione, come può fare Iced: il
            // collector non può dipendere da un successivo update del producer.
            let mut recipes: Vec<_> = into_recipes(app.subscription())
                .into_iter()
                .filter(|r| !baseline.contains(&hash86(&**r)))
                .collect();
            avvia80(task, &mut producer, &mut maybe_started, &mut finished)?;
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
                    return Err(
                        io::Error::other("producer JJ86 terminato prima del vero flock").into(),
                    );
                }
                if let Some(id) = waiting86(lock.as_ref().unwrap(), &program)?
                    && workers86()? == 2
                {
                    origin = Some(id);
                    break;
                }
                if start.elapsed() > Duration::from_secs(10) {
                    return Err(
                        io::Error::other("child JJ/2reader non osservati durante flock").into(),
                    );
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            // Nessun EOF/risultato: Drop della vera stream mentre il child è vivo.
            drop(producer.take());
            if let Some(runtime) = worker_runtime.take() {
                tokio::task::spawn_blocking(move || runtime.shutdown_timeout(Duration::ZERO)).await?;
                assert_eq!(workers86()?, 2);
                let id = origin.as_ref().unwrap();
                assert_eq!(identity86(id.0)?, Some(id.1.clone()));
                eprintln!("JJ_APP_RUNTIME_REALE86:runtime_originario_distrutto_child_e_due_lettori_ancora_vivi=confermato");
            }
            if mode == 2 {
                // Tutti i collector del vecchio app vengono rilasciati.
                // L'app vuoto sostitutivo serve solo al cleanup della fixture.
                recipes.clear();
                drop(std::mem::replace(&mut app, super::app()));
                let id = origin.as_ref().unwrap();
                let deadline = Instant::now();
                loop {
                    if workers86()? == 0 && identity86(id.0)? != Some(id.1.clone()) {
                        break;
                    }
                    if deadline.elapsed() > Duration::from_secs(10) {
                        return Err(io::Error::other("Drop app: child o lettori JJ ancora vivi").into());
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let other = File::open(&path)?;
                assert_eq!(flock86(&other, libc::LOCK_EX | libc::LOCK_NB).unwrap_err().kind(), io::ErrorKind::WouldBlock);
                assert!(!lume_jj::quarantine_has_children());
                flock86(lock.as_ref().unwrap(), libc::LOCK_UN)?;
                drop(lock.take());
                assert_eq!(std::fs::read_to_string(&file)?, "prima 😀\n");
                assert_eq!(setup80(&program, &home, &root, &["log", "--no-graph", "-r", "@", "-T", "description"])?, "");
                eprintln!("JJ_APP_DROP_REALE86:child_e_lettori_assenti_lock_ancora_posseduto=confermato");
                return Ok(());
            }
            if matches!(mode, 1 | 3) {
                let close_task = app.esci(Some(iced::window::Id::unique()));
                let epoch = app.close_epoch;
                assert!(app.search_exit.is_some());
                let _early = app.update(Message::SearchExitReady(epoch, true));
                assert!(app.search_exit.is_some(), "ACK prematuro mentre jj attende flock");
                avvia80(close_task, &mut producer, &mut maybe_started, &mut finished)?;
                let mut messages = raccogli80(&mut producer, &mut finished).await?;
                assert_eq!(messages.len(), 1);
                assert!(matches!(&messages[0], Message::SearchExitReady(e, true) if *e == epoch));
                assert_eq!(workers86()?, 0);
                let id = origin.as_ref().unwrap();
                assert_ne!(identity86(id.0)?, Some(id.1.clone()));
                app.jj_pending_action.as_ref().expect("owner ammesso conservato").inspect_completion(|result| {
                    assert_eq!(result.expect("completion reale raccolta").as_ref().unwrap_err(), crate::jj_actions::CANCELLED);
                });
                let _window_close_not_executed = app.update(messages.pop().unwrap());
                assert!(app.search_exit.is_none());
                // Il lock resta posseduto fino all’ACK: cancel, non rilascio
                // fixture, ha terminato jj e i suoi lettori.
                let other = File::open(&path)?;
                assert_eq!(flock86(&other, libc::LOCK_EX | libc::LOCK_NB).unwrap_err().kind(), io::ErrorKind::WouldBlock);
                flock86(lock.as_ref().unwrap(), libc::LOCK_UN)?;
                drop(lock.take());
                assert_eq!(std::fs::read_to_string(&file)?, "prima 😀\n");
                assert_eq!(setup80(&program, &home, &root, &["log", "--no-graph", "-r", "@", "-T", "description"])?, "");
                eprintln!("JJ_APP_CLOSE_REALE86:ack_dopo_cancel_reap_readers_join_lock_ancora_posseduto=confermato");
                return Ok(());
            }
            assert_eq!(
                recipes.len(),
                1,
                "app non conserva recipe di raccolta per azione JJ realmente ammessa"
            );
            let mut executions = tracker.update(std::iter::once(recipes.pop().unwrap()), sender.take().unwrap());
            assert_eq!(executions.len(), 1);
            collector = executions.pop();
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(30),
                    collector.as_mut().unwrap().as_mut()
                )
                .await
                .is_err(),
                "collector non inventa risultato/avvia un successore mentre native lock held"
            );
            assert_eq!(workers86()?, 2);
            let id = origin.as_ref().unwrap();
            assert_eq!(identity86(id.0)?, Some(id.1.clone()));
            let other = File::open(&path)?;
            assert_eq!(
                other.metadata()?.ino(),
                lock.as_ref().unwrap().metadata()?.ino()
            );
            assert_eq!(
                flock86(&other, libc::LOCK_EX | libc::LOCK_NB)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::WouldBlock
            );
            flock86(lock.as_ref().unwrap(), libc::LOCK_UN)?;
            drop(lock.take());
            collector.as_mut().unwrap().as_mut().await;
            collector_finished = true;
            drop(collector.take());
            let mut messages = Vec::new();
            while let Some(m) = receiver.next().await {
                messages.push(m);
                if messages.len() > 1 {
                    return Err(io::Error::other("collector JJ86 output duplicato").into());
                }
            }
            // Vera execution Tracker + output/EOF del sink prima delle assert.
            assert_eq!(messages.len(), 1);
            assert!(matches!(&messages[0], Message::JjActionCollected(signal) if app.jj_pending_action.as_ref().is_some_and(|job| job.matches(signal))));
            assert_eq!(workers86()?, 0);
            assert_ne!(identity86(id.0)?, Some(id.1.clone()));
            let generation = app.jj_gen;
            avvia80(
                app.update(messages.pop().unwrap()),
                &mut producer,
                &mut maybe_started,
                &mut finished,
            )?;
            let messages = raccogli80(&mut producer, &mut finished).await?;
            let notice = app
                .notice
                .as_ref()
                .ok_or_else(|| io::Error::other("collector non consegna esito JJ86"))?;
            assert_eq!(notice.1.kind, NoticeKind::Info);
            assert!(notice.1.text.starts_with("Jujutsu - Descrizione:"));
            assert_ne!(notice.1.text, "Jujutsu - Descrizione: annullato", "Info di cancellazione non è successo nativo");
            let nonce = notice.0;
            snapshot80(&mut app, messages, DESCRIPTION, false, Some(nonce));
            assert!(!app.jj_busy);
            assert_eq!(app.jj_gen, generation + 1);
            assert_eq!(
                setup80(
                    &program,
                    &home,
                    &root,
                    &["log", "--no-graph", "-r", "@", "-T", "description"]
                )?,
                format!("{DESCRIPTION}\n")
            );
            assert_eq!(std::fs::read_to_string(&file)?, "prima 😀\n");
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        })
        .catch_unwind(),
    )
    .await;
    let result: Esito80 = match scenario {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err(io::Error::other("scenario JJ86 panic").into()),
        Err(_) => Err(io::Error::other("scenario JJ86 deadline").into()),
    };
    // Indipendente dal catch: stesso source/invocazione; niente kill da PID
    // osservato, nuovo worker o drain concorrente. Fixture incerta conservata.
    app.jj_cancel.store(true, Ordering::SeqCst);
    drop(lock.take());
    let mut cleanup_ok = true;
    if let Some(runtime) = worker_runtime.take() {
        cleanup_ok &= tokio::task::spawn_blocking(move || {
            runtime.shutdown_timeout(Duration::ZERO);
        })
        .await
        .is_ok();
    }
    if let Some(collector) = collector.as_mut()
        && !collector_finished
    {
        let collected = tokio::time::timeout(Duration::from_secs(30), collector.as_mut()).await;
        collector_finished = collected.is_ok();
        cleanup_ok &= collector_finished;
    }
    if producer.is_some() && !finished {
        cleanup_ok &= matches!(
            tokio::time::timeout(
                Duration::from_secs(30),
                raccogli80(&mut producer, &mut finished)
            )
            .await,
            Ok(Ok(_))
        );
    }
    // Nel pre-fix handle perso: osservazione assenza NON qualifica join.
    let deadline = Instant::now();
    loop {
        match workers86() {
            Ok(0) => break,
            Ok(_) if deadline.elapsed() < Duration::from_secs(10) => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            _ => {
                cleanup_ok = false;
                break;
            }
        }
    }
    if let Some((pid, start)) = origin {
        cleanup_ok &= identity86(pid).is_ok_and(|observed| observed != Some(start));
    }
    if result.is_err() || !cleanup_ok {
        if !cleanup_ok {
            std::mem::forget((collector, producer, app));
        }
        let residue = fixture.keep();
        eprintln!(
            "JJ_APP_ADMITTED_REALE86:fixture_preservata={}; raccolta piena non affermata nel fallimento",
            residue.display()
        );
        return result.and(Err(io::Error::other("raccolta JJ86 incerta").into()));
    }
    fixture.close()?;
    if mode == 3 {
        eprintln!(
            "JJ_APP_RUNTIME_REALE86:sequenza_completa=private_runtime_shutdown_live_native_flock_exit_collector_join_child_readers_ack_lock_held"
        );
    } else if mode == 1 {
        eprintln!(
            "JJ_APP_CLOSE_REALE86:sequenza_completa=native_flock_task_drop_exit_gate_cancel_reap_two_readers_join_ack_no_window_execution"
        );
    } else if mode == 2 {
        eprintln!(
            "JJ_APP_DROP_REALE86:sequenza_completa=native_flock_producer_drop_all_app_collectors_drop_cancel_child_readers_absent_no_runtime_shutdown"
        );
    } else {
        eprintln!(
            "JJ_APP_ADMITTED_REALE86:sequenza_completa=native_describe_owned_flock_two_readers_actual_task_drop_actual_app_recipe_collect_same_job_notice_reload"
        );
    }
    Ok(())
}
