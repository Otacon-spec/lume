//! Watcher filesystem event-driven: nessun polling dei file. Su Windows il
//! thread del backend di notify si sveglia comunque ogni 100 ms finché un
//! watcher è vivo (`notify-8.2.0/src/windows.rs:154`, `WaitForSingleObjectEx`).
//!
//! `notify::recommended_watcher` consegna gli eventi grezzi a una callback
//! che li inoltra su un canale interno; un thread dedicato fa il coalescing:
//! dal primo evento raccoglie per [`COALESCE_WINDOW`], deduplica i path e
//! invia un unico [`FsEvent::Changed`]. Gli eventi `Access` sono ignorati.

use std::collections::HashSet;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
mod mailbox;
pub use mailbox::{FsEventReceiver, FsEventSender, fs_event_channel};
use std::thread;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Finestra di coalescing: dal primo evento grezzo si raccoglie per questo
/// intervallo prima di emettere un `FsEvent::Changed`.
const COALESCE_WINDOW: Duration = Duration::from_millis(200);

/// Limite dei path dettagliati; oltre il limite rileggere lo scope (Rescan).
const MAX_COALESCED_PATHS: usize = 512;
const MAX_COALESCED_PATH_BYTES: usize = 256 * 1024;

fn accumulate_paths(
    seen: &mut HashSet<PathBuf>,
    bytes: &mut usize,
    rescan: &mut bool,
    incoming: Vec<PathBuf>,
) {
    if *rescan {
        return;
    }
    for path in incoming {
        if seen.contains(&path) {
            continue;
        }
        let size = path.as_os_str().as_encoded_bytes().len();
        if seen.len() >= MAX_COALESCED_PATHS
            || size > MAX_COALESCED_PATH_BYTES.saturating_sub(*bytes)
        {
            seen.clear();
            *bytes = 0;
            *rescan = true;
            break;
        }
        *bytes += size;
        seen.insert(path);
    }
}

/// Evento coalescato consegnato al chiamante.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEvent {
    /// Il backend ha perso eventi: rileggere lo scope osservato.
    Rescan,
    /// Path toccati (deduplicati) nella finestra di coalescing.
    Changed(Vec<PathBuf>),
    /// Errore asincrono emesso dal backend nativo dopo l'avvio.
    Error(String),
}

type RawFsEvent = FsEvent;

// Traduzione unica della callback nativa, testabile con gli stessi tipi notify.
fn raw_event(res: notify::Result<notify::Event>) -> Option<RawFsEvent> {
    let event = match res {
        Ok(event) => event,
        Err(error) => return Some(RawFsEvent::Error(error.to_string())),
    };
    if event.need_rescan() {
        return Some(RawFsEvent::Rescan);
    }
    if matches!(event.kind, EventKind::Access(_)) || event.paths.is_empty() {
        return None;
    }
    Some(RawFsEvent::Changed(event.paths))
}

/// Possiede backend e coalescer. Il collector permette la raccolta fuori UI;
/// il Drop è un fallback sincrono e non abbandona il thread.
#[derive(Debug)]
pub struct FsWatcher {
    watcher: Option<RecommendedWatcher>,
    coalescer: Option<thread::JoinHandle<()>>,
}

/// Ownership trasferibile del watcher: `join` chiude il backend prima di
/// raccogliere il coalescer. Non richiede un runtime async.
#[derive(Debug)]
pub struct FsWatcherCollector {
    watcher: Option<RecommendedWatcher>,
    coalescer: Option<thread::JoinHandle<()>>,
}

impl FsWatcherCollector {
    pub fn join(mut self) -> thread::Result<()> {
        self.collect()
    }

    fn collect(&mut self) -> thread::Result<()> {
        drop(self.watcher.take());
        self.coalescer.take().map_or(Ok(()), |thread| thread.join())
    }
}

impl Drop for FsWatcherCollector {
    fn drop(&mut self) {
        let _ = self.collect();
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        drop(FsWatcherCollector {
            watcher: self.watcher.take(),
            coalescer: self.coalescer.take(),
        });
    }
}

