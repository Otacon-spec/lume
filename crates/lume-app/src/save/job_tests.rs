use super::*;
use futures::{Future, StreamExt, task::ArcWake};
use lume_core::{DocId, Document};
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

fn ok<T, E: fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("fixture non riuscita: {error:?}"),
    }
}
fn some<T>(value: Option<T>) -> T {
    match value {
        Some(value) => value,
        None => panic!("valore della fixture assente"),
    }
}
fn runtime() -> tokio::runtime::Runtime {
    ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build())
}
fn request(path: &Path) -> Request {
    let mut doc = Document::from_str("\u{feff}testo\r\n", Some(path.to_owned()), DocId(7));
    Request {
        ticket: Ticket {
            document: DocId(7),
            sequence: 1,
        },
        path: path.to_owned(),
        snapshot: doc.snapshot_for_save(),
        note: None,
        aliases: None,
        expected_fingerprint: None,
    }
}
fn receipt(receipt: &Receipt) -> Completion {
    some(receipt.take())
}

#[test]
fn collector_freddo_non_avvia_io_e_abbandono_lo_sveglia() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("freddo.txt");
    let job = Job::new(request(&path));
    let delivery = job.delivery();
    let rt = runtime();
    rt.block_on(async {
        let collector = job.collect();
        tokio::pin!(collector);
        assert!(futures::poll!(&mut collector).is_pending());
        assert!(!path.exists());
        drop(delivery);
        let result = ok(tokio::time::timeout(std::time::Duration::from_secs(3), collector).await);
        assert!(job.matches(&result));
        assert!(
            receipt(&result)
                .result
                .err()
                .is_some_and(|s| s.contains("non avviato"))
        );
        assert!(!path.exists());
    });
}

#[test]
fn drop_prepoll_non_avvia_io_ne_richiede_runtime() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("freddo.txt");
    let job = Job::new(request(&path));
    drop(job.delivery());
    let result = futures::executor::block_on(job.collect());
    assert!(receipt(&result).result.is_err());
    assert!(!path.exists());
}

#[test]
fn primo_poll_senza_runtime_rifiuta_senza_scrivere() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("freddo.txt");
    let job = Job::new(request(&path));
    let result = futures::executor::block_on(job.delivery());
    assert!(
        receipt(&result)
            .result
            .err()
            .is_some_and(|s| s.contains("Runtime"))
    );
    assert!(!path.exists());
}

#[test]
fn delivery_abbandonata_dopo_ammissione_nega_il_commit_prepubblicazione() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("scritto.txt");
    let job = Job::new(request(&path));
    let rt = runtime();
    rt.block_on(async {
        // Saturazione posseduta del solo blocking slot; nessun fake writer.
        // Il save è ammesso/accodato ma non può pubblicare prima del release.
        let (entered, admitted) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            ok(entered.send(()));
            ok(gate.recv_timeout(std::time::Duration::from_secs(5)));
        });
        ok(admitted.recv_timeout(std::time::Duration::from_secs(3)));
        let mut delivery = Box::pin(job.delivery());
        assert!(futures::poll!(&mut delivery).is_pending());
        drop(delivery);
        assert!(!path.exists());
        ok(release.send(()));
        ok(blocker.await);
        let result =
            ok(tokio::time::timeout(std::time::Duration::from_secs(3), job.collect()).await);
        assert!(job.matches(&result));
        assert!(receipt(&result).result.is_err());
        assert!(!path.exists());
    });
}

#[test]
fn fallback_senza_lease_nega_il_commit_prima_del_rientro_runtime() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("fallback.txt");
    let job = Job::new(request(&path));
    let rt = runtime();
    let mut delivery = Box::pin(job.delivery());
    {
        let _entered = rt.enter();
        // La prima future poll ammette e invia davvero il blocking job.
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let _ = delivery.as_mut().poll(&mut cx);
    }
    drop(delivery);
    let result = futures::executor::block_on(job.collect());
    assert!(receipt(&result).result.is_err());
    assert!(!path.exists());
}

