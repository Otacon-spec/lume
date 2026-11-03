//! Letture possedute dall'app; nessun worker finché la future non è interrogata.
//! La raccolta esplicita va eseguita fuori UI; Drop è il fallback sincrono.
use futures::{FutureExt, future::BoxFuture};
use std::sync::{Arc, Mutex};

/// Durata del consumatore, distinta dall'owner del worker già avviato.
#[derive(Clone, Debug)]
pub(crate) struct DeliveryStatus(Arc<std::sync::atomic::AtomicBool>);
#[derive(Debug)]
pub(crate) struct Delivery {
    status: DeliveryStatus,
    delivered: std::sync::atomic::AtomicBool,
}
impl DeliveryStatus {
    pub(crate) fn new() -> (Self, Delivery) {
        let status = Self(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        (
            status.clone(),
            Delivery {
                status,
                delivered: std::sync::atomic::AtomicBool::new(false),
            },
        )
    }
    pub(crate) fn abandoned(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}
impl Delivery {
    pub(crate) fn delivered(&self) {
        self.delivered
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        if !self.delivered.load(std::sync::atomic::Ordering::Acquire) {
            self.status
                .0
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

struct State {
    closed: bool,
    workers: Vec<std::thread::JoinHandle<()>>,
}
struct Shared {
    state: Mutex<State>,
    slots: Arc<tokio::sync::Semaphore>,
    revoked: tokio::sync::Semaphore,
    collecting: Mutex<()>,
    #[cfg(test)]
    collection_started: Arc<std::sync::atomic::AtomicBool>,
}
pub(crate) struct Readers {
    shared: Arc<Shared>,
}
/// Fallback dell'app anche quando il collector ha già trasferito i handle
/// al lavoro blocking. Drop attende lo stesso mutex di raccolta.
pub(crate) struct CollectionOwner {
    readers: Readers,
}
impl CollectionOwner {
    pub(crate) fn finished(&self) -> bool {
        let Ok(_collection) = self.readers.shared.collecting.try_lock() else {
            return false;
        };
        !self.readers.has_workers()
    }
}
impl Default for Readers {
    fn default() -> Self {
        Self::with_slots(Arc::new(tokio::sync::Semaphore::new(2)))
    }
}
impl Readers {
    fn with_slots(slots: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    closed: false,
                    workers: Vec::new(),
                }),
                slots,
                revoked: tokio::sync::Semaphore::new(0),
                collecting: Mutex::new(()),
                #[cfg(test)]
                collection_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
        }
    }
    pub(crate) fn successor(&self) -> Self {
        Self::with_slots(self.shared.slots.clone())
    }

    pub(crate) fn collection_owner(&self) -> CollectionOwner {
        self.close();
        CollectionOwner {
            readers: Readers {
                shared: self.shared.clone(),
            },
        }
    }

    pub(crate) fn read<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> BoxFuture<'static, Result<T, String>> {
        let shared = self.shared.clone();
        async move {
            let permit = tokio::select! {
                biased;
                _ = shared.revoked.acquire() => return Err("Letture chiuse".to_owned()),
                permit = shared.slots.clone().acquire_owned() =>
                    permit.map_err(|_| "Letture chiuse".to_owned())?,
            };
            let (sender, receiver) = futures::channel::oneshot::channel();
            {
                let mut state = shared
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if state.closed {
                    return Err("Letture chiuse".to_owned());
                }
                // Solo join già terminati: nessuna attesa del filesystem qui.
                let mut index = 0;
                while index < state.workers.len() {
                    if state
                        .workers
                        .get(index)
                        .is_some_and(std::thread::JoinHandle::is_finished)
                    {
                        let worker = state.workers.swap_remove(index);
                        let _ = worker.join();
                    } else {
                        index += 1;
                    }
                }
                let worker = std::thread::Builder::new()
                    .name("lume-file-read".into())
                    .spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                            .map_err(|_| "Worker lettura terminato con panic".to_owned());
                        let _ = sender.send(result);
                        drop(permit);
                    })
                    .map_err(|error| error.to_string())?;
                // Pubblicazione sotto lo stesso lock della chiusura: niente
                // worker avviato fuori dal dominio del collector.
                state.workers.push(worker);
            }
            receiver
                .await
                .map_err(|_| "Risultato lettura non disponibile".to_owned())?
        }
        .boxed()
    }

    pub(crate) fn has_workers(&self) -> bool {
        !self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .workers
            .is_empty()
    }

    pub(crate) fn close(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .closed = true;
        self.shared.revoked.close();
    }

    pub(crate) fn collect(&self) -> bool {
        let _collection = self
            .shared
            .collecting
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let workers = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.closed = true;
            self.shared.revoked.close();
            std::mem::take(&mut state.workers)
        };
        let mut success = true;
        #[cfg(test)]
        self.shared
            .collection_started
            .store(true, std::sync::atomic::Ordering::Release);
        for worker in workers {
            success &= worker.join().is_ok();
        }
        success
    }

    #[cfg(test)]
    pub(crate) fn collection_started_probe(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.shared.collection_started.clone()
    }
}
impl Drop for Readers {
    fn drop(&mut self) {
        let _ = self.collect();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use futures::FutureExt;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    #[ignore = "profilo release dispatch e stage, avviare esplicitamente"]
    fn benchmark_dispatch_and_file_stages() {
        use sha2::{Digest, Sha256};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let readers = Readers::default();
        runtime
            .block_on(async { tokio::task::spawn_blocking(|| 42).await })
            .unwrap();
        futures::executor::block_on(readers.read(|| 42)).unwrap();
        let mut owned_ns = Vec::new();
        let mut tokio_ns = Vec::new();
        for sample in 0..101 {
            for owned in if sample % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let start = std::time::Instant::now();
                let value = if owned {
                    futures::executor::block_on(readers.read(|| 42)).unwrap()
                } else {
                    runtime
                        .block_on(async { tokio::task::spawn_blocking(|| 42).await })
                        .unwrap()
                };
                let elapsed = start.elapsed().as_nanos();
                assert_eq!(value, 42);
                if owned {
                    owned_ns.push(elapsed);
                } else {
                    tokio_ns.push(elapsed);
                }
            }
        }
        println!(
            "READER_NO_IO:{}",
            serde_json::json!({"owned_ns":owned_ns,"tokio_ns":tokio_ns,"samples":101})
        );
        let dir = tempfile::tempdir().unwrap();
        for mib in [1_usize, 8, 31] {
            let path = dir.path().join(format!("stage-{mib}.txt"));
            std::fs::write(&path, vec![b'R'; mib * 1024 * 1024]).unwrap();
            let expected =
                crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
                    .unwrap()
                    .fingerprint();
            let mut samples = Vec::new();
            for _ in 0..31 {
                let start = std::time::Instant::now();
                let bytes =
                    crate::file_read::read_bytes_capped(&path, crate::file_read::MAX_FILE_BYTES)
                        .unwrap();
                let read_ns = start.elapsed().as_nanos();
                let capacity = bytes.capacity();
                let start = std::time::Instant::now();
                let fingerprint: [u8; 32] = Sha256::digest(&bytes).into();
                let hash_ns = start.elapsed().as_nanos();
                let start = std::time::Instant::now();
                let text = String::from_utf8(bytes).unwrap();
                let utf8_ns = start.elapsed().as_nanos();
                let start = std::time::Instant::now();
                let text: Arc<str> = Arc::from(text);
                let arc_ns = start.elapsed().as_nanos();
                assert_eq!(fingerprint, expected);
                assert_eq!(text.len(), mib * 1024 * 1024);
                std::hint::black_box(&text);
                samples.push(serde_json::json!({"read_ns":read_ns,"hash_ns":hash_ns,"utf8_ns":utf8_ns,"arc_ns":arc_ns,"capacity":capacity}));
            }
            println!(
                "READER_STAGES:{}",
                serde_json::json!({"bytes":mib*1024*1024,"samples":samples})
            );
        }
        assert!(readers.collect());
    }