impl FsWatcher {
    /// Trasferisce backend e thread al collector, da raccogliere fuori UI.
    /// Il watch rimane attivo fino a `join` o Drop del collector.
    pub fn shutdown(mut self) -> FsWatcherCollector {
        FsWatcherCollector {
            watcher: self.watcher.take(),
            coalescer: self.coalescer.take(),
        }
    }

    /// Osserva soltanto il primo livello delle directory richieste usando una
    /// singola istanza del backend nativo. Nessun discendente viene registrato
    /// implicitamente: il chiamante può quindi mantenere un insieme di watch
    /// proporzionale alle sole cartelle effettivamente visibili o in uso.
    pub fn watch_directories(
        directories: &[PathBuf],
    ) -> notify::Result<(FsWatcher, FsEventReceiver)> {
        let (raw_tx, raw_rx) = fs_event_channel();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Some(event) = raw_event(res) {
                    let _ = raw_tx.send(event);
                }
            })?;
        let mut watched = HashSet::with_capacity(directories.len());
        let mut registered = 0;
        for path in directories {
            if watched.insert(path) {
                match watcher.watch(path, RecursiveMode::NonRecursive) {
                    Ok(()) => registered += 1,
                    // Uno scope può diventare obsoleto fra l'evento UI e la
                    // registrazione nativa (es. cartella eliminata da Explorer).
                    Err(_) if matches!(path.try_exists(), Ok(false)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        if registered == 0 {
            return Err(notify::Error::generic(
                "nessuna directory valida registrata dal watcher",
            ));
        }

        let (tx, rx) = fs_event_channel();
        let coalescer = thread::Builder::new()
            .name("lume-fs-coalesce".into())
            .spawn(move || Self::coalesce_loop(&raw_rx, &tx))
            .map_err(notify::Error::io)?;
        Ok((
            FsWatcher {
                watcher: Some(watcher),
                coalescer: Some(coalescer),
            },
            rx,
        ))
    }

    /// Loop del thread di coalescing: blocca sul primo evento, poi raccoglie
    /// fino alla scadenza della finestra, deduplica e invia. Termina quando
    /// il watcher (lato sender grezzo) o il chiamante (lato receiver) spariscono.
    fn coalesce_loop(raw_rx: &FsEventReceiver, tx: &FsEventSender) {
        while let Ok(first) = raw_rx.recv() {
            let (first, mut rescan) = match first {
                RawFsEvent::Rescan => (Vec::new(), true),
                RawFsEvent::Changed(paths) => (paths, false),
                RawFsEvent::Error(error) => {
                    if tx.send(FsEvent::Error(error)).is_err() {
                        return;
                    }
                    continue;
                }
            };
            let mut seen: HashSet<PathBuf> = HashSet::new();
            let mut bytes = 0;
            accumulate_paths(&mut seen, &mut bytes, &mut rescan, first);
            let deadline = Instant::now() + COALESCE_WINDOW;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match raw_rx.recv_timeout(deadline - now) {
                    Ok(RawFsEvent::Rescan) => {
                        rescan = true;
                        seen.clear();
                        bytes = 0;
                    }
                    Ok(RawFsEvent::Changed(more)) => {
                        accumulate_paths(&mut seen, &mut bytes, &mut rescan, more);
                    }
                    Ok(RawFsEvent::Error(error)) => {
                        if tx.send(FsEvent::Error(error)).is_err() {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    // watcher droppato: invia quanto raccolto e poi esci
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            let event = if rescan {
                FsEvent::Rescan
            } else {
                FsEvent::Changed(seen.into_iter().collect())
            };
            if tx.send(event).is_err() {
                return; // il chiamante ha droppato il receiver
            }
        }
        // raw_rx disconnesso: il watcher è stato droppato, fine del thread
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn output_fermo_fonde_rescan_e_conserva_ultimo_errore() {
        let (tx, rx) = fs_event_channel();
        for index in 0..8 {
            let (raw_tx, raw_rx) = fs_event_channel();
            raw_tx.send(RawFsEvent::Rescan).unwrap();
            raw_tx
                .send(RawFsEvent::Error(format!("errore-{index}")))
                .unwrap();
            drop(raw_tx);
            FsWatcher::coalesce_loop(&raw_rx, &tx);
        }
        drop(tx);
        let events: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            events.len(),
            2,
            "un Rescan e un ultimo errore, senza backlog crescente"
        );
        assert!(events.contains(&FsEvent::Rescan));
        assert!(events.contains(&FsEvent::Error("errore-7".into())));
    }

    #[test]
    fn accumulo_limita_byte_deduplica_e_accetta_la_soglia_esatta() {
        let mut seen = HashSet::new();
        let mut bytes = 0;
        let mut rescan = false;
        let exact = PathBuf::from("a".repeat(MAX_COALESCED_PATH_BYTES));
        accumulate_paths(&mut seen, &mut bytes, &mut rescan, vec![exact.clone()]);
        assert!(!rescan);
        assert_eq!(bytes, MAX_COALESCED_PATH_BYTES);
        accumulate_paths(&mut seen, &mut bytes, &mut rescan, vec![exact]);
        assert!(!rescan, "duplicato non deve consumare budget");
        accumulate_paths(&mut seen, &mut bytes, &mut rescan, vec![PathBuf::from("b")]);
        assert!(rescan);
        assert!(seen.is_empty());
        assert_eq!(bytes, 0);
        accumulate_paths(&mut seen, &mut bytes, &mut rescan, vec![PathBuf::from("c")]);
        assert!(
            seen.is_empty(),
            "nessun dettaglio dopo la perdita del burst"
        );

        let mut seen = HashSet::new();
        let mut bytes = 0;
        let mut rescan = false;
        let paths = (0..MAX_COALESCED_PATHS)
            .map(|i| PathBuf::from(format!("file-{i}")))
            .collect();
        accumulate_paths(&mut seen, &mut bytes, &mut rescan, paths);
        assert!(!rescan);
        assert_eq!(seen.len(), MAX_COALESCED_PATHS);
        accumulate_paths(
            &mut seen,
            &mut bytes,
            &mut rescan,
            vec![PathBuf::from("overflow")],
        );
        assert!(rescan && seen.is_empty());
    }

    #[test]
    fn burst_oltre_limite_path_diventa_rescan_e_conserva_errore() {
        let (raw_tx, raw_rx) = fs_event_channel();
        let (tx, rx) = fs_event_channel();
        let paths = (0..1024)
            .map(|index| PathBuf::from(format!("/fixture/file-{index:04}.txt")))
            .collect();
        raw_tx.send(RawFsEvent::Changed(paths)).unwrap();
        raw_tx
            .send(RawFsEvent::Error("errore corrente".into()))
            .unwrap();
        drop(raw_tx);
        FsWatcher::coalesce_loop(&raw_rx, &tx);
        drop(tx);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(events.contains(&FsEvent::Error("errore corrente".into())));
        assert!(
            events.contains(&FsEvent::Rescan),
            "burst oltre512 path deve invalidare lo scope"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FsEvent::Changed(_)))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "misura nativa serializzata tramite runner"]
    fn misura_backlog_consumer_fermo_native() {
        use std::os::unix::ffi::OsStrExt;
        fn rss() -> u64 {
            fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap()
        }
        let root = make_tempdir("native_queue_measure");
        let (watcher, rx) = FsWatcher::watch_directories(std::slice::from_ref(&root)).unwrap();
        let before = rss();
        for burst in 0..8 {
            for index in 0..1024 {
                fs::write(
                    root.join(format!("burst-{burst}-file-{index:04}.txt")),
                    b"fixture\n",
                )
                .unwrap();
            }
            // Consumer sempre fermo: questa attesa delimita il corpus di burst,
            // non è polling in produzione né una misura di latenza UI.
            thread::sleep(COALESCE_WINDOW + Duration::from_millis(100));
        }
        let live_rss = rss();
        watcher.shutdown().join().unwrap();
        let queued_rss = rss();
        let mut batches = 0;
        let mut paths = 0;
        let mut bytes = 0;
        let mut rescans = 0;
        for event in rx.try_iter() {
            match event {
                FsEvent::Changed(changed) => {
                    batches += 1;
                    paths += changed.len();
                    bytes += changed
                        .iter()
                        .map(|p| p.as_os_str().as_bytes().len())
                        .sum::<usize>();
                }
                FsEvent::Rescan => rescans += 1,
                FsEvent::Error(error) => panic!("errore backend: {error}"),
            }
        }
        assert!(batches > 0 || rescans > 0, "nessun evento nativo osservato");
        fs::remove_dir_all(&root).unwrap();
        println!(
            "WATCHER_QUEUE_NATIVE:bursts=8 files=8192 batches={batches} paths={paths} path_bytes_min={bytes} rescans={rescans} rss_before_kib={before} rss_backlog_live_kib={live_rss} rss_queued_after_close_kib={queued_rss}"
        );
        println!(
            "WATCHER_QUEUE_NATIVE:complete=notify_consumer_stopped_collector_join_drain_fixture_removed"
        );
    }

    #[test]
    fn rescan_senza_percorsi_non_deve_essere_scartato() {
        let event = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        assert!(event.paths.is_empty());
        assert!(event.need_rescan());
        assert!(
            matches!(raw_event(Ok(event)), Some(RawFsEvent::Rescan)),
            "perdita eventi richiede recupero"
        );
    }

    #[test]
    fn rescan_dentro_burst_sostituisce_changed_e_sopravvive_a_shutdown() {
        let (raw_tx, raw_rx) = fs_event_channel();
        let (tx, rx) = fs_event_channel();
        raw_tx
            .send(RawFsEvent::Changed(vec![PathBuf::from("first")]))
            .unwrap();
        raw_tx.send(RawFsEvent::Rescan).unwrap();
        raw_tx.send(RawFsEvent::Rescan).unwrap();
        raw_tx
            .send(RawFsEvent::Changed(vec![PathBuf::from("last")]))
            .unwrap();
        drop(raw_tx);
        FsWatcher::coalesce_loop(&raw_rx, &tx);
        drop(tx);
        assert_eq!(rx.recv().unwrap(), FsEvent::Rescan);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn rescan_come_primo_evento_e_access_flag_non_vengono_filtrati() {
        let event = notify::Event::new(EventKind::Access(notify::event::AccessKind::Any))
            .set_flag(notify::event::Flag::Rescan);
        assert!(matches!(raw_event(Ok(event)), Some(RawFsEvent::Rescan)));
        let (raw_tx, raw_rx) = fs_event_channel();
        let (tx, rx) = fs_event_channel();
        raw_tx.send(RawFsEvent::Rescan).unwrap();
        drop(raw_tx);
        FsWatcher::coalesce_loop(&raw_rx, &tx);
        drop(tx);
        assert_eq!(rx.recv().unwrap(), FsEvent::Rescan);
        assert!(rx.recv().is_err());
    }

    /// Tempdir manuale con nome unico (pid + nanos); da rimuovere a fine test.
    fn make_tempdir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("lume_core_{tag}_{}_{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn changed_paths(event: FsEvent) -> Vec<PathBuf> {
        match event {
            FsEvent::Changed(paths) => paths,
            FsEvent::Error(error) => panic!("errore watcher inatteso: {error}"),
            FsEvent::Rescan => panic!("rescan inatteso nel caso nominale"),
        }
    }

    #[test]
    fn drop_owner_e_collector_chiudono_il_canale_prima_di_tornare() {
        for transfer in [false, true] {
            let root = make_tempdir("watcher-drop");
            let (watcher, rx) = FsWatcher::watch_directories(std::slice::from_ref(&root)).unwrap();
            if transfer {
                drop(watcher.shutdown());
            } else {
                drop(watcher);
            }
            assert!(matches!(
                rx.try_recv(),
                Err(mpsc::TryRecvError::Disconnected)
            ));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn collector_possiede_backend_e_thread_fino_al_join() {
        let root = make_tempdir("watcher-collector");
        let (watcher, rx) = FsWatcher::watch_directories(std::slice::from_ref(&root)).unwrap();
        let collector = watcher.shutdown();
        fs::write(root.join("alive.txt"), "owner vivo").unwrap();
        let paths = changed_paths(rx.recv_timeout(Duration::from_secs(3)).unwrap());
        assert!(paths.iter().any(|path| path.ends_with("alive.txt")));
        collector.join().unwrap();
        while rx.try_recv().is_ok() {}
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn smoke_evento_alla_creazione_di_un_file() {
        let root = make_tempdir("watcher");
        let (watcher, rx) =
            FsWatcher::watch_directories(std::slice::from_ref(&root)).expect("watch avviato");
        // breve pausa per essere certi che il backend nativo sia in ascolto
        thread::sleep(Duration::from_millis(100));

        fs::write(root.join("nuovo.txt"), "ciao").unwrap();

        let ev = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("FsEvent entro 3 secondi");
        let paths = changed_paths(ev);
        assert!(!paths.is_empty());
        assert!(
            paths.iter().any(|p| p.ends_with("nuovo.txt")),
            "atteso un path che termina con nuovo.txt, ricevuti: {paths:?}"
        );

        drop(watcher);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn watch_selettivo_non_scende_nei_discendenti_non_richiesti() {
        let root = make_tempdir("watcher_selective");
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();

        let (watcher, rx) =
            FsWatcher::watch_directories(std::slice::from_ref(&root)).expect("watch avviato");
        thread::sleep(Duration::from_millis(100));

        fs::write(nested.join("deep.txt"), "non deve svegliare il watcher").unwrap();
        assert!(
            matches!(
                rx.recv_timeout(Duration::from_millis(600)),
                Err(RecvTimeoutError::Timeout)
            ),
            "una directory non richiesta è stata osservata ricorsivamente"
        );

        fs::write(root.join("top.txt"), "deve essere osservato").unwrap();
        let paths = changed_paths(
            rx.recv_timeout(Duration::from_secs(3))
                .expect("evento del primo livello"),
        );
        assert!(paths.iter().any(|path| path.ends_with("top.txt")));

        drop(watcher);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn directory_scomparsa_non_disabilita_gli_altri_watch() {
        let root = make_tempdir("watcher_stale");
        let missing = root.join("already-removed");

        let (watcher, rx) = FsWatcher::watch_directories(&[missing, root.clone()])
            .expect("la directory valida resta osservata");
        thread::sleep(Duration::from_millis(100));

        fs::write(root.join("still-watched.txt"), "evento reale").unwrap();
        let paths = changed_paths(
            rx.recv_timeout(Duration::from_secs(3))
                .expect("evento dalla directory valida"),
        );
        assert!(paths.iter().any(|path| path.ends_with("still-watched.txt")));

        drop(watcher);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nessuna_directory_registrata_restituisce_errore() {
        let root = make_tempdir("watcher_all_missing");
        let missing = root.join("missing");

        let error = FsWatcher::watch_directories(&[missing])
            .expect_err("uno scope interamente invalido non è operativo");
        assert!(
            error.to_string().contains("nessuna directory"),
            "errore inatteso: {error}"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn errore_runtime_del_backend_arriva_al_chiamante() {
        let (raw_tx, raw_rx) = fs_event_channel();
        let (tx, rx) = fs_event_channel();
        raw_tx
            .send(RawFsEvent::Error("backend nativo interrotto".into()))
            .unwrap();
        drop(raw_tx);

        FsWatcher::coalesce_loop(&raw_rx, &tx);

        assert_eq!(
            rx.recv().unwrap(),
            FsEvent::Error("backend nativo interrotto".into())
        );
    }
}
