//! Misura Linux isolata della pipeline Cerca reale, non della GUI.
//! Nessun executor/pool/backend alternativo: si usano spawn_search e run_search.
//! I conteggi /proc sono TOTALI del processo di test, non attribuzioni ai worker.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const FILES: usize = 200;
const HITS_PER_FILE: usize = 10;
const LINE_BYTES: usize = 4096;
const WAIT: Duration = Duration::from_millis(200);

fn corpus() -> (tempfile::TempDir, Vec<PathBuf>, String) {
    let dir = tempfile::tempdir().unwrap();
    // 7 byte UTF-8 di prefisso + padding + LF = 4096 byte esatti.
    let line = format!("ago é {}\n", "x".repeat(LINE_BYTES - 8));
    assert_eq!(line.len(), LINE_BYTES);
    let content = line.repeat(HITS_PER_FILE);
    let mut paths = Vec::new();
    let mut hash = Sha256::new();
    for index in 0..FILES {
        let name = format!("src/query-{index:03}.sql");
        let file = dir.path().join(&name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &content).unwrap();
        // Manifest indipendente dalla radice temporanea: nome LF hash LF.
        hash.update(name.as_bytes());
        hash.update(b"\n");
        hash.update(format!("{:x}\n", Sha256::digest(content.as_bytes())));
        paths.push(file);
    }
    (dir, paths, format!("{:x}", hash.finalize()))
}

fn kib(text: &str, label: &str) -> u64 {
    text.lines()
        .find_map(|line| line.strip_prefix(label))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .parse()
        .unwrap()
}

fn resources(path: &str, repeat: usize, stage: &str) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let smaps = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap();
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields = stat.rsplit_once(") ").unwrap().1;
    let user: u64 = fields.split_whitespace().nth(11).unwrap().parse().unwrap();
    let system: u64 = fields.split_whitespace().nth(12).unwrap().parse().unwrap();
    println!(
        "RISORSE,{path},{repeat},{stage},{},{},{},{},{}",
        kib(&status, "VmRSS:"),
        kib(&smaps, "Pss:"),
        kib(&smaps, "Private_Clean:") + kib(&smaps, "Private_Dirty:"),
        kib(&status, "Threads:"),
        user + system
    );
}

#[derive(Default)]
struct Observed {
    events: usize,
    batches: usize,
    files: usize,
    hits: usize,
    unique_line_bytes: usize,
    terminal: bool,
    lines: HashSet<usize>,
    paths: HashSet<PathBuf>,
}
impl Observed {
    fn see(&mut self, update: &lume_core::SearchUpdate) {
        self.events += 1;
        match update {
            lume_core::SearchUpdate::Batch(files) => {
                self.batches += 1;
                for file in files {
                    assert!(self.paths.insert(file.path.clone()));
                    self.files += 1;
                    for hit in &file.hits {
                        self.hits += 1;
                        assert_eq!(hit.line_text.get(hit.range_in_line.clone()).unwrap(), "ago");
                        let pointer = hit.line_text.as_ptr() as usize;
                        if self.lines.insert(pointer) {
                            self.unique_line_bytes += hit.line_text.len();
                        }
                    }
                }
            }
            lume_core::SearchUpdate::Done {
                files,
                matches,
                truncated,
            } => {
                assert!(!self.terminal);
                assert_eq!(
                    (*files, *matches, *truncated),
                    (FILES, FILES * HITS_PER_FILE, true)
                );
                self.terminal = true;
            }
            lume_core::SearchUpdate::Error(error) => panic!("ricerca reale fallita: {error}"),
        }
    }
    fn validate(&self, expected: &[PathBuf]) {
        assert!(
            self.terminal,
            "EOF senza Done non vale come misura riuscita"
        );
        assert_eq!(
            (self.events, self.batches, self.files, self.hits),
            (11, 10, FILES, FILES * HITS_PER_FILE)
        );
        assert_eq!(self.paths, expected.iter().cloned().collect());
        // Ogni batch resta vivo nei risultati del modello/core consumer:
        // nessun riuso di puntatore che falsi la deduplicazione Arc.
        assert_eq!(
            self.unique_line_bytes,
            FILES * HITS_PER_FILE * (LINE_BYTES - 1)
        );
    }
}