#[test]
fn ricevuta_clonata_ha_un_solo_consumo_e_job_estraneo_non_la_accetta() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("unico.txt");
    let job = Job::new(request(&path));
    let other = Job::new(request(&path)); // Anche stesso ticket non è stessa origine.
    let rt = runtime();
    rt.block_on(async {
        let mut delivery = job.delivery();
        let Some(Event::Ready(ready)) = delivery.next().await else {
            panic!("Ready del save originale assente");
        };
        assert!(job.matches_ready(&ready));
        assert!(ready.grant());
        let Some(Event::Done(result)) = delivery.next().await else {
            panic!("Done del save originale assente");
        };
        let copy = result.clone();
        assert!(job.matches(&result));
        assert!(!other.matches(&result));
        assert!(receipt(&result).result.is_ok());
        assert!(copy.take().is_none());
        assert!(job.collect().await.take().is_none());
        drop(other.delivery());
    });
}

#[test]
fn collector_concorrenti_condividono_esito_e_errore_del_writer_vero() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("manca").join("impossibile.txt");
    let job = Job::new(request(&path));
    let rt = runtime();
    rt.block_on(async {
        let (a, b) = futures::join!(job.delivery(), job.collect());
        assert!(Arc::ptr_eq(&a.0, &b.0));
        assert!(receipt(&a).result.is_err());
        assert!(b.take().is_none());
        assert!(!path.exists());
        assert!(!dir.path().join("manca").exists());
    });
}

#[test]
fn stream_fallback_emette_una_ricevuta_e_poi_eof() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("freddo.txt");
    let job = Job::new(request(&path));
    drop(job.delivery());
    futures::executor::block_on(async {
        let stream = completed(&job);
        futures::pin_mut!(stream);
        let Some(Event::Done(result)) = stream.next().await else {
            panic!("il collector deve emettere una ricevuta terminale");
        };
        assert!(receipt(&result).result.is_err());
        assert!(stream.next().await.is_none());
    });
}

#[test]
fn revoca_worker_ammesso_prima_del_commit_aborta_senza_pubblicare() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("scritto.txt");
    let job = Job::new(request(&path));
    job.discard_pending(Some(DocId(8))); // Non il proprietario: nessuna revoca.
    let rt = runtime();
    rt.block_on(async {
        let (entered, admitted) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            ok(entered.send(()));
            ok(gate.recv_timeout(std::time::Duration::from_secs(5)));
        });
        ok(admitted.recv_timeout(std::time::Duration::from_secs(3)));
        let mut delivery = Box::pin(job.delivery());
        assert!(futures::poll!(&mut delivery).is_pending());
        assert!(job.running());
        job.discard_pending(Some(DocId(7))); // Già ammesso: raccogliere.
        job.discard_pending(None);
        drop(delivery);
        ok(release.send(()));
        ok(blocker.await);
        let result =
            ok(tokio::time::timeout(std::time::Duration::from_secs(3), job.collect()).await);
        assert!(receipt(&result).result.is_err());
        assert!(!path.exists());
    });
}

