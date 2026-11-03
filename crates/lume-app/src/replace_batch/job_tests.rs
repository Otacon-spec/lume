#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use futures::StreamExt;
use std::time::Duration;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn source(dir: &std::path::Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"ago\n").unwrap();
    path
}
fn found(path: PathBuf) -> FileMatches {
    FileMatches {
        path,
        hits: vec![lume_core::SearchHit {
            line_number: 1,
            line_text: "ago".into(),
            range_in_line: 0..3,
        }],
    }
}
fn job(id: u64, paths: &[PathBuf]) -> Job {
    Job::new(
        id,
        paths.iter().cloned().map(found).collect(),
        "AGO".into(),
        vec![],
        vec![],
        Arc::new(AtomicBool::new(false)),
    )
}
async fn next(stream: &mut (impl Stream<Item = Event> + Unpin)) -> Event {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
}
async fn ready(stream: &mut (impl Stream<Item = Event> + Unpin)) -> Ready {
    match next(stream).await {
        Event::Ready(ready) => ready,
        other => panic!("atteso consenso, ricevuto {other:?}"),
    }
}
async fn done(stream: &mut (impl Stream<Item = Event> + Unpin)) -> Receipt {
    match next(stream).await {
        Event::Done(receipt) => receipt,
        other => panic!("attesa ricevuta, ricevuto {other:?}"),
    }
}

#[test]
fn drop_owner_concluso_non_revoca_la_fase_autosave_successiva() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        let job = job(1, std::slice::from_ref(&path));
        let cancel = job.0.cancel.clone();
        let mut stream = job.delivery();
        let token = ready(&mut stream).await;
        assert!(token.grant());
        let receipt = done(&mut stream).await;
        assert!(receipt.take().unwrap().report.ok());
        assert!(stream.next().await.is_none());
        drop(token);
        drop(stream);
        drop(job);
        assert!(!cancel.load(Ordering::Acquire));
        assert_eq!(std::fs::read(path).unwrap(), b"AGO\n");
    });
}

#[test]
fn consenso_reale_concede_o_nega_senza_plan_nella_ui() {
    runtime().block_on(async {
        for grant in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = source(dir.path(), "file.txt");
            let job = job(1, std::slice::from_ref(&path));
            let mut stream = job.delivery();
            let token = ready(&mut stream).await;
            assert!(job.matches_ready(&token));
            assert_eq!(std::fs::read(&path).unwrap(), b"ago\n");
            if grant {
                assert!(token.grant());
            } else {
                token.deny();
            }
            let receipt = done(&mut stream).await;
            assert!(job.matches(&receipt));
            let outcome = receipt.take().unwrap();
            assert!(job.matches(&receipt)); // Identità prima e dopo consumo.
            assert!(receipt.take().is_none());
            assert_eq!(outcome.report.ok(), grant);
            assert!(outcome.report.cleanup_failures.is_empty());
            assert_eq!(
                std::fs::read(&path).unwrap(),
                if grant { &b"AGO\n"[..] } else { &b"ago\n"[..] }
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
            assert!(stream.next().await.is_none());
        }
    });
}

#[test]
fn collector_freddo_non_ammette_e_drop_delivery_notifica_done() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        let job = job(2, std::slice::from_ref(&path));
        let collector = completed(&job);
        futures::pin_mut!(collector);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), collector.next())
                .await
                .is_err()
        );
        assert!(matches!(&*job.lock(), Phase::Pending(Some(_))));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        drop(job.delivery());
        let receipt = done(&mut collector).await;
        assert!(job.matches(&receipt));
        assert!(
            receipt
                .take()
                .unwrap()
                .report
                .error
                .unwrap()
                .contains("non avviata")
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"ago\n");
        let mut late = job.delivery();
        assert!(job.matches(&done(&mut late).await));
    });
}

#[test]
fn produttore_ammesso_perso_collector_recupera_stessa_lease_e_done() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        let job = job(3, std::slice::from_ref(&path));
        let mut producer = job.delivery();
        let token = ready(&mut producer).await;
        drop(producer);
        let collector = completed(&job);
        futures::pin_mut!(collector);
        let fallback = ready(&mut collector).await;
        assert!(job.matches_ready(&fallback));
        assert!(Arc::ptr_eq(&token.metadata, &fallback.metadata));
        assert!(fallback.grant());
        let receipt = done(&mut collector).await;
        assert!(job.matches(&receipt));
        assert_eq!(receipt.take().unwrap().report.committed, vec![path.clone()]);
        assert_eq!(std::fs::read(path).unwrap(), b"AGO\n");
    });
}

