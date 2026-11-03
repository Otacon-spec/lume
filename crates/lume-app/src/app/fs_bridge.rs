use std::{path::PathBuf, thread};

/// Identità della recipe; gli Arc nei messaggi impediscono riuso dell'identità
/// finché una consegna precedente è ancora in volo.
#[derive(Clone, Debug)]
pub(crate) struct WatchScope(std::sync::Arc<WatchConfiguration>);

#[derive(Debug)]
struct WatchConfiguration {
    root_epoch: u64,
    directories: Vec<PathBuf>,
}

impl WatchScope {
    pub(super) fn new(root_epoch: u64, directories: Vec<PathBuf>) -> Self {
        Self(std::sync::Arc::new(WatchConfiguration {
            root_epoch,
            directories,
        }))
    }

    pub(super) fn matches(&self, root_epoch: u64, directories: &[PathBuf]) -> bool {
        self.0.root_epoch == root_epoch && self.0.directories == directories
    }

    pub(super) fn same_recipe(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn directories(&self) -> &[PathBuf] {
        &self.0.directories
    }
}

impl std::hash::Hash for WatchScope {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&std::sync::Arc::as_ptr(&self.0), state);
    }
}

// Osservabilità del child isolato: pubblicata soltanto dopo l'invio riuscito.
// Non è compilata nel prodotto; non altera ownership o canali della recipe.
#[cfg(all(test, target_os = "linux"))]
static LAST_DELIVERY: std::sync::Mutex<Option<lume_core::FsEvent>> = std::sync::Mutex::new(None);

/// Adattamento Stream della mailbox core, senza coda aggiuntiva.
pub(super) struct BridgeEvents(lume_core::FsEventReceiver);
impl futures::Stream for BridgeEvents {
    type Item = lume_core::FsEvent;
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().0.poll_next(cx)
    }
}

/// Owner del backend nativo, coalescer e ponte std→async. Drop chiude prima
/// il watcher per liberare recv, poi raccoglie il ponte senza polling.
pub(super) struct FsBridge {
    watcher: Option<lume_core::FsWatcher>,
    bridge: Option<thread::JoinHandle<()>>,
}

impl FsBridge {
    pub(super) fn start(
        directories: &[PathBuf],
    ) -> Result<(Self, BridgeEvents), Box<dyn std::error::Error + Send + Sync>> {
        let (watcher, events) = lume_core::FsWatcher::watch_directories(directories)?;
        let (sender, receiver) = lume_core::fs_event_channel();
        let bridge = thread::Builder::new()
            .name("lume-fs-bridge".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    #[cfg(all(test, target_os = "linux"))]
                    let delivered = event.clone();
                    if sender.send(event).is_err() {
                        break;
                    }
                    #[cfg(all(test, target_os = "linux"))]
                    {
                        *LAST_DELIVERY
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) = Some(delivered);
                    }
                }
            })?;
        Ok((
            Self {
                watcher: Some(watcher),
                bridge: Some(bridge),
            },
            BridgeEvents(receiver),
        ))
    }
}