struct WakeOutside {
    job: Job,
    calls: AtomicUsize,
}
impl ArcWake for WakeOutside {
    fn wake_by_ref(this: &Arc<Self>) {
        assert!(this.job.0.phase.try_lock().is_ok(), "callback sotto mutex");
        this.calls.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
fn wake_abbandono_non_tiene_mutex_e_non_perde_consumer_silenzioso() {
    let dir = ok(tempfile::tempdir());
    let job = Job::new(request(&dir.path().join("freddo.txt")));
    let wake = Arc::new(WakeOutside {
        job: job.clone(),
        calls: AtomicUsize::new(0),
    });
    let waker = futures::task::waker(wake.clone());
    let mut cx = Context::from_waker(&waker);
    let mut collector = Box::pin(job.collect());
    assert!(collector.as_mut().poll(&mut cx).is_pending());
    drop(job.delivery());
    assert!(wake.calls.load(Ordering::SeqCst) > 0);
    assert!(collector.as_mut().poll(&mut cx).is_ready());
}

fn richiesta_con_nome(path: &Path) -> Request {
    Request {
        aliases: Some(Vec::new()),
        ..request(path)
    }
}

#[test]
fn save_ordinario_non_pubblica_prima_del_permesso_del_worker() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("ordinario.txt");
    ok(std::fs::write(&path, b"originale"));
    let job = Job::new(request(&path));
    runtime().block_on(async {
        let mut delivery = job.delivery();
        let ready = match some(
            tokio::time::timeout(std::time::Duration::from_secs(3), delivery.next())
                .await
                .ok()
                .flatten(),
        ) {
            Event::Ready(ready) => ready,
            Event::Done(_) => panic!("il save ordinario deve attendere l'autorizzazione"),
        };
        assert_eq!(ok(std::fs::read(&path)), b"originale");
        assert!(ready.grant());
        let done = concluso(&mut delivery).await;
        assert!(receipt(&done).result.is_ok());
        assert_eq!(ok(std::fs::read(&path)), b"\xef\xbb\xbftesto\r\n");
    });
}

async fn pronto(delivery: &mut Delivery) -> Ready {
    match some(
        tokio::time::timeout(std::time::Duration::from_secs(3), delivery.next())
            .await
            .ok()
            .flatten(),
    ) {
        Event::Ready(ready) => ready,
        Event::Done(_) => panic!("Ready effettivo assente"),
    }
}
async fn concluso(delivery: &mut Delivery) -> Receipt {
    match some(
        tokio::time::timeout(std::time::Duration::from_secs(3), delivery.next())
            .await
            .ok()
            .flatten(),
    ) {
        Event::Done(receipt) => receipt,
        Event::Ready(_) => panic!("Ready duplicato nella stessa consegna"),
    }
}
fn residui(dir: &Path) -> usize {
    ok(std::fs::read_dir(dir)).count()
}

#[test]
fn preparato_reale_non_pubblica_prima_di_ready_e_nega_con_pulizia() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("destinazione.txt");
    ok(std::fs::write(&path, "originale"));
    let job = Job::new(richiesta_con_nome(&path));
    runtime().block_on(async {
        let mut delivery = job.delivery();
        let ready = pronto(&mut delivery).await;
        assert!(job.matches_ready(&ready));
        assert_eq!(ok(std::fs::read_to_string(&path)), "originale");
        assert!(
            residui(dir.path()) > 1,
            "preparato realmente posseduto dal worker"
        );
        ready.deny();
        let done = concluso(&mut delivery).await;
        assert!(job.matches(&done));
        assert!(receipt(&done).result.is_err());
        assert_eq!(ok(std::fs::read_to_string(&path)), "originale");
        assert_eq!(residui(dir.path()), 1);
        assert!(delivery.next().await.is_none());
    });
}

#[test]
fn perdita_ultima_decisione_aborta_file_reale_e_fallback_non_la_ricrea() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("negato.txt");
    let job = Job::new(richiesta_con_nome(&path));
    runtime().block_on(async {
        let mut delivery = job.delivery();
        let ready = pronto(&mut delivery).await;
        assert!(!path.exists());
        let duplicate = ready.clone();
        drop(delivery);
        drop(ready);
        assert!(job.matches_ready(&duplicate));
        drop(duplicate);
        let fallback = completed(&job);
        futures::pin_mut!(fallback);
        let Event::Done(done) = some(ok(tokio::time::timeout(
            std::time::Duration::from_secs(3),
            fallback.next(),
        )
        .await)) else {
            panic!("autorità persa non può essere ricreata dal collector");
        };
        assert!(receipt(&done).result.is_err());
        assert!(!path.exists());
        assert_eq!(residui(dir.path()), 0);
    });
}

#[test]
fn perdita_ultimo_owner_aborta_e_runtime_raccoglie_il_worker() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("runtime-loss.txt");
    let job = Job::new(request(&path));
    let rt = runtime();
    let mut delivery = job.delivery();
    let ready = rt.block_on(pronto(&mut delivery));

    assert!(!path.exists());
    assert_eq!(residui(dir.path()), 1, "il temporaneo è del worker attivo");

    drop(delivery);
    drop(ready);
    drop(job);
    drop(rt);

    assert!(
        !path.exists(),
        "nessun commit dopo la perdita dell'ultimo owner"
    );
    assert_eq!(residui(dir.path()), 0, "lo shutdown raccoglie il cleanup");
}

