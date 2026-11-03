//! Cerca: producer posseduto separatamente dalla data Task, raccolta su
//! blocking worker condiviso e un solo messaggio UI in volo con ack.
use futures::{FutureExt, Stream, StreamExt, channel::oneshot};
use lume_core::{SearchEvents, SearchOptions, SearchOrigin, SearchScope, SearchUpdate};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};
type ResultJoin = Result<(), String>;
type Handle = futures::future::Shared<futures::future::BoxFuture<'static, ResultJoin>>;
struct Input {
    scope: SearchScope,
    options: SearchOptions,
}
enum Phase {
    Pending(Option<Input>),
    Starting,
    Running(Handle),
    Done(ResultJoin),
}
struct State {
    phase: Phase,
    canceled: bool,
    origin: Option<SearchOrigin>,
}
struct Inner {
    id: u64,
    generation: Arc<AtomicU64>,
    expected: u64,
    state: Mutex<State>,
    wake: tokio::sync::Notify,
    predecessor: Mutex<Option<Job>>,
}
fn lock(job: &Job) -> MutexGuard<'_, State> {
    job.0
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
#[derive(Clone)]
pub(crate) struct Job(Arc<Inner>);
impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchJob").finish_non_exhaustive()
    }
}
impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Job {}
impl Hash for Job {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Arc::as_ptr(&self.0).hash(h);
    }
}
impl Job {
    pub(crate) fn id(&self) -> u64 {
        self.0.id
    }
    pub(crate) fn generation(&self) -> u64 {
        self.0.expected
    }
    pub(crate) fn is_current(&self) -> bool {
        // Scope della richiesta, non esito del producer. Un guard che
        // cancella il core per un panic non deve mascherare l'errore come
        // abbandono volontario del consumer.
        !lock(self).canceled && self.0.generation.load(Ordering::SeqCst) == self.0.expected
    }
    pub(crate) fn cancel(&self) {
        let origin = {
            let mut s = lock(self);
            s.canceled = true;
            if matches!(s.phase, Phase::Pending(_)) {
                s.phase = Phase::Done(Ok(()));
            }
            s.origin.clone()
        };
        if let Some(origin) = origin {
            origin.cancel();
        }
        let predecessor = self
            .0
            .predecessor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(predecessor);
        self.0.wake.notify_waiters();
    }
    async fn canceled(&self) {
        loop {
            let n = self.0.wake.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if lock(self).canceled {
                return;
            }
            n.await;
        }
    }
    pub(crate) fn collected(&self) -> bool {
        matches!(lock(self).phase, Phase::Done(_))
    }
    fn matches_result(&self, result: &ResultJoin) -> bool {
        matches!(&lock(self).phase, Phase::Done(actual) if actual == result)
    }
    pub(crate) fn succeeded(&self) -> bool {
        self.matches_result(&Ok(()))
    }
    fn has_producer(&self) -> bool {
        matches!(lock(self).phase, Phase::Starting | Phase::Running(_))
    }
    async fn start(&self) -> Option<SearchEvents> {
        let predecessor = self
            .0
            .predecessor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(old) = predecessor {
            tokio::select! { _=self.canceled()=>return None, _=old.collect()=>{} }
        }
        if !self.is_current() {
            self.cancel();
            return None;
        }
        let runtime = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => runtime,
            Err(_) => {
                self.done(Err(
                    "Runtime Cerca non disponibile: ricerca non avviata".into()
                ));
                return None;
            }
        };
        let input = {
            let mut s = lock(self);
            if s.canceled {
                return None;
            }
            let Phase::Pending(input) = &mut s.phase else {
                return None;
            };
            let input = input.take()?;
            s.phase = Phase::Starting;
            input
        };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let job = self.clone();
        // Ammettere il collector PRIMA di creare il producer. L'handle std
        // non passa mai nella data Task; il suo Drop di sicurezza è su worker.
        let handle = runtime.spawn_blocking(move || {
            if !job.is_current() {
                return Ok(());
            }
            let run = lume_core::spawn_search_owned(
                input.scope,
                input.options,
                Arc::clone(&job.0.generation),
                job.0.expected,
            )
            .map_err(|error| format!("Impossibile avviare Cerca: {error}"))?;
            let origin = run.events.origin();
            let mut producer = Producer {
                origin: origin.clone(),
                worker: Some(run.worker),
            };
            let canceled = {
                let mut s = lock(&job);
                s.origin = Some(origin.clone());
                s.canceled
            };
            if canceled {
                origin.cancel();
            }
            let _ = sender.send(run.events); // Err restituisce e droppa la source
            producer.join()
        });
        let completion = async move {
            handle.await.unwrap_or_else(|_| {
                Err("Raccolta Cerca terminata con errore: esito da verificare".into())
            })
        }
        .boxed()
        .shared();
        lock(self).phase = Phase::Running(completion);
        self.0.wake.notify_waiters();
        tokio::select! { _=self.canceled()=>None, result=receiver=>result.ok() }
    }
    fn done(&self, result: ResultJoin) {
        lock(self).phase = Phase::Done(result);
        self.0.wake.notify_waiters();
    }
    pub(crate) async fn collect(&self) -> ResultJoin {
        loop {
            let notified = self.0.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let handle = {
                let s = lock(self);
                match &s.phase {
                    Phase::Done(result) => return result.clone(),
                    Phase::Running(handle) => Some(handle.clone()),
                    _ => None,
                }
            };
            if let Some(handle) = handle {
                // Shared resta nella cella; un collector vivo ma non pollato
                // non può monopolizzare l'handle e bloccare close/fallback.
                // Await e callback del future avvengono fuori dal mutex.
                let result = handle.await;
                self.done(result.clone());
                return result;
            }
            notified.await;
        }
    }
}
// Creato/distrutto esclusivamente nella closure spawn_blocking: un panic
// prima della raccolta non stacca il producer standard né blocca la UI.
struct Producer {
    origin: SearchOrigin,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Producer {
    fn join(&mut self) -> ResultJoin {
        match self.worker.take() {
            Some(worker) => worker.join().map_err(|_| {
                "Il producer Cerca è terminato con errore: risultato incompleto".into()
            }),
            None => Ok(()),
        }
    }
}
impl Drop for Producer {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.origin.cancel();
            let _ = self.join();
        }
    }
}

