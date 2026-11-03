//! Receipt vecchie/membership mutata dopo effetti reali; non native GUI.
use super::*;

fn attende_bytes(path: &std::path::Path, expected: &[u8]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if std::fs::read(path).is_ok_and(|bytes| bytes == expected) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "scrittura worker non osservata"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn vecchia_ricevuta_non_libera_job_successivo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let mut first_stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let first = runtime.block_on(first_stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(first_ready)) = first else {
        panic!("Ready del worker assente");
    };
    let ready_replay = first_ready.clone();
    let _ = app.update(Message::SaveReady(first_ready));
    attende_bytes(&path, b"Xprima");
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    ));
    let _ = app.update(Message::SaveReady(ready_replay));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    let first_done = runtime.block_on(first_stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveCollected(first_receipt)) = first_done else {
        panic!("Done del primo job assente");
    };
    let receipt_replay = first_receipt.clone();
    let _ = app.update(Message::SaveCollected(first_receipt));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));

    let mut next_stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let next_job = app.save_job.clone().unwrap();
    let next_ready = runtime.block_on(next_stream.next()).unwrap();
    let iced_runtime::Action::Output(next_ready @ Message::SaveReady(_)) = next_ready else {
        panic!("Ready del job successivo assente");
    };
    let _ = app.update(next_ready);
    let epoch = app.scritture.guarda(None).epoca;
    let _ = app.update(Message::SaveCollected(receipt_replay));
    assert_eq!(app.save_job.as_ref(), Some(&next_job));
    assert_eq!(app.scritture.guarda(None).epoca, epoch);
    let next_done = runtime.block_on(next_stream.next()).unwrap();
    let iced_runtime::Action::Output(next_done) = next_done else {
        panic!("Done del job successivo assente");
    };
    let _ = app.update(next_done);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "XYprima");
}

#[test]
fn non_salvare_raccoglie_fallback_dopo_pubblicazione_senza_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let mut stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let job = app.save_job.clone().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let first = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = first else {
        panic!("Ready del worker ordinario assente");
    };
    let _ = app.update(Message::SaveReady(ready));
    attende_bytes(&path, b"Xprima");
    drop(stream);
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    assert!(app.riquadro_chiusura().is_some());
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    let mut fallback = fallback(&app, &job);
    let result = futures::executor::block_on(fallback.next()).unwrap();
    let _ = app.update(result);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.riquadro_chiusura().is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
}

#[test]
fn non_salvare_revoca_job_freddo_anche_con_task_ancora_viva() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id)); // NON droppare: NonSalvare deve revocare il pending.
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    assert!(app.riquadro_chiusura().is_some());
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let first = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(first) = first else {
        panic!("ricevuta assente");
    };
    let _ = app.update(first);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.riquadro_chiusura().is_none());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "prima",
        "NonSalvare revoca prima dell'ammissione: nessuna scrittura"
    );
}

#[test]
fn non_salvare_revoca_save_ordinario_granted_prima_del_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let mut stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let job = app.save_job.clone().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del save ordinario assente");
    };
    let pause = job.pause().unwrap();
    let _ = app.update(Message::SaveReady(ready));
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    pause.finish(false);
    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done) = done else {
        panic!("Done dopo la revoca assente");
    };
    let _ = app.update(done);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.chiusura.is_none());
}

#[test]
fn non_salvare_dopo_commit_started_raccoglie_save_e_chiude_scheda() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    chiudi_scheda(&mut app, id);
    let task = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    let job = app.save_job.clone().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    job.set_commit_started_hook(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del salvataggio assente");
    };
    let _ = app.update(Message::SaveReady(ready));
    started_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("worker non ha attraversato CommitStarted");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");

    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.groups.contains(id));
    assert!(app.riquadro_chiusura().is_some());
    release_tx.send(()).unwrap();

    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) = done else {
        panic!("Done del commit già avviato assente");
    };
    let _ = app.update(done);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
    assert!(!app.groups.contains(id));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.riquadro_chiusura().is_none());
}

#[test]
fn shutdown_runtime_dopo_commit_started_raccoglie_save_autorizzato_dall_app() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    let job = app.save_job.clone().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    job.set_commit_started_hook(move || {
        started_tx.send(()).unwrap();
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    });
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del save app assente");
    };
    let _ = app.update(Message::SaveReady(ready));
    started_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("il save dell'app non ha attraversato CommitStarted");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");

    drop(stream);
    drop(app);
    drop(job);
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        shutdown_tx.send(()).unwrap();
        drop(runtime);
        finished_tx.send(()).unwrap();
    });
    shutdown_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    assert!(
        finished_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err(),
        "lo shutdown non deve finire mentre il worker è ancora fermo"
    );

    release_tx.send(()).unwrap();
    finished_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    shutdown.join().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "non deve restare un temporaneo del save");
}