#[test]
fn shutdown_runtime_dopo_commit_started_attende_pubblicazione_reale() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("runtime-commit.txt");
    ok(std::fs::write(&path, "prima"));
    let job = Job::new(request(&path));
    let rt = runtime();
    let mut delivery = job.delivery();
    let ready = rt.block_on(pronto(&mut delivery));
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    job.set_commit_started_hook(move || {
        ok(started_tx.send(()));
        ok(release_rx.recv_timeout(std::time::Duration::from_secs(5)));
    });

    assert!(ready.grant());
    ok(started_rx.recv_timeout(std::time::Duration::from_secs(3)));
    assert_eq!(ok(std::fs::read_to_string(&path)), "prima");

    drop(delivery);
    drop(ready);
    drop(job);
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        ok(shutdown_tx.send(()));
        drop(rt);
        ok(finished_tx.send(()));
    });
    ok(shutdown_rx.recv_timeout(std::time::Duration::from_secs(3)));
    assert!(
        finished_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err(),
        "lo runtime non deve dichiarare concluso lo shutdown mentre il commit è bloccato"
    );

    ok(release_tx.send(()));
    ok(finished_rx.recv_timeout(std::time::Duration::from_secs(3)));
    ok(shutdown.join());
    assert_eq!(ok(std::fs::read_to_string(&path)), "\u{feff}testo\r\n");
    assert_eq!(
        residui(dir.path()),
        1,
        "resta soltanto il file destinazione"
    );
}

#[test]
fn collector_recupera_stesso_ready_vivo_e_foreign_replay_non_autorizzano() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("valido.txt");
    let job = Job::new(richiesta_con_nome(&path));
    let foreign = Job::new(richiesta_con_nome(&path));
    runtime().block_on(async {
        let mut delivery = job.delivery();
        let ready = pronto(&mut delivery).await;
        assert!(!foreign.matches_ready(&ready));
        let fallback = completed(&job);
        futures::pin_mut!(fallback);
        let Event::Ready(recovered) = some(fallback.next().await) else {
            panic!("Ready recuperabile assente");
        };
        assert!(Arc::ptr_eq(&ready.metadata, &recovered.metadata));
        drop(delivery);
        drop(ready);
        assert!(job.matches_ready(&recovered));
        assert!(recovered.grant());
        assert!(!job.matches_ready(&recovered));
        assert!(!recovered.grant());
        let Event::Done(done) = some(ok(tokio::time::timeout(
            std::time::Duration::from_secs(3),
            fallback.next(),
        )
        .await)) else {
            panic!("ricevuta autentica assente");
        };
        assert!(job.matches(&done));
        assert!(receipt(&done).result.is_ok());
        assert_eq!(ok(std::fs::read(&path)), b"\xef\xbb\xbftesto\r\n");
        assert_eq!(residui(dir.path()), 1);
        assert!(fallback.next().await.is_none());
    });
}

#[test]
fn granted_revocato_aborta_preparato_reale_senza_pubblicare() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("negato.txt");
    let job = Job::new(richiesta_con_nome(&path));
    runtime().block_on(async {
        let mut delivery = job.delivery();
        let ready = pronto(&mut delivery).await;
        let pause = some(job.pause());
        assert!(ready.grant());
        job.discard_pending(Some(DocId(7)));
        assert!(job.running(), "attendere davvero il ritorno del worker");
        pause.finish(false);
        let done = concluso(&mut delivery).await;
        assert!(receipt(&done).result.is_err());
        assert!(!path.exists());
        assert_eq!(residui(dir.path()), 0);
    });
}

#[test]
fn fixture_request_con_nome_non_approva_implicitamente() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("no_bypass.txt");
    let completion = runtime().block_on(richiesta_con_nome(&path).run());
    assert!(completion.result.is_err());
    assert!(!path.exists());
    assert_eq!(residui(dir.path()), 0);
}
