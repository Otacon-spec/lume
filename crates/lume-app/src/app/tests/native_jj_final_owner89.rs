//! Ultimo owner del load JJ, worker ancora vivo: l'uscita dell'app raccoglie
//! il JoinHandle reale fuori dal thread UI e il child non resta orfano.
//! Linux, binario JJ esplicito via LUME_JJ; non GUI, main close né azioni.
use super::admitted86::{flock86, identity86, waiting86};
use super::*;
use crate::jj_actions;
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ultimo_owner_load_worker_e_child_raccolti_dall_uscita_reale() -> Esito80 {
    let Some(program) = std::env::var_os("LUME_JJ").map(PathBuf::from) else {
        eprintln!("SALTATO: manca LUME_JJ assoluto per il binario jj reale");
        return Ok(());
    };
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("jj reale non assoluto/file").into());
    }
    eprintln!(
        "JJ_FINAL_OWNER_REALE89:server_sha256={}",
        crate::aggiornamento::esadecimale(&Sha256::digest(std::fs::read(&program)?))
    );
    let dir = program
        .parent()
        .ok_or_else(|| io::Error::other("jj reale senza cartella"))?;
    let cli = lume_jj::JjCli::locate_in(&[dir.to_path_buf()])
        .ok_or_else(|| io::Error::other("locator esplicito jj assente"))?;
    assert_eq!(cli.program(), program);

    let fixture = tempfile::Builder::new()
        .prefix("lume-jj-final89-città-")
        .tempdir()?;
    let home = fixture.path().join("home");
    std::fs::create_dir(&home)?;
    let root = fixture.path().join("repo");
    std::fs::create_dir(&root)?;
    if let Err(error) = setup80(&program, &home, &root, &["git", "init"]) {
        let _ = fixture.keep();
        return Err(error);
    }
    let file = root.join("città.txt");
    std::fs::write(&file, "uscita finale 😀\n")?;
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".jj/working_copy/working_copy.lock"))?;
    flock86(&held, libc::LOCK_EX | libc::LOCK_NB)?;

    let cancel = Arc::new(AtomicBool::new(false));
    let mut app = app();
    app.jj_workspace = Some(root.clone());
    app.jj_gen = 1;
    app.jj_cancel = cancel.clone().into();
    let generation = app.jj_gen;
    let (pending, permit) =
        jj_actions::load_admission::Pending::with_final(generation, app.jj_final.clone());
    assert!(permit.admit());
    let worker_root = root.clone();
    let worker_cancel = cancel.clone();
    let worker = permit
        .start(move || jj_actions::load_with(Some(cli), &worker_root, &worker_cancel))
        .ok_or("worker load non avviato")?;

    // Attesa dell'evento reale: il child jj è bloccato sul nostro flock.
    let start = Instant::now();
    let origin = loop {
        if let Some(id) = waiting86(&held, &program)? {
            break id;
        }
        if start.elapsed() > Duration::from_secs(10) {
            cancel.store(true, Ordering::Release);
            drop(held);
            drop(worker);
            return Err(io::Error::other("child jj bloccato non osservato").into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };

    // Ultimo owner: Pending, Permit e Handle del producer spariscono mentre
    // il worker è ancora dentro il child jj. Resta solo il registro dell'app.
    drop(worker);
    drop(permit);
    drop(pending);
    assert!(
        app.jj_final.has_pending(),
        "il registro finale deve possedere il worker ammesso"
    );

    // L'uscita segnala la cancellazione e deve raccogliere lo stesso worker
    // anche se nessun Pending/Collector della subscription è sopravvissuto.
    let close = app.esci(None);
    assert!(
        app.search_exit.is_some(),
        "l'uscita deve attendere il worker anche senza Pending"
    );
    let epoch = app.close_epoch;
    let mut exit = iced_runtime::task::into_stream(close).ok_or("task uscita assente")?;
    let action = tokio::time::timeout(Duration::from_secs(60), exit.next())
        .await
        .map_err(|_| io::Error::other("raccolta finale entro il termine"))?
        .ok_or("esito uscita assente")?;
    let iced_runtime::Action::Output(message) = action else {
        return Err(io::Error::other("output uscita inatteso").into());
    };
    assert!(
        matches!(&message, Message::SearchExitReady(e, true) if *e == epoch),
        "uscita non confermata dopo il join del worker: {message:?}"
    );
    assert_eq!(
        app.jj_final.joined(),
        1,
        "l'uscita deve aver atteso il JoinHandle del worker"
    );
    assert!(!app.jj_final.has_pending());
    // Il worker ha raccolto il child prima di uscire: nessuna identità viva.
    assert_ne!(
        identity86(origin.0)?,
        Some(origin.1.clone()),
        "child jj ancora vivo dopo l'uscita"
    );
    assert_eq!(std::fs::read_to_string(&file)?, "uscita finale 😀\n");
    drop(held);
    eprintln!(
        "JJ_FINAL_OWNER_REALE89:sequenza_completa=refresh_nativo_flock_ultimo_owner_perso_esci_worker_joined_child_raccolto"
    );
    fixture.close()?;
    Ok(())
}