#[test]
fn uscita_finestra_attende_receipt_dopo_commit_started() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (_id, path) = prepara(&mut app, dir.path());
    let window = iced::window::Id::unique();
    let _ = app.update(Message::WindowCloseRequested(window));
    assert!(app.riquadro_chiusura().is_some());
    let task = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    let job = app.save_job.clone().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    job.set_commit_started_hook(move || {
        started_tx.send(()).unwrap();
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    });
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del save per l'uscita assente");
    };
    let _ = app.update(Message::SaveReady(ready));
    started_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("il save di uscita non ha attraversato CommitStarted");
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.close_after_save.is_some());
    assert!(app.riquadro_chiusura().is_some());

    release_tx.send(()).unwrap();
    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) = done else {
        panic!("receipt del save non ricevuta prima dell'uscita");
    };
    let close = app.update(done);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.chiusura.is_none());
    assert!(app.close_after_save.is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");

    let mut close_actions = iced_runtime::task::into_stream(close).unwrap();
    let Some(iced_runtime::Action::Window(iced_runtime::window::Action::Close(actual))) =
        runtime.block_on(close_actions.next())
    else {
        panic!("la chiusura finestra non segue la receipt raccolta");
    };
    assert_eq!(actual, window);
}

#[cfg(unix)]
#[test]
fn uscita_immediata_dopo_commit_started_lascia_esito_disco_verificabile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crash.txt");
    std::fs::write(&path, "prima").unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("app::tests::save_owned_tests::stale_tests::worker_child_esce_dopo_commit_started")
        .arg("--nocapture")
        .env("LUME_TEST_SAVE_CRASH_CHILD", "1")
        .env("LUME_TEST_SAVE_CRASH_ROOT", dir.path())
        .output()
        .unwrap();
    assert_eq!(
        child.status.code(),
        Some(86),
        "child deve terminare dal hook dopo CommitStarted: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        names
            .iter()
            .any(|name| name.to_string_lossy().starts_with(".lume-save-")),
        "uscita improvvisa prima del rename lascia il temporaneo da recuperare"
    );
}

#[cfg(unix)]
#[test]
fn worker_child_esce_dopo_commit_started() {
    if std::env::var_os("LUME_TEST_SAVE_CRASH_CHILD").is_none() {
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os("LUME_TEST_SAVE_CRASH_ROOT").unwrap());
    let mut app = app();
    let (id, _) = prepara(&mut app, &root);
    let task = app.update(Message::SaveDocFixture(id));
    let job = app.save_job.clone().unwrap();
    job.set_commit_started_hook(|| std::process::exit(86));
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready prima del crash controllato assente");
    };
    drop(app.update(Message::SaveReady(ready)));
    let _ = runtime.block_on(stream.next());
    panic!("il worker deve terminare il processo dopo CommitStarted");
}

#[test]
fn non_salvare_raccoglie_abbandono_freddo_senza_io() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    let job = app.save_job.clone().unwrap();
    drop(task);
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    assert!(app.riquadro_chiusura().is_some());
    let mut fallback = fallback(&app, &job);
    let result = futures::executor::block_on(fallback.next()).unwrap();
    let _ = app.update(result);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.riquadro_chiusura().is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
}

#[test]
fn membership_cambiata_dopo_scrittura_reale_dichiara_effetto_non_ack() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let mut stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let first = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(first) = first else {
        panic!("ricevuta del worker assente");
    };
    let Message::SaveReady(ready) = first else {
        panic!("Ready del worker assente");
    };
    let _ = app.update(Message::SaveReady(ready));
    attende_bytes(&path, b"Xprima");
    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done) = done else {
        panic!("Done del worker assente");
    };
    // Mutation controllata del modello, NON gesto GUI che scavalca il guard.
    app.workspace.close(id).unwrap();
    let new = app.workspace.open(Some(path.clone()), "nuovo");
    assert_ne!(new, id);
    let doc = app.workspace.get_mut(new).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "Z", doc.rope().len_chars()),
    ));
    let _ = app.update(done);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.workspace.get(new).unwrap().is_dirty());
    assert_eq!(app.workspace.get(new).unwrap().rope().to_string(), "Znuovo");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Il file è stato scritto")
    );
}