fn core_measure(paths: &[PathBuf], repeat: usize) {
    let generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
    resources("core", repeat, "prima");
    let start = Instant::now();
    let receiver = lume_core::spawn_search(
        lume_core::SearchScope::Aperti(paths.to_vec()),
        lume_core::SearchOptions::literal("ago"),
        generation,
        1,
    );
    let returned_us = start.elapsed().as_micros();
    std::thread::sleep(WAIT); // ritardo deliberato del consumer, SOLO nel test
    resources("core", repeat, "non_drenato_200ms");
    let drain = Instant::now();
    let mut observed = Observed::default();
    let mut retained = Vec::new();
    while let Ok(update) = receiver.recv_timeout(Duration::from_secs(10)) {
        observed.see(&update);
        let terminal = matches!(update, lume_core::SearchUpdate::Done { .. });
        retained.push(update);
        if terminal {
            break;
        }
    }
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(10)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    observed.validate(paths);
    println!(
        "MISURA,core,{repeat},{returned_us},{},{},{},{},{}",
        drain.elapsed().as_micros(),
        start.elapsed().as_micros(),
        observed.events,
        observed.hits,
        observed.unique_line_bytes
    );
    resources("core", repeat, "risultati_trattenuti");
    drop(retained);
    drop(receiver);
    resources("core", repeat, "rilasciato");
    // Disconnected dimostra uscita del sender, NON join: API corrente non
    // restituisce owner/JoinHandle. Non fingere una raccolta verificata.
}

fn core_owned_measure(paths: &[PathBuf], repeat: usize) {
    let generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
    resources("core_owned", repeat, "prima");
    let start = Instant::now();
    let mut run = lume_core::spawn_search_owned(
        lume_core::SearchScope::Aperti(paths.to_vec()),
        lume_core::SearchOptions::literal("ago"),
        generation,
        1,
    )
    .unwrap();
    let returned_us = start.elapsed().as_micros();
    std::thread::sleep(WAIT); // solo harness: consumer non drenante
    resources("core_owned", repeat, "non_drenato_200ms");
    let drain = Instant::now();
    let mut observed = Observed::default();
    let mut retained = Vec::new();
    futures::executor::block_on(async {
        while let Some(update) = run.events.next().await {
            observed.see(&update);
            retained.push(update);
        }
    });
    // Questo è il thread del test, NON UI: join esplicito dopo EOF.
    run.worker.join().unwrap();
    observed.validate(paths);
    println!(
        "MISURA,core_owned,{repeat},{returned_us},{},{},{},{},{}",
        drain.elapsed().as_micros(),
        start.elapsed().as_micros(),
        observed.events,
        observed.hits,
        observed.unique_line_bytes
    );
    resources("core_owned", repeat, "risultati_trattenuti");
    drop(retained);
    drop(run.events);
    resources("core_owned", repeat, "rilasciato");
}