#[test]
fn ultima_lease_persa_non_viene_ricreata_dal_collector() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        let job = job(4, std::slice::from_ref(&path));
        let mut producer = job.delivery();
        let token = ready(&mut producer).await;
        drop(token);
        drop(producer);
        let collector = completed(&job);
        futures::pin_mut!(collector);
        let outcome = done(&mut collector).await.take().unwrap();
        assert!(outcome.report.committed.is_empty());
        assert_eq!(outcome.report.not_attempted, vec![path.clone()]);
        assert!(outcome.report.cleanup_failures.is_empty());
        assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    });
}

#[test]
fn token_estranei_con_stesso_id_non_autorizzano_ne_consumano() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let first = source(dir.path(), "primo.txt");
        let second = source(dir.path(), "secondo.txt");
        let a = job(7, &[first]);
        let b = job(7, &[second]);
        assert_eq!(a.id(), b.id());
        assert_ne!(a, b);
        assert_eq!(a, a.clone());
        let mut sa = a.delivery();
        let mut sb = b.delivery();
        let ra = ready(&mut sa).await;
        let rb = ready(&mut sb).await;
        assert!(!a.matches_ready(&rb));
        assert!(!b.matches_ready(&ra));
        assert!(a.matches_ready(&ra));
        assert!(b.matches_ready(&rb));
        ra.deny();
        rb.deny();
        let ca = done(&mut sa).await;
        let cb = done(&mut sb).await;
        assert!(!a.matches(&cb));
        assert!(!b.matches(&ca));
        assert!(a.matches(&ca));
        assert!(b.matches(&cb));
        assert!(ca.take().is_some());
        assert!(cb.take().is_some());
    });
}

#[test]
fn pausa_revocata_dopo_grant_aborta_prima_del_commit() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        let job = job(8, std::slice::from_ref(&path));
        let pause = job.pause();
        let mut stream = job.delivery();
        let token = ready(&mut stream).await;
        assert!(token.grant());
        pause.finish(false);
        let outcome = done(&mut stream).await.take().unwrap();
        assert!(outcome.report.committed.is_empty());
        assert!(outcome.report.cleanup_failures.is_empty());
        assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    });
}

#[test]
fn hook_commit_del_gate_e_cancellazione_restano_puntuali() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        let job = job(9, std::slice::from_ref(&path));
        let cancel = job.0.cancel.clone();
        job.set_commit_started_hook(move || cancel.store(true, Ordering::Release));
        let mut stream = job.delivery();
        let token = ready(&mut stream).await;
        assert!(token.grant());
        let outcome = done(&mut stream).await.take().unwrap();
        assert!(outcome.report.committed.is_empty());
        assert_eq!(outcome.report.not_attempted, vec![path.clone()]);
        assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    });
}

#[test]
fn cancellazione_dopo_primo_file_preserva_prefisso_e_aborta_tutti_i_restanti() {
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        source(dir.path(), "uno.txt"),
        source(dir.path(), "due.txt"),
        source(dir.path(), "tre.txt"),
    ];
    let cancel = Arc::new(AtomicBool::new(false));
    let plan = prepare_sync(
        paths.iter().cloned().map(found).collect(),
        "AGO".into(),
        vec![],
        vec![],
        cancel.clone(),
    )
    .unwrap();
    let outcome = super::super::commit_with_boundary(plan, &cancel, |index| {
        if index == 1 {
            cancel.store(true, Ordering::Release);
        }
    });
    assert_eq!(outcome.report.committed, vec![paths[0].clone()]);
    assert_eq!(outcome.report.not_attempted, paths[1..]);
    assert!(outcome.report.failed.is_none());
    assert!(outcome.report.cleanup_failures.is_empty());
    assert_eq!(std::fs::read(&paths[0]).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read(&paths[1]).unwrap(), b"ago\n");
    assert_eq!(std::fs::read(&paths[2]).unwrap(), b"ago\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
}