#[derive(Default)]
pub(crate) struct Jobs {
    sequence: u64,
    jobs: BTreeMap<u64, Job>,
    current: Option<Job>,
}
impl Jobs {
    pub(crate) fn reserve(
        &mut self,
        scope: SearchScope,
        options: SearchOptions,
        generation: Arc<AtomicU64>,
        expected: u64,
    ) -> Result<Job, String> {
        self.cancel();
        // Un producer alla volta. Config nuove non iniziate vengono revocate
        // e liberate subito; non accumulare IO/workers al ritmo dei tasti.
        self.jobs.retain(|_, job| job.has_producer());
        let predecessor = self.jobs.values().next_back().cloned();
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("Identificativi Cerca esauriti")?;
        let job = Job(Arc::new(Inner {
            id: self.sequence,
            generation,
            expected,
            state: Mutex::new(State {
                phase: Phase::Pending(Some(Input { scope, options })),
                canceled: false,
                origin: None,
            }),
            wake: tokio::sync::Notify::new(),
            predecessor: Mutex::new(predecessor),
        }));
        self.jobs.insert(job.id(), job.clone());
        self.current = Some(job.clone());
        Ok(job)
    }
    pub(crate) fn cancel(&self) {
        if let Some(job) = &self.current {
            job.cancel();
        }
    }
    pub(crate) fn remove(&mut self, id: u64, result: &ResultJoin) -> bool {
        if !self
            .jobs
            .get(&id)
            .is_some_and(|job| job.matches_result(result))
        {
            return false;
        }
        self.jobs.remove(&id).is_some()
    }
    pub(crate) fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot(self.jobs.values().cloned().collect())
    }
    pub(crate) fn current(&self) -> Option<Job> {
        self.current.clone()
    }
}
impl Drop for Jobs {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel();
        }
        self.cancel();
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Snapshot(pub(crate) Vec<Job>);
type ExitHandle = futures::future::Shared<futures::future::BoxFuture<'static, bool>>;
#[derive(Clone)]
pub(crate) struct ExitCompletion(Arc<(u64, ExitHandle)>);
impl ExitCompletion {
    pub(crate) fn new(epoch: u64, handle: ExitHandle) -> Self {
        Self(Arc::new((epoch, handle)))
    }
    pub(crate) fn result(&self) -> Option<bool> {
        self.0.1.peek().copied()
    }
    pub(crate) fn collect(&self) -> impl std::future::Future<Output = bool> + Send + use<> {
        self.0.1.clone()
    }
}
impl PartialEq for ExitCompletion {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ExitCompletion {}
impl Hash for ExitCompletion {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}
pub(crate) fn exit_completed(
    exit: &ExitCompletion,
) -> impl Stream<Item = (u64, bool)> + Send + use<> {
    let epoch = exit.0.0;
    let collect = exit.collect();
    futures::stream::once(async move { (epoch, collect.await) })
}

pub(crate) struct Exit {
    pub(crate) jobs: Snapshot,
    pub(crate) completion: ExitCompletion,
    pub(crate) jj_action: Option<crate::jj_actions::admission::Collector>,
    pub(crate) jj_load: Option<crate::jj_actions::load_admission::Collector>,
    pub(crate) syntax: Option<Handle>,
    pub(crate) readers: Option<Handle>,
    pub(crate) overlay: Option<crate::overlay_manager::OverlayId>,
    pub(crate) epoch: u64,
    pub(crate) window: Option<iced::window::Id>,
    pub(crate) file_epoch: u64,
    pub(crate) root: Option<std::path::PathBuf>,
    pub(crate) documents: Vec<(lume_core::DocId, lume_core::SaveSnapshot)>,
}
pub(crate) fn completions(jobs: &Snapshot) -> impl Stream<Item = (u64, ResultJoin)> + Send + use<> {
    futures::stream::select_all(jobs.0.iter().cloned().map(|job| {
        Box::pin(futures::stream::once(async move {
            (job.id(), job.collect().await)
        }))
    }))
    .fuse()
}

#[derive(Clone)]
pub(crate) struct Delivery {
    event: Arc<Mutex<Option<SearchUpdate>>>,
    ack: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    job: Job,
}
impl std::fmt::Debug for Delivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchDelivery").finish_non_exhaustive()
    }
}
impl Delivery {
    pub(crate) fn is_current(&self) -> bool {
        self.job.is_current()
    }
    pub(crate) fn take(&self) -> Option<SearchUpdate> {
        self.event.lock().ok()?.take()
    }
    #[cfg(test)]
    pub(crate) fn observe(&self) -> Option<SearchUpdate> {
        self.event.lock().ok()?.as_ref().cloned()
    }
    pub(crate) fn acknowledge(&self, accepted: bool) {
        let sender = self.ack.lock().ok().and_then(|mut slot| slot.take());
        if let Some(sender) = sender {
            let _ = sender.send(accepted);
        }
    }
}
#[cfg(test)]
mod tests;

