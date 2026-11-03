//! Prova del guard di raccolta: engine/FS/thread reali, non GUI o Driver SQL.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

#[cfg(target_os = "linux")]
#[test]
#[ignore = "helper eseguito dal parent isolato"]
fn child_runtime_timeout_producer_vivo() {
    fn producer() -> Option<(std::path::PathBuf, String)> {
        std::fs::read_dir("/proc/self/task")
            .unwrap()
            .find_map(|item| {
                let path = item.ok()?.path();
                let stat = std::fs::read_to_string(path.join("stat")).ok()?;
                let (_, tail) = stat.rsplit_once(')')?;
                if !stat.contains("(lume-cerca)") {
                    return None;
                }
                let start = tail.split_whitespace().nth(19)?.to_owned();
                Some((path, start))
            })
    }
    let root =
        std::path::PathBuf::from(std::env::var_os("LUME_SEARCH_RUNTIME_TIMEOUT_ROOT").unwrap());
    let paths = (0..100)
        .map(|index| {
            let path = root.join(format!("runtime-timeout-{index}.txt"));
            std::fs::write(&path, "ago reale\n").unwrap();
            path
        })
        .collect();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut jobs = Jobs::default();
    let job = jobs
        .reserve(
            SearchScope::Aperti(paths),
            SearchOptions::literal("ago"),
            Arc::new(AtomicU64::new(1)),
            1,
        )
        .unwrap();
    let mut stream = Box::pin(deliveries(job.clone()));
    let packet = runtime.block_on(stream.next()).unwrap();
    assert!(matches!(packet.observe(), Some(SearchUpdate::Batch(_))));
    let identity = producer().expect("producer reale prima dello shutdown");
    assert!(job.is_current());
    assert!(!job.collected());
    // Il consumer e il packet restano vivi: il producer non può completare
    // le cinque Batch nella mailbox limitata, senza ulteriori poll/ack.
    runtime.shutdown_timeout(std::time::Duration::ZERO);
    assert_eq!(producer(), Some(identity.clone()));
    assert!(job.is_current());
    assert!(!job.collected());
    // Cancel dopo la distruzione del runtime originario, non prima.
    drop(stream);
    assert!(!job.is_current());
    let result = futures::executor::block_on(job.collect());
    assert!(result.is_ok());
    assert!(job.collected() && job.succeeded());
    assert!(jobs.remove(job.id(), &result));
    assert!(!jobs.busy());
    // Ricevuta join sopra; procfs può eliminare il task un poco dopo il join.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while producer() == Some(identity.clone()) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_ne!(producer(), Some(identity));
    assert!(matches!(packet.observe(), Some(SearchUpdate::Batch(_))));
    drop(packet);
    println!("LUME_SEARCH_RUNTIME_TIMEOUT_COMPLETE");
}

#[cfg(target_os = "linux")]
#[test]
fn runtime_timeout_con_producer_vivo_raccolto_senza_secondo_runtime() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    // Il parent conserva la fixture anche se deve terminare il child.
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "search_events::tests::child_runtime_timeout_producer_vivo",
            "--ignored",
            "--nocapture",
        ])
        .env("LUME_SEARCH_RUNTIME_TIMEOUT_ROOT", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut timeout = false;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            timeout = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!timeout, "child timeout: ucciso e raccolto; {stdout}");
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("LUME_SEARCH_RUNTIME_TIMEOUT_COMPLETE"));
    assert!(stdout.contains("1 passed; 0 failed"));
}

#[test]
fn consumer_abbandonato_e_runtime_terminato_conservano_ricevuta_raccoglibile() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..100)
        .map(|index| {
            let path = dir.path().join(format!("runtime-{index}.txt"));
            std::fs::write(&path, "ago reale\n").unwrap();
            path
        })
        .collect();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut jobs = Jobs::default();
    let job = jobs
        .reserve(
            SearchScope::Aperti(paths),
            SearchOptions::literal("ago"),
            Arc::new(AtomicU64::new(1)),
            1,
        )
        .unwrap();
    let mut stream = Box::pin(deliveries(job.clone()));
    let packet = runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
    });
    assert!(matches!(packet.observe(), Some(SearchUpdate::Batch(_))));
    assert!(job.is_current());
    assert!(!job.collected());
    // Nessun ack: source e DataGuard abbandonati prima della fine runtime.
    drop(stream);
    assert!(!job.is_current());
    // Drop Runtime attende i blocking worker già avviati, senza timeout/detach.
    drop(runtime);
    // La receipt resta leggibile senza creare un secondo runtime Tokio.
    let result = futures::executor::block_on(job.collect());
    assert!(result.is_ok());
    assert!(job.collected());
    assert!(job.succeeded());
    assert!(jobs.remove(job.id(), &result));
    assert!(!jobs.busy());
    drop(packet);
}