impl Drop for FsBridge {
    fn drop(&mut self) {
        drop(self.watcher.take());
        if let Some(bridge) = self.bridge.take() {
            let _ = bridge.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[cfg(target_os = "linux")]
    struct PressureWriter {
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        writes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        worker: Option<thread::JoinHandle<std::io::Result<usize>>>,
    }

    #[cfg(target_os = "linux")]
    impl PressureWriter {
        fn start(path: PathBuf) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
            use std::sync::{
                Arc,
                atomic::{AtomicBool, AtomicUsize, Ordering},
                mpsc,
            };
            let stop = Arc::new(AtomicBool::new(false));
            let writes = Arc::new(AtomicUsize::new(0));
            let (ready, started) = mpsc::channel();
            let stopping = stop.clone();
            let progress = writes.clone();
            let worker = thread::Builder::new()
                .name("lume-pressure".into())
                .spawn(move || {
                    while !stopping.load(Ordering::Acquire) {
                        std::fs::write(&path, b"writer nativo attivo")?;
                        let count = progress.fetch_add(1, Ordering::SeqCst) + 1;
                        if count == 1 {
                            let _ = ready.send(());
                        }
                    }
                    Ok(progress.load(Ordering::SeqCst))
                })?;
            let owner = Self {
                stop,
                writes,
                worker: Some(worker),
            };
            started.recv_timeout(std::time::Duration::from_secs(5))?;
            Ok(owner)
        }

        fn count(&self) -> usize {
            self.writes.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn is_running(&self) -> bool {
            !self.stop.load(std::sync::atomic::Ordering::Acquire)
                && self
                    .worker
                    .as_ref()
                    .is_some_and(|worker| !worker.is_finished())
        }

        fn finish(mut self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            let worker = self.worker.take().ok_or("writer già raccolto")?;
            Ok(worker.join().map_err(|_| "writer pressure panic")??)
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for PressureWriter {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn await_worker_departure(
        owned: &std::collections::BTreeMap<u32, String>,
    ) -> Result<std::time::Duration, Box<dyn std::error::Error + Send + Sync>> {
        let start = std::time::Instant::now();
        loop {
            let after = worker_identities();
            let remaining: Vec<_> = owned
                .iter()
                .filter(|(id, birth)| after.get(id) == Some(*birth))
                .map(|(id, _)| *id)
                .collect();
            if remaining.is_empty() {
                return Ok(start.elapsed());
            }
            if start.elapsed() >= std::time::Duration::from_secs(1) {
                return Err(format!("worker identities did not disappear: {remaining:?}").into());
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[cfg(target_os = "linux")]
    fn worker_identities() -> std::collections::BTreeMap<u32, String> {
        let mut found = std::collections::BTreeMap::new();
        if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
            for entry in tasks.flatten() {
                let Ok(name) = std::fs::read_to_string(entry.path().join("comm")) else {
                    continue;
                };
                // comm è troncato a 15 byte: include anche il backend inotify.
                if !name.starts_with("lume-fs-") && !name.starts_with("notify-rs inoti") {
                    continue;
                }
                let Ok(id) = entry.file_name().to_string_lossy().parse() else {
                    continue;
                };
                if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat"))
                    && let Some((_, fields)) = stat.rsplit_once(')')
                    && let Some(start) = fields.split_whitespace().nth(19)
                {
                    found.insert(id, start.to_owned());
                }
            }
        }
        found
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recipe_drop_collects_workers_with_and_without_original_runtime()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        struct ChildOwner(Option<std::process::Child>);
        impl Drop for ChildOwner {
            fn drop(&mut self) {
                if let Some(mut child) = self.0.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        let dir = tempfile::tempdir()?;
        let log = dir.path().join("child.log");
        let output = std::fs::File::create(&log)?;
        let child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "app::fs_bridge::tests::recipe_drop_isolated_child",
                "--ignored",
                "--show-output",
            ])
            .env("LUME_FS_RECIPE_CHILD", "1")
            .env("TMPDIR", dir.path())
            .stdin(std::process::Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output)
            .spawn()?;
        let mut owner = ChildOwner(Some(child));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let status = loop {
            let child = owner.0.as_mut().ok_or("child non posseduto")?;
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                return Err("child recipe timeout".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        owner.0.take(); // try_wait ha raccolto il processo terminale.
        let text = std::fs::read_to_string(log)?;
        eprintln!("{text}");
        assert!(status.success());
        assert!(text.contains("1 passed; 0 failed; 0 ignored;"));
        assert!(text.contains("FS_RECIPE_DROP:runtime_loss=false"));
        assert!(text.contains("FS_RECIPE_DROP:runtime_loss=true"));
        assert!(text.contains("FS_BRIDGE_BACKLOG:queued="));
        assert!(text.contains(
            "FS_BRIDGE_SATURATION:files=8192 rescan=1 closed=true drained_without_runtime=true"
        ));
        assert!(text.contains("FS_RECIPE_BACKLOG:runtime_loss=false"));
        assert!(text.contains("FS_RECIPE_BACKLOG:runtime_loss=true"));
        assert!(text.contains("FS_RECIPE_PRESSURE:runtime_loss=false"));
        assert!(text.contains("FS_RECIPE_PRESSURE:runtime_loss=true"));
        assert!(text.contains("threads_absent=3"));
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "helper child isolato, eseguito dal test padre recipe_drop"]
    fn recipe_drop_isolated_child() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::app::{Message, fs_subscription};
        use iced::advanced::subscription::into_recipes;
        use std::time::{Duration, Instant};
        if std::env::var("LUME_FS_RECIPE_CHILD").as_deref() != Ok("1") {
            return Err("helper richiede il test padre".into());
        }
        for runtime_loss in [false, true] {
            let dir = tempfile::tempdir()?;
            let baseline = worker_identities();
            let recipe = into_recipes(fs_subscription(WatchScope::new(
                0,
                vec![dir.path().to_path_buf()],
            )))
            .pop()
            .ok_or("recipe watcher assente")?;
            let mut stream = recipe.stream(futures::stream::empty().boxed());
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let path = dir.path().join("recipe.txt");
            let owned = runtime.block_on(async {
                let mut next = Box::pin(stream.next());
                let owned = tokio::time::timeout(Duration::from_secs(5), async {
                    tokio::select! {
                        _ = next.as_mut() => panic!("evento prima della scrittura fixture"),
                        identities = async {
                            loop {
                                let identities: std::collections::BTreeMap<_, _> = worker_identities().into_iter()
                                    .filter(|(id, start)| baseline.get(id) != Some(start)).collect();
                                if identities.len() >= 3 { break identities; }
                                tokio::time::sleep(Duration::from_millis(2)).await;
                            }
                        } => identities,
                    }
                }).await?;
                std::fs::write(&path, "evento reale")?;
                let event = tokio::time::timeout(Duration::from_secs(5), next).await?
                    .ok_or("recipe terminata")?;
                assert!(matches!(event, Message::FsChanged(paths) if paths.contains(&path)));
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(owned)
            })?;
            assert_eq!(owned.len(), 3);
            // La recipe non viene più polled: il nuovo evento deve arrivare
            // nel suo canale interno, non essere consumato da stream.next().
            let sentinel = dir.path().join("recipe-backlog-511.txt");
            for index in 0..512 {
                std::fs::write(
                    dir.path().join(format!("recipe-backlog-{index}.txt")),
                    "pending",
                )?;
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let delivered = LAST_DELIVERY
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let queued = matches!(delivered.as_ref(), Some(lume_core::FsEvent::Changed(paths)) if paths.contains(&sentinel));
                drop(delivered);
                if queued {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err("recipe: sentinel non consegnato nel backlog".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let pressure_path = dir.path().join("pressure.txt");
            let writer = PressureWriter::start(pressure_path.clone())?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let delivered = LAST_DELIVERY
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let pressure = matches!(delivered.as_ref(), Some(lume_core::FsEvent::Changed(paths)) if paths.contains(&pressure_path));
                drop(delivered);
                if pressure && writer.count() >= 32 {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err("writer: consegna pressure non osservata".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(writer.is_running());
            let mut runtime = Some(runtime);
            if runtime_loss {
                drop(runtime.take());
            }
            let writes_before = writer.count();
            let before = Instant::now();
            drop(stream);
            let elapsed = before.elapsed();
            let writes_after = writer.count();
            assert!(writer.is_running());
            // join può precedere la rimozione del task da /proc. Il tempo
            // osservazionale è separato dalla durata Drop già acquisita.
            let departure = await_worker_departure(&owned)?;
            eprintln!(
                "FS_THREAD_DEPARTURE:runtime_loss={runtime_loss} after_drop_ms={:.6}",
                departure.as_secs_f64() * 1000.0
            );
            eprintln!(
                "FS_RECIPE_DROP:runtime_loss={runtime_loss} raccolta_ms={:.6}",
                elapsed.as_secs_f64() * 1000.0
            );
            eprintln!(
                "FS_RECIPE_BACKLOG:runtime_loss={runtime_loss} files=512 sentinel_consegnato=true consumer_non_polled=true"
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            while writer.count() <= writes_before {
                if Instant::now() >= deadline {
                    return Err("writer fermato insieme al watcher".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let final_writes = writer.finish()?;
            eprintln!(
                "FS_RECIPE_PRESSURE:runtime_loss={runtime_loss} writes_before={writes_before} writes_after={writes_after} final_writes={final_writes} writer_joined=true threads_absent=3"
            );
        }
        // Non consumare il receiver: prova la cancellazione con consegne
        // native già accodate, senza confondere write con consegna notify.
        let dir = tempfile::tempdir()?;
        let baseline = worker_identities();
        let (owner, events) = FsBridge::start(&[dir.path().to_path_buf()])?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let owned = loop {
            let identities: std::collections::BTreeMap<_, _> = worker_identities()
                .into_iter()
                .filter(|(id, start)| baseline.get(id) != Some(start))
                .collect();
            if identities.len() >= 3 {
                break identities;
            }
            if Instant::now() >= deadline {
                return Err("worker backlog non osservati".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(owned.len(), 3);
        for index in 0..512 {
            std::fs::write(dir.path().join(format!("pending-{index}.txt")), "evento")?;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let queued = loop {
            let queued = futures::Stream::size_hint(&events).0;
            if queued > 0 {
                break queued;
            }
            if Instant::now() >= deadline {
                return Err("nessuna consegna nativa accodata".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        // Come la cancellazione del consumer: il destinatario scompare
        // prima della raccolta, con backend/coalescer ancora posseduti.
        drop(events);
        let before = Instant::now();
        drop(owner);
        let elapsed = before.elapsed();
        let departure = await_worker_departure(&owned)?;
        eprintln!(
            "FS_BRIDGE_THREAD_DEPARTURE:after_drop_ms={:.6}",
            departure.as_secs_f64() * 1000.0
        );
        eprintln!(
            "FS_BRIDGE_BACKLOG:queued={queued} files=512 raccolta_ms={:.6}",
            elapsed.as_secs_f64() * 1000.0
        );

        // Pipeline intera, consumer fermo durante otto burst nativi. Nessun
        // runtime e nessuna factory di eventi; il ponte non attende spazio.
        let dir = tempfile::tempdir()?;
        let (owner, events) = FsBridge::start(&[dir.path().to_path_buf()])?;
        for burst in 0..8 {
            for index in 0..1024 {
                std::fs::write(
                    dir.path()
                        .join(format!("saturation-{burst}-{index:04}.txt")),
                    "fixture\n",
                )?;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(futures::Stream::size_hint(&events).0 <= 2);
        }
        drop(owner);
        assert_eq!(futures::Stream::size_hint(&events), (1, Some(1)));
        let retained = futures::executor::block_on(events.collect::<Vec<_>>());
        assert_eq!(retained, vec![lume_core::FsEvent::Rescan]);
        eprintln!(
            "FS_BRIDGE_SATURATION:files=8192 rescan=1 closed=true drained_without_runtime=true"
        );
        Ok(())
    }

    #[tokio::test]
    async fn native_event_and_drop_collect_both_workers()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let (owner, mut events) = FsBridge::start(&[dir.path().to_path_buf()])?;
        let path = dir.path().join("città.txt");
        std::fs::write(&path, "evento 😀")?;
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
            .await?
            .ok_or("stream chiuso prima dell'evento")?;
        assert!(matches!(event, lume_core::FsEvent::Changed(paths) if paths.contains(&path)));
        drop(owner);
        while events.next().await.is_some() {}
        Ok(())
    }
}