    #[test]
    fn worker_panic_returns_error_and_releases_both_slots() {
        let readers = Readers::default();
        let result =
            futures::executor::block_on(readers.read(|| panic!("panic controllato reader")));
        assert_eq!(result, Err("Worker lettura terminato con panic".to_owned()));
        let workers = std::mem::take(&mut readers.shared.state.lock().unwrap().workers);
        for worker in workers {
            worker.join().unwrap();
        }
        let permit = readers
            .shared
            .slots
            .clone()
            .try_acquire_many_owned(2)
            .unwrap();
        drop(permit);
        assert_eq!(futures::executor::block_on(readers.read(|| 42)), Ok(42));
        assert!(readers.collect());
    }

    #[test]
    #[ignore = "sonda release seriale su file reali, avviare esplicitamente"]
    fn benchmark_owned_reader_against_tokio_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let readers = Readers::default();
        for mib in [1_usize, 8, 31] {
            let path = dir.path().join(format!("reader-{mib}.txt"));
            std::fs::write(&path, vec![b'R'; mib * 1024 * 1024]).unwrap();
            let expected =
                crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
                    .unwrap()
                    .fingerprint();
            let mut owned_ns = Vec::new();
            let mut tokio_ns = Vec::new();
            // Warm-up escluso: runtime/blocking pool e pagina filesystem.
            let warm = path.clone();
            runtime
                .block_on(async move {
                    tokio::task::spawn_blocking(move || {
                        crate::file_read::read_testo_capped(&warm, crate::file_read::MAX_FILE_BYTES)
                    })
                    .await
                })
                .unwrap()
                .unwrap();
            let warm = path.clone();
            futures::executor::block_on(readers.read(move || {
                crate::file_read::read_testo_capped(&warm, crate::file_read::MAX_FILE_BYTES)
            }))
            .unwrap()
            .unwrap();
            for sample in 0..31 {
                for owned in if sample % 2 == 0 {
                    [true, false]
                } else {
                    [false, true]
                } {
                    let worker_path = path.clone();
                    let before = std::time::Instant::now();
                    let result = if owned {
                        futures::executor::block_on(readers.read(move || {
                            crate::file_read::read_testo_capped(
                                &worker_path,
                                crate::file_read::MAX_FILE_BYTES,
                            )
                        }))
                        .unwrap()
                        .unwrap()
                    } else {
                        runtime
                            .block_on(async move {
                                tokio::task::spawn_blocking(move || {
                                    crate::file_read::read_testo_capped(
                                        &worker_path,
                                        crate::file_read::MAX_FILE_BYTES,
                                    )
                                })
                                .await
                            })
                            .unwrap()
                            .unwrap()
                    };
                    let elapsed = before.elapsed().as_nanos();
                    assert_eq!(result.fingerprint(), expected);
                    assert_eq!(result.len(), mib * 1024 * 1024);
                    drop(result);
                    if owned {
                        owned_ns.push(elapsed);
                    } else {
                        tokio_ns.push(elapsed);
                    }
                }
            }
            println!(
                "READER_DISPATCH:{}",
                serde_json::json!({"bytes":mib*1024*1024,"samples":31,"owned_ns":owned_ns,"tokio_ns":tokio_ns,"fingerprint":format!("{expected:x?}")})
            );
        }
        assert!(readers.collect());
    }

    #[test]
    fn read_and_collection_need_no_tokio_runtime() {
        let readers = Readers::default();
        assert_eq!(futures::executor::block_on(readers.read(|| 42)), Ok(42));
        assert!(readers.collect());
        assert!(futures::executor::block_on(readers.read(|| 7)).is_err());
    }

    #[test]
    fn third_read_waits_and_cold_waiter_is_revocable() {
        queued_reads_are_revocable(1, false);
    }

    #[test]
    fn pressure_waiters_drop_and_shutdown_start_no_extra_work() {
        queued_reads_are_revocable(256, true);
    }

    fn queued_reads_are_revocable(count: usize, close: bool) {
        let readers = Readers::default();
        let mut active = Vec::new();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let mut task = readers.read(move || {
                started_tx.send(()).unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            });
            assert!(task.as_mut().now_or_never().is_none());
            started_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            active.push(task);
            releases.push(release_tx);
        }
        let ran = Arc::new(AtomicBool::new(false));
        let mut waiters = Vec::new();
        for _ in 0..count {
            let worker_ran = ran.clone();
            let mut waiting = readers.read(move || worker_ran.store(true, Ordering::Release));
            assert!(waiting.as_mut().now_or_never().is_none());
            waiters.push(waiting);
        }
        assert_eq!(readers.shared.state.lock().unwrap().workers.len(), 2);
        assert!(!ran.load(Ordering::Acquire));
        if close {
            drop(waiters.drain(..count / 2).collect::<Vec<_>>());
            readers.close();
            for waiting in waiters {
                assert!(futures::executor::block_on(waiting).is_err());
            }
        } else {
            drop(waiters);
        }
        for release in releases {
            release.send(()).unwrap();
        }
        for task in active {
            assert!(futures::executor::block_on(task).is_ok());
        }
        assert!(readers.collect());
        assert!(!ran.load(Ordering::Acquire));
    }

    #[test]
    fn cold_drop_starts_no_work() {
        let readers = Readers::default();
        let ran = Arc::new(AtomicBool::new(false));
        let worker_ran = ran.clone();
        drop(readers.read(move || worker_ran.store(true, Ordering::Release)));
        drop(readers);
        assert!(!ran.load(Ordering::Acquire));
    }

    #[test]
    fn started_consumer_drop_leaves_owner_collecting() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let readers = Readers::default();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = finished.clone();
        let mut task = readers.read(move || {
            started_tx.send(()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            worker_finished.store(true, Ordering::Release);
        });
        runtime.block_on(async {
            assert!(task.as_mut().now_or_never().is_none());
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        drop(task);
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(25));
            let _ = release_tx.send(());
        });
        drop(readers);
        let collected = finished.load(Ordering::Acquire);
        release.join().unwrap();
        assert!(collected, "owner Drop deve raccogliere il reader avviato");
    }
}