#[test]
fn eof_core_revocato_senza_cancel_richiesta_e_errore_incompleto_non_silenzio() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for index in 0..100 {
        let path = dir.path().join(format!("query-{index}.txt"));
        std::fs::write(&path, "ago reale\n").unwrap();
        paths.push(path);
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut jobs = Jobs::default();
            let job = jobs
                .reserve(
                    SearchScope::Aperti(paths),
                    SearchOptions::literal("ago"),
                    Arc::new(AtomicU64::new(1)),
                    1,
                )
                .unwrap();
            let mut stream = Box::pin(deliveries(job.clone()));
            let packet = stream.next().await.unwrap();
            assert!(matches!(packet.take(), Some(SearchUpdate::Batch(_))));
            packet.acknowledge(true);
            // EOF controllato del vero engine; nessuna factory o dato finto.
            // Modella il core interrotto da un guard, non un cancel UI richiesto.
            lock(&job).origin.clone().unwrap().cancel();
            let next = stream.next().await;
            let outcome = next.as_ref().and_then(Delivery::take);
            if let Some(next) = next {
                next.acknowledge(true);
            }
            drop(stream);
            assert!(job.collect().await.is_ok());
            assert!(
                matches!(outcome,Some(SearchUpdate::Error(text)) if text.contains("incompleti")),
                "EOF di richiesta ancora valida non deve essere silenzioso"
            );
        });
}

#[test]
fn panic_collector_guard_su_worker_cancella_e_raccoglie_producer_pieno() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for index in 0..100 {
        let path = dir.path().join(format!("file-{index}.txt"));
        std::fs::write(&path, "ago reale\n").unwrap();
        paths.push(path);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let unwinding = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_unwinding = Arc::clone(&unwinding);
        let mut collector = tokio::task::spawn_blocking(move || {
            let run = lume_core::spawn_search_owned(
                SearchScope::Aperti(paths),
                SearchOptions::literal("ago"),
                Arc::new(AtomicU64::new(1)),
                1,
            )
            .unwrap();
            let origin = run.events.origin();
            let _events = run.events;
            let guard = Producer {
                origin: origin.clone(),
                worker: Some(run.worker),
            };
            std::thread::sleep(std::time::Duration::from_millis(30)); // solo harness
            let finished = guard.worker.as_ref().unwrap().is_finished();
            let _ = sender.send((origin, finished));
            struct UnwindMarker(Arc<std::sync::atomic::AtomicBool>);
            impl Drop for UnwindMarker {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _unwind_marker = UnwindMarker(worker_unwinding);
            // Questo test misura Drop durante l'unwind. Il panic hook
            // globale precede l'unwind e può contendere con la risoluzione
            // backtrace degli altri test prima che il guard sia raggiunto.
            std::panic::resume_unwind(Box::new(
                "panic volontario prima del join: guard deve raccogliere su worker",
            ));
        });
        let (origin, finished) = tokio::time::timeout(std::time::Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap();
        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), &mut collector).await;
        assert!(
            joined.is_ok(),
            "collector timeout: finished={} origin_current={} unwinding={}",
            collector.is_finished(),
            origin.is_current(),
            unwinding.load(Ordering::SeqCst)
        );
        assert!(joined.unwrap().is_err());
        assert!(unwinding.load(Ordering::SeqCst));
        assert!(!finished, "producer deve essere vivo a coda non drenata");
        assert!(
            !origin.is_current(),
            "Drop guard è stato eseguito prima del termine del collector"
        );
    });
}