async fn app_measure(root: &std::path::Path, paths: &[PathBuf], repeat: usize, mode: &str) {
    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(root.to_path_buf());
    let requests = app.registry.set_active_panel(Some(tools::search_tool::ID));
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();
    resources(mode, repeat, "prima");
    let start = Instant::now();
    let task = app.run_search(
        lume_core::SearchOptions::literal("ago"),
        lume_core::SearchArea::Progetto,
    );
    let returned_us = start.elapsed().as_micros();
    let generation = app.search_gen.load(Ordering::SeqCst);
    // Stesso intervallo della base23/24. Modalità app_attiva: primo packet
    // vero tenuto vivo e NON applicato, producer ora attivo con ack pendente.
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let mut held = if mode == "app_attiva" {
        let first = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            first,
            iced_runtime::Action::Output(Message::SearchUpdate(..))
        ));
        Some(first)
    } else {
        None
    };
    let held_first_us = held.as_ref().map(|_| start.elapsed().as_micros());
    // Negli altri modi la Task non pollata non crea producer: non inventare
    // backlog. La cancellazione è prima del primo poll, distinta da attiva.
    tokio::time::sleep(WAIT).await;
    resources(mode, repeat, "non_drenato_200ms");
    if mode == "app_cancel" {
        app.cancel_search();
    }
    let drain = Instant::now();
    let mut first_us = held_first_us;
    let mut observed = Observed::default();
    let mut accepted = 0;
    let mut ui_us = 0;
    // In cancellazione il modello rifiuta gen vecchia: per tenere validi gli
    // Arc osservati conserviamo i messaggi fino alla verifica, SOLO qui.
    let mut rejected = Vec::new();
    loop {
        let action = if let Some(first) = held.take() {
            Some(first)
        } else {
            tokio::time::timeout(Duration::from_secs(10), stream.next())
                .await
                .unwrap()
        };
        let Some(iced_runtime::Action::Output(message)) = action else {
            assert!(action.is_none(), "azione inattesa nella Task Cerca");
            break;
        };
        if let Message::SearchUpdate(delivered_generation, delivery) = &message {
            first_us.get_or_insert(start.elapsed().as_micros());
            assert_eq!(*delivered_generation, generation);
            let update = delivery.observe().unwrap();
            observed.see(&update);
            if delivery.is_current()
                && *delivered_generation == app.search_gen.load(Ordering::SeqCst)
            {
                accepted += 1;
            }
        } else {
            assert!(
                matches!(message, Message::SearchCollected(..)),
                "messaggio inatteso nella Task Cerca"
            );
        }
        if mode == "app_cancel" {
            rejected.push(message.clone());
        }
        let apply = Instant::now();
        let next = app.update(message); // produzione: consegna al modello del tool
        if let Some(mut following) = iced_runtime::task::into_stream(next) {
            // Task::batch può avere uno stream presente ma vuoto: non è
            // una nuova operazione. Un effetto reale va invece segnalato,
            // non ignorato come se avessimo una finestra GUI.
            assert!(
                tokio::time::timeout(Duration::from_secs(10), following.next())
                    .await
                    .unwrap()
                    .is_none(),
                "effetto secondario non previsto nella misura del modello"
            );
        }
        ui_us += apply.elapsed().as_micros();
        if mode == "app_lenta" {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    if mode == "app_cancel" {
        assert!(observed.hits <= FILES * HITS_PER_FILE);
        assert!(observed.paths.iter().all(|path| paths.contains(path)));
    } else {
        observed.validate(paths);
    }
    assert_eq!(accepted, if mode == "app_cancel" { 0 } else { 11 });
    println!(
        "MISURA,{mode},{repeat},{returned_us},{},{},{},{},{},{}",
        drain.elapsed().as_micros(),
        start.elapsed().as_micros(),
        observed.events,
        observed.hits,
        observed.unique_line_bytes,
        first_us.map_or("nessuno".into(), |time| time.to_string())
    );
    println!("TERMINALE,{mode},{repeat},{}", observed.terminal);
    println!("APPLICAZIONE,{mode},{repeat},{accepted},{ui_us}");
    resources(mode, repeat, "risultati_trattenuti");
    drop(stream);
    drop(rejected);
    let requests = app.registry.set_active_panel(None);
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();
    resources(mode, repeat, "vista_chiusa");
    drop(app);
    resources(mode, repeat, "rilasciato");
}

#[test]
#[ignore = "misura isolata Linux: usare verifica_locale.sh measure-search, non accettazione GUI/Windows"]
fn misura_pipeline_cerca_reale() {
    println!(
        "SORGENTI,bench={:x},app={:x},core={:x},canale={:x},supervisor={:x}",
        Sha256::digest(include_bytes!("search_measure_tests.rs")),
        Sha256::digest(include_bytes!("../app.rs")),
        Sha256::digest(include_bytes!("../../../lume-core/src/search.rs")),
        Sha256::digest(include_bytes!("../../../lume-core/src/search/events.rs")),
        Sha256::digest(include_bytes!("../search_events.rs"))
    );
    let (dir, paths, hash) = corpus();
    println!("CORPUS,200file,10righe,4096byte_riga,8192000byte,sha256={hash}");
    println!(
        "LIMITI,debug Linux; non GUI; VmRSS/Pss/private resident KiB NON Windows private bytes; CPU tick /proc; thread TOTALI; filesystem cache non svuotata; niente join certificato"
    );
    println!(
        "COLONNE_RISORSE,percorso,ripetizione,stadio,rss_kib,pss_kib,private_resident_kib,thread_totali,cpu_tick_totali"
    );
    println!(
        "COLONNE_MISURA,percorso,ripetizione,ritorno_task_us,drenaggio_us,fine_us,eventi,match,byte_linee_uniche,primo_evento_us_se_app"
    );
    // Prima ripetizione distinta: cache FS/allocatore/runtime non freddi
    // per garanzia. Le altre quattro servono al confronto di ripetibilità.
    for repeat in 0..5 {
        core_measure(&paths, repeat);
    }
    for repeat in 0..5 {
        core_owned_measure(&paths, repeat);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for mode in ["app_normale", "app_lenta", "app_cancel", "app_attiva"] {
            for repeat in 0..5 {
                app_measure(dir.path(), &paths, repeat, mode).await;
            }
        }
    });
}