#[cfg(unix)]
#[test]
fn abort_esplicito_riporta_residuo_e_tenta_anche_gli_handle_successivi() {
    let dir = tempfile::tempdir().unwrap();
    let residual = tempfile::tempdir().unwrap();
    let path = source(dir.path(), "file.txt");
    let mut plan = prepare_sync(
        vec![found(path.clone())],
        "AGO".into(),
        vec![],
        vec![],
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let temporary = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|candidate| candidate != &path)
        .unwrap();
    let moved = residual.path().join("residuo");
    std::fs::rename(&temporary, &moved).unwrap();
    std::fs::create_dir(&temporary).unwrap();
    let second = source(dir.path(), "secondo.txt");
    let extra = prepare_sync(
        vec![found(second.clone())],
        "AGO".into(),
        vec![],
        vec![],
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    plan.closed.extend(extra.closed);
    let shared = super::super::Shared::new(plan);
    let report = runtime().block_on(super::super::abort(shared)).unwrap();
    assert_eq!(report.cleanup_failures.len(), 1);
    assert_eq!(&report.cleanup_failures.first().unwrap().0, &path);
    assert!(!report.ok());
    assert!(report.describe().contains("residui da verificare"));
    assert!(temporary.is_dir());
    assert!(moved.is_file());
    assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
    assert_eq!(std::fs::read(second).unwrap(), b"ago\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
}

#[test]
fn fingerprint_stantio_dopo_ready_aborta_tutto_prima_del_primo_commit() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let first = source(dir.path(), "primo.txt");
        let second = source(dir.path(), "secondo.txt");
        let job = job(12, &[first.clone(), second.clone()]);
        let mut stream = job.delivery();
        let token = ready(&mut stream).await;
        std::fs::write(&second, b"esterno\n").unwrap();
        assert!(token.grant());
        let outcome = done(&mut stream).await.take().unwrap();
        assert!(outcome.report.committed.is_empty());
        assert_eq!(
            outcome.report.not_attempted,
            vec![first.clone(), second.clone()]
        );
        assert!(outcome.report.cleanup_failures.is_empty());
        assert_eq!(std::fs::read(first).unwrap(), b"ago\n");
        assert_eq!(std::fs::read(second).unwrap(), b"esterno\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    });
}

#[test]
fn senza_runtime_ricevuta_errore_non_avvia_io() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), "file.txt");
    let job = job(10, std::slice::from_ref(&path));
    let mut stream = job.delivery();
    let receipt = match futures::executor::block_on(stream.next()).unwrap() {
        Event::Done(receipt) => receipt,
        event => panic!("attesa ricevuta senza runtime: {event:?}"),
    };
    assert!(job.matches(&receipt));
    assert!(
        receipt
            .take()
            .unwrap()
            .report
            .error
            .unwrap()
            .contains("Runtime")
    );
    assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn ultimo_owner_revoca_worker_senza_ciclo_con_la_future() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        let job = job(11, std::slice::from_ref(&path));
        let weak = Arc::downgrade(&job.0);
        let cancel = job.0.cancel.clone();
        let mut stream = job.delivery();
        let token = ready(&mut stream).await;
        drop(stream);
        drop(job);
        assert!(weak.upgrade().is_none());
        assert!(cancel.load(Ordering::Acquire));
        assert!(!token.grant());
        // Il worker nativo può essere ancora nel cleanup: non equiparare
        // drop dell'app a una join o a certezza di rimozione immediata.
        tokio::time::timeout(Duration::from_secs(5), async {
            while std::fs::read_dir(dir.path()).unwrap().count() != 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
    });
}

#[test]
fn avviso_sincronizzazione_cartella_non_rende_fallita_la_sostituzione() {
    let mut report = crate::replace_batch::Report::default();
    assert_eq!(report.sync_warning(), None);
    report.committed.push(PathBuf::from("a.txt"));
    report
        .sync_warnings
        .push((PathBuf::from("a.txt"), "EIO".into()));

    assert!(report.ok());
    let warning = report.sync_warning().unwrap();
    assert!(warning.contains("a.txt: EIO"), "{warning}");
}