struct DataGuard(Job);
impl Drop for DataGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct DataState {
    guard: DataGuard,
    events: Option<SearchEvents>,
    started: bool,
    previous: Option<oneshot::Receiver<bool>>,
    terminal: bool,
    eof: bool,
}
pub(crate) fn deliveries(job: Job) -> impl Stream<Item = Delivery> + Send {
    futures::stream::unfold(
        DataState {
            guard: DataGuard(job),
            events: None,
            started: false,
            previous: None,
            terminal: false,
            eof: false,
        },
        |mut state| async move {
            let job = &state.guard.0;
            if let Some(ack) = state.previous.take() {
                let accepted =
                    tokio::select! {_=job.canceled()=>false, value=ack=>matches!(value,Ok(true))};
                if !accepted {
                    return None;
                }
            }
            if state.eof || !job.is_current() {
                return None;
            }
            if !state.started {
                state.started = true;
                state.events = job.start().await;
            }
            let event = match &mut state.events {
                Some(events) => events.next().await,
                None => None,
            };
            let event = match event {
                Some(event) => event,
                None if !job.is_current() => return None,
                None if !state.terminal => {
                    state.eof = true;
                    let error = job.collect().await.err().unwrap_or_else(|| {
                        "Lo stream Cerca è terminato senza esito finale: risultati incompleti"
                            .into()
                    });
                    SearchUpdate::Error(error)
                }
                None => return None,
            };
            state.terminal |= matches!(event, SearchUpdate::Done { .. } | SearchUpdate::Error(_));
            let (sender, receiver) = oneshot::channel();
            state.previous = Some(receiver);
            let packet = Delivery {
                event: Arc::new(Mutex::new(Some(event))),
                ack: Arc::new(Mutex::new(Some(sender))),
                job: job.clone(),
            };
            Some((packet, state))
        },
    )
    .fuse()
}
