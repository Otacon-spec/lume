//! Una connessione = un thread. Il modello è quello già in casa di
//! `GlobalFileSearchWorker` (`lume-app/src/search.rs`): mailbox `Mutex` +
//! `Condvar`, quindi **parcheggiato costa zero CPU**, una richiesta nuova
//! sostituisce quella pendente rispondendole `Superseded`, e il `Drop` chiude
//! tutto.
//!
//! Un thread **per connessione** e non per workspace: in modalità confronto i
//! due riquadri devono interrogare A e B davvero in parallelo.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use futures::channel::oneshot;
mod event_channel;
use event_channel::EventSender;
pub use event_channel::{QueryEvents, QueryOrigin};

use crate::driver::{
    ColumnInfo, ConnectionProfile, DriverFactory, Interrupter, QueryBatch, QueryFlow, QuerySink,
    Secret, SqlError,
};

/// Due slot nel canale del worker, non una garanzia sul relay/UI a valle.
/// Il producer si parcheggia solo mentre la consegna attiva è piena; resta
/// interrompibile anche quando il receiver è aperto ma non drena.
pub const QUERY_EVENT_QUEUE_CAPACITY: usize = 2;
/// Primo budget dell'acquisizione progressiva. Il batch in corso può
/// superarlo, in particolare per una singola riga grande.
pub const RESULT_BUDGET_BYTES: usize = 64 * 1024 * 1024;
pub const RESULT_BUDGET_ROWS: usize = 100_000;

/// Eventi del prossimo percorso progressivo. Ogni evento è associato dal
/// chiamante a connessione e generazione; il payload dei batch resta bounded.
#[derive(Clone, PartialEq)]
pub enum QueryEvent {
    Started {
        columns: Vec<ColumnInfo>,
    },
    ResultSetStarted {
        index: usize,
        columns: Vec<ColumnInfo>,
    },
    Batch(QueryBatch),
    BudgetReached {
        rows: usize,
        accounted_bytes: usize,
    },
    Completed {
        elapsed: std::time::Duration,
    },
    Stopped {
        rows: usize,
        accounted_bytes: usize,
    },
    Failed(String),
}

impl std::fmt::Debug for QueryEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Started { columns } => f
                .debug_struct("Started")
                .field("colonne", &columns.len())
                .finish(),
            Self::ResultSetStarted { index, columns } => f
                .debug_struct("ResultSetStarted")
                .field("indice", index)
                .field("colonne", &columns.len())
                .finish(),
            Self::Batch(batch) => f.debug_tuple("Batch").field(batch).finish(),
            Self::BudgetReached {
                rows,
                accounted_bytes,
            }
            | Self::Stopped {
                rows,
                accounted_bytes,
            } => {
                let tipo = if matches!(self, Self::Stopped { .. }) {
                    "Stopped"
                } else {
                    "BudgetReached"
                };
                f.debug_struct(tipo)
                    .field("righe", rows)
                    .field("byte_contabilizzati", accounted_bytes)
                    .finish()
            }
            Self::Completed { elapsed } => f
                .debug_struct("Completed")
                .field("elapsed", elapsed)
                .finish(),
            Self::Failed(message) => f
                .debug_struct("Failed")
                .field("dettagli_byte", &message.len())
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Request {
    Tables,
    Columns(String),
}

/// Il testo resta disponibile al driver, ma non alla diagnostica accidentale.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tables => f.write_str("Tables"),
            Self::Columns(name) => f
                .debug_struct("Columns")
                .field("nome_byte", &name.len())
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum Response {
    Tables(Vec<String>),
    Columns {
        table: String,
        columns: Vec<ColumnInfo>,
    },
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tables(tables) => f
                .debug_struct("Tables")
                .field("numero", &tables.len())
                .finish(),
            Self::Columns { table, columns } => f
                .debug_struct("Columns")
                .field("tabella_byte", &table.len())
                .field("colonne", &columns.len())
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Connecting,
    Connected,
    Failed,
}

impl State {
    const fn as_u8(self) -> u8 {
        match self {
            Self::Connecting => 0,
            Self::Connected => 1,
            Self::Failed => 2,
        }
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Connected,
            2 => Self::Failed,
            _ => Self::Connecting,
        }
    }
}

enum Work {
    Request {
        request: Request,
        sender: oneshot::Sender<Result<Response, SqlError>>,
    },
    Stream {
        sql: String,
        events: EventSender,
    },
}

struct Job {
    work: Work,
    generation: Arc<AtomicU64>,
    my_generation: u64,
}

#[derive(Default)]
struct Mailbox {
    pending: Option<Job>,
}

struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    shutdown: AtomicBool,
    state: AtomicU8,
    /// Ultimo errore leggibile, per il pallino di stato del pannello.
    error: Mutex<Option<String>>,
    /// La connessione è fallita **per l'autenticazione**, non per la rete.
    /// È l'unica distinzione che serve a chi chiama: un segreto rifiutato va
    /// tolto dalla cassaforte, un host irraggiungibile no. Il testo
    /// dell'errore da solo non basta a deciderlo.
    auth_failed: AtomicBool,
    /// Il loop del worker è uscito: serve ai test per osservare il `Drop`
    /// senza attendere il `join`, che è volutamente delegato.
    finished: AtomicBool,
    /// Come interrompere la query in corso, se il driver corrente sa farlo.
    /// Lo pubblica il worker **prima** di segnare `Connected` e lo toglie
    /// quando scarta il driver.
    interrupter: Mutex<Option<Interrupter>>,
    /// Qualcuno ha chiesto di interrompere il job in corso. Il worker lo
    /// azzera a ogni inizio di job e, a fine query, lo legge per decidere se
    /// scartare il driver (lo stato dopo un'Attention è ambiguo).
    cancelled: AtomicBool,
    query_control: Mutex<QueryControl>,
    query_wake: Condvar,
    /// Wake delle future consumer: indipendente da eventi/EOF del Driver.
    event_wake: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct QueryControl {
    generation: Option<u64>,
    command: Option<QueryCommand>,
}

#[derive(Clone, Copy)]
enum QueryCommand {
    Continue,
    Stop,
}

/// Handle di una connessione viva. Droppandolo si chiude il socket: è **il**
/// meccanismo dell'ibernazione, non un dettaglio.
pub struct Connection {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    profile: ConnectionProfile,
}

impl Connection {
    /// Avvia il thread e **ritorna subito**: la connessione vera avviene nel
    /// worker (handshake TLS + auth valgono centinaia di ms su Azure e
    /// Oracle) e lo stato passa da `Connecting` a `Connected`/`Failed`.
    pub fn open(
        profile: ConnectionProfile,
        secret: Secret,
        factory: DriverFactory,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
            state: AtomicU8::new(State::Connecting.as_u8()),
            error: Mutex::new(None),
            auth_failed: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            interrupter: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            query_control: Mutex::new(QueryControl::default()),
            query_wake: Condvar::new(),
            event_wake: Arc::new(tokio::sync::Notify::new()),
        });
        let thread_shared = shared.clone();
        let thread_profile = profile.clone();
        let thread = thread::Builder::new()
            .name(format!("lume-sql-{}", profile.id))
            .spawn(move || {
                worker_loop(&thread_shared, &thread_profile, &secret, factory);
                thread_shared.finished.store(true, Ordering::Release);
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
            profile,
        })
    }

    pub fn profile(&self) -> &ConnectionProfile {
        &self.profile
    }

    pub fn state(&self) -> State {
        State::from_u8(self.shared.state.load(Ordering::Acquire))
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|e| e.clone())
    }

    /// Il tentativo di connessione è stato respinto dall'autenticazione.
    /// Vero solo dopo un fallimento: `false` non vuol dire «autenticato».
    pub fn auth_failed(&self) -> bool {
        self.shared.auth_failed.load(Ordering::Acquire)
    }

    /// Accoda una richiesta. Se ce n'era una pendente non ancora iniziata, le
    /// risponde `Superseded`: chi scrive in fretta non accumula lavoro.
    pub fn submit(
        &self,
        request: Request,
        generation: Arc<AtomicU64>,
        my_generation: u64,
    ) -> Result<oneshot::Receiver<Result<Response, SqlError>>, SqlError> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(SqlError::Unavailable("connessione chiusa".into()));
        }
        let (sender, receiver) = oneshot::channel();
        let job = Job {
            work: Work::Request { request, sender },
            generation,
            my_generation,
        };
        self.enqueue(job)?;
        Ok(receiver)
    }

    /// Accoda una query che consegna batch attraverso una coda di due slot.
    /// Il receiver chiuso è il segnale di fine; gli errori arrivano come
    /// `QueryEvent::Failed` per non richiedere un secondo canale.
    pub fn submit_query_stream(
        &self,
        sql: String,
        generation: Arc<AtomicU64>,
        my_generation: u64,
    ) -> Result<Receiver<QueryEvent>, SqlError> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(SqlError::Unavailable("connessione chiusa".into()));
        }
        let (events, receiver) = sync_channel(QUERY_EVENT_QUEUE_CAPACITY);
        let job = Job {
            work: Work::Stream {
                sql,
                events: events.into(),
            },
            generation,
            my_generation,
        };
        self.enqueue(job)?;
        Ok(receiver)
    }

    /// Stessa richiesta/driver, trasporto async bounded posseduto dalla UI.
    /// Non crea reader thread; non cambia l'API std dei consumer esistenti.
    pub fn submit_query_events(
        &self,
        sql: String,
        generation: Arc<AtomicU64>,
        my_generation: u64,
    ) -> Result<QueryEvents, SqlError> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(SqlError::Unavailable("connessione chiusa".into()));
        }
        let (events, receiver) =
            QueryEvents::channel(self.shared.clone(), generation.clone(), my_generation);
        self.enqueue(Job {
            work: Work::Stream { sql, events },
            generation,
            my_generation,
        })?;
        Ok(receiver)
    }

    fn enqueue(&self, job: Job) -> Result<(), SqlError> {
        let mut mailbox = self
            .shared
            .mailbox
            .lock()
            .map_err(|_| SqlError::Unavailable("mailbox danneggiata".into()))?;
        if let Some(previous) = mailbox.pending.replace(job) {
            reply_error(previous, SqlError::Superseded);
        }
        // Prima rendere visibile il lavoro, poi interrompere la pausa.
        // Viceversa una pausa che si arma fra i due passi perderebbe Stop.
        drop(mailbox);
        self.interrupt_paused_query();
        // Risveglia anche il producer parcheggiato su coda eventi piena,
        // prima che BudgetReached abbia armato il controllo della pausa.
        self.shared.query_wake.notify_all();
        self.shared.event_wake.notify_waiters();
        self.shared.wake.notify_one();
        Ok(())
    }

    pub fn continue_query(&self, generation: u64) -> Result<(), SqlError> {
        self.command_query(generation, QueryCommand::Continue)
    }

    pub fn stop_query(&self, generation: u64) -> Result<(), SqlError> {
        self.command_query(generation, QueryCommand::Stop)
    }

    fn command_query(&self, generation: u64, command: QueryCommand) -> Result<(), SqlError> {
        let mut control = self
            .shared
            .query_control
            .lock()
            .map_err(|_| SqlError::Unavailable("controllo query danneggiato".into()))?;
        if control.generation != Some(generation) {
            return Err(SqlError::Unavailable("nessuna query in pausa".into()));
        }
        if matches!(control.command, Some(QueryCommand::Stop))
            && matches!(command, QueryCommand::Continue)
        {
            return Err(SqlError::Unavailable("la query sta già terminando".into()));
        }
        control.command = Some(command);
        drop(control);
        self.shared.query_wake.notify_all();
        Ok(())
    }

    /// Un nuovo lavoro non resta dietro un cursore in pausa. Il token di
    /// generazione rende il suo esito obsoleto; questo risveglio gli permette
    /// anche di liberare davvero il worker senza polling.
    fn interrupt_paused_query(&self) {
        if let Ok(mut control) = self.shared.query_control.lock()
            && control.generation.is_some()
            && self
                .shared
                .mailbox
                .lock()
                .is_ok_and(|m| m.pending.is_some())
        {
            // Se il worker ha già consumato il job, non interrompere la sua
            // nuova pausa: Stop riguarda solo il cursore davanti al pending.
            control.command = Some(QueryCommand::Stop);
            drop(control);
            self.shared.query_wake.notify_all();
        }
    }

    /// Interrompe la query **in corso** sul server, se il driver sa farlo
    /// (Azure SQL: Attention TDS). Non blocca e non aspetta la conferma: la
    /// query si chiude da sé con `QueryEvent::Failed("… interrotta su
    /// richiesta")` e il worker scarta il driver. Senza `Interrupter`
    /// (Oracle) non fa nulla. Se non c'è nessuna query in corso l'interruzione
    /// si perde, ed è voluto.
    pub fn cancel_running(&self) {
        let interrupter = self
            .shared
            .interrupter
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        let Some(interrupter) = interrupter else {
            return;
        };
        // Prima il segno, poi l'interruzione: quando il worker vede la query
        // fallire il segno c'è già.
        self.shared.cancelled.store(true, Ordering::SeqCst);
        self.shared.query_wake.notify_all();
        interrupter.interrupt();
    }

    /// Il loop del worker è terminato. Solo osservazione, per i test.
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.cancel_running();
        if let Ok(mut mailbox) = self.shared.mailbox.lock()
            && let Some(job) = mailbox.pending.take()
        {
            reply_error(job, SqlError::Unavailable("connessione chiusa".into()));
        }
        self.shared.wake.notify_all();
        self.shared.query_wake.notify_all();
        self.shared.event_wake.notify_waiters();
        // Il `join` va fuori dal thread della UI. Una query Azure in corso è
        // stata appena interrotta (`cancel_running`), quindi il thread esce
        // in fretta; una query Oracle no (`oracledb` non ha un'interruzione a
        // richiesta) e aspettarla qui bloccherebbe l'interfaccia proprio
        // mentre si iberna. Il reaper vive quanto la query residua e poi
        // sparisce.
        if let Some(thread) = self.thread.take()
            && !thread.is_finished()
        {
            let _ = thread::Builder::new()
                .name("lume-sql-reaper".into())
                .spawn(move || {
                    let _ = thread.join();
                });
        }
    }
}

fn worker_loop(
    shared: &Shared,
    profile: &ConnectionProfile,
    secret: &Secret,
    factory: DriverFactory,
) {
    let initial_driver = match factory(profile, secret) {
        Ok(driver) => {
            set_interrupter(shared, driver.interrupter());
            shared
                .state
                .store(State::Connected.as_u8(), Ordering::Release);
            driver
        }
        Err(error) => {
            if let Ok(mut slot) = shared.error.lock() {
                *slot = Some(error.to_string());
            }
            shared
                .auth_failed
                .store(matches!(error, SqlError::Auth(_)), Ordering::Release);
            shared.state.store(State::Failed.as_u8(), Ordering::Release);
            // Non si esce: il thread resta a rispondere `Unavailable` finché
            // l'handle non viene droppato, così lo stato resta osservabile.
            drain_until_shutdown(shared, &error.to_string());
            return;
        }
    };
    let mut driver = Some(initial_driver);

    while !shared.shutdown.load(Ordering::Acquire) {
        let Some(job) = wait_for_job(shared) else {
            return;
        };
        // Un'interruzione arrivata prima di questo job riguardava il job
        // precedente (o nessuno): non deve toccare questo.
        shared.cancelled.store(false, Ordering::SeqCst);
        // Dopo uno stop esplicito non assumiamo che il driver (in particolare
        // TDS) abbia drenato il result stream: la prossima richiesta parte da
        // una connessione nuova invece di ereditare uno stato ambiguo.
        if driver.is_none() {
            match factory(profile, secret) {
                Ok(reconnected) => {
                    set_interrupter(shared, reconnected.interrupter());
                    shared
                        .state
                        .store(State::Connected.as_u8(), Ordering::Release);
                    driver = Some(reconnected);
                }
                Err(error) => {
                    if let Ok(mut slot) = shared.error.lock() {
                        *slot = Some(error.to_string());
                    }
                    shared
                        .auth_failed
                        .store(matches!(error, SqlError::Auth(_)), Ordering::Release);
                    shared.state.store(State::Failed.as_u8(), Ordering::Release);
                    reply_error(job, SqlError::Unavailable(error.to_string()));
                    drain_until_shutdown(shared, &error.to_string());
                    return;
                }
            }
        }
        if job.generation.load(Ordering::Acquire) != job.my_generation {
            reply_error(job, SqlError::Superseded);
            continue;
        }
        let Some(active_driver) = driver.as_mut() else {
            reply_error(
                job,
                SqlError::Unavailable("connessione non ripristinata".into()),
            );
            continue;
        };
        let Job {
            work,
            generation,
            my_generation,
        } = job;
        match work {
            Work::Stream { sql, events } => {
                let result = stream_query(
                    &mut **active_driver,
                    &sql,
                    events,
                    shared,
                    &generation,
                    my_generation,
                );
                if let Err(error) = &result
                    && !matches!(error, SqlError::Superseded)
                    && let Ok(mut slot) = shared.error.lock()
                {
                    *slot = Some(error.to_string());
                }
                let annullata = shared.cancelled.swap(false, Ordering::SeqCst);
                if annullata || result.is_err() || matches!(result, Ok(StreamOutcome::Stopped)) {
                    driver = None;
                    set_interrupter(shared, None);
                }
            }
            Work::Request { request, sender } => {
                let result = match &request {
                    Request::Tables => active_driver.tables().map(Response::Tables),
                    Request::Columns(table) => {
                        active_driver
                            .columns(table)
                            .map(|columns| Response::Columns {
                                table: table.clone(),
                                columns,
                            })
                    }
                };
                let result = if generation.load(Ordering::Acquire) != my_generation {
                    Err(SqlError::Superseded)
                } else {
                    result
                };
                if let Err(error) = &result
                    && !matches!(error, SqlError::Superseded)
                    && let Ok(mut slot) = shared.error.lock()
                {
                    *slot = Some(error.to_string());
                }
                let _ = sender.send(result);
            }
        }
    }
}

/// Connessione fallita: si continua a rispondere finché non arriva il `Drop`.
fn drain_until_shutdown(shared: &Shared, error: &str) {
    while let Some(job) = wait_for_job(shared) {
        reply_error(job, SqlError::Unavailable(error.to_owned()));
    }
}

/// Pubblica (o toglie) il mezzo per interrompere il driver corrente.
fn set_interrupter(shared: &Shared, interrupter: Option<Interrupter>) {
    if let Ok(mut slot) = shared.interrupter.lock() {
        *slot = interrupter;
    }
}

fn reply_error(job: Job, error: SqlError) {
    match job.work {
        Work::Request { sender, .. } => {
            let _ = sender.send(Err(error));
        }
        Work::Stream { events, .. } => {
            let _ = events.try_send(QueryEvent::Failed(error.to_string()));
        }
    }
}
struct EventSink<'a> {
    events: EventSender,
    generation: &'a Arc<AtomicU64>,
    expected_generation: u64,
    rows: usize,
    accounted_bytes: usize,
    budget_reached: bool,
    shared: &'a Shared,
    byte_limit: usize,
    row_limit: usize,
    stopped: bool,
}

impl EventSink<'_> {
    fn send(&mut self, mut event: QueryEvent) -> Result<(), SqlError> {
        loop {
            self.active()?;
            if self.shared.shutdown.load(Ordering::Acquire) {
                return Err(SqlError::Unavailable("connessione chiusa".into()));
            }
            if self.shared.cancelled.load(Ordering::Acquire) {
                return Err(SqlError::Query(crate::driver::QUERY_INTERROTTA.into()));
            }
            match self.events.try_send(event) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => {
                    return Err(SqlError::Unavailable("la vista SQL è stata chiusa".into()));
                }
                Err(TrySendError::Full(unsent)) => event = unsent,
            }
            let control = self
                .shared
                .query_control
                .lock()
                .map_err(|_| SqlError::Unavailable("controllo query danneggiato".into()))?;
            self.active()?;
            if self.shared.shutdown.load(Ordering::Acquire) {
                return Err(SqlError::Unavailable("connessione chiusa".into()));
            }
            if self.shared.cancelled.load(Ordering::Acquire) {
                return Err(SqlError::Query(crate::driver::QUERY_INTERROTTA.into()));
            }
            if control.generation == Some(self.expected_generation)
                && matches!(control.command, Some(QueryCommand::Stop))
            {
                self.stopped = true;
                return Err(SqlError::Unavailable(
                    "query fermata durante la consegna".into(),
                ));
            }
            // Ordine dei lock uguale alla pausa: control → mailbox.
            if self
                .shared
                .mailbox
                .lock()
                .map_err(|_| SqlError::Unavailable("mailbox danneggiata".into()))?
                .pending
                .is_some()
            {
                return Err(SqlError::Superseded);
            }
            // SOLO mentre una query sta consegnando su una coda piena:
            // std Receiver non notifica il drain e l'Arc<AtomicU64> pubblico
            // può cambiare senza notify. Nessun timer da connessione idle;
            // wait_for_job e la pausa budget rimangono Condvar senza polling.
            let _ = self
                .shared
                .query_wake
                .wait_timeout(control, std::time::Duration::from_millis(20))
                .map_err(|_| SqlError::Unavailable("controllo query danneggiato".into()))?;
        }
    }

    /// Esito terminale best-effort: mai tenere il worker/reaper bloccato
    /// dietro batch che la vista ha smesso di leggere. La chiusura senza
    /// terminale non equivale a Completed: il consumer deve dichiararla.
    fn terminal(&self, event: QueryEvent) -> Result<(), SqlError> {
        self.events.try_send(event).map_err(|_| {
            SqlError::Unavailable("esito terminale SQL non consegnato: stream interrotto".into())
        })
    }

    fn active(&self) -> Result<(), SqlError> {
        if self.generation.load(Ordering::Acquire) == self.expected_generation {
            Ok(())
        } else {
            Err(SqlError::Superseded)
        }
    }
}

impl QuerySink for EventSink<'_> {
    fn started(&mut self, columns: Vec<ColumnInfo>) -> Result<QueryFlow, SqlError> {
        self.active()?;
        self.accounted_bytes =
            self.accounted_bytes
                .saturating_add(crate::driver::accounted_columns_bytes(
                    &columns,
                    columns.capacity(),
                ));
        self.send(QueryEvent::Started { columns })?;
        Ok(QueryFlow::Continue)
    }

    fn result_set_started(
        &mut self,
        index: usize,
        columns: Vec<ColumnInfo>,
    ) -> Result<QueryFlow, SqlError> {
        self.active()?;
        self.accounted_bytes =
            self.accounted_bytes
                .saturating_add(crate::driver::accounted_columns_bytes(
                    &columns,
                    columns.capacity(),
                ));
        self.send(QueryEvent::ResultSetStarted { index, columns })?;
        Ok(QueryFlow::Continue)
    }

    fn batch(&mut self, batch: QueryBatch) -> Result<QueryFlow, SqlError> {
        self.active()?;
        self.rows = self.rows.saturating_add(batch.rows.len());
        self.accounted_bytes = self.accounted_bytes.saturating_add(batch.accounted_bytes);
        self.send(QueryEvent::Batch(batch))?;
        if self.rows >= self.row_limit || self.accounted_bytes >= self.byte_limit {
            self.budget_reached = true;
            return self.wait_for_decision();
        }
        Ok(QueryFlow::Continue)
    }
}

impl EventSink<'_> {
    fn wait_for_decision(&mut self) -> Result<QueryFlow, SqlError> {
        let mut control = self
            .shared
            .query_control
            .lock()
            .map_err(|_| SqlError::Unavailable("controllo query danneggiato".into()))?;
        self.active()?;
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(SqlError::Unavailable("connessione chiusa".into()));
        }
        // Il submit può aver preceduto l'armamento. Con enqueue→interrupt
        // e questa osservazione sotto lock copriamo entrambi gli ordini.
        let pending = self
            .shared
            .mailbox
            .lock()
            .map_err(|_| SqlError::Unavailable("mailbox danneggiata".into()))?
            .pending
            .as_ref()
            .map(|_| QueryCommand::Stop);
        control.generation = Some(self.expected_generation);
        control.command = pending;
        drop(control);
        // Pubblicare soltanto DOPO l'armamento. Mai tenere il mutex durante
        // send bounded: chi riceve deve poter inviare Stop/Continue subito.
        let publication = self.send(QueryEvent::BudgetReached {
            rows: self.rows,
            accounted_bytes: self.accounted_bytes,
        });
        let mut control = self
            .shared
            .query_control
            .lock()
            .map_err(|_| SqlError::Unavailable("controllo query danneggiato".into()))?;
        if let Err(error) = publication {
            control.generation = None;
            control.command = None;
            return if self.stopped {
                Ok(QueryFlow::Stop)
            } else {
                Err(error)
            };
        }
        // NON azzerare command qui: può essere arrivato durante send.
        while control.command.is_none()
            && !self.shared.shutdown.load(Ordering::Acquire)
            && !self.events.is_closed()
        {
            control = self
                .shared
                .query_wake
                .wait(control)
                .map_err(|_| SqlError::Unavailable("controllo query danneggiato".into()))?;
        }
        let command = control.command.take();
        control.generation = None;
        match command {
            Some(QueryCommand::Continue) => {
                self.byte_limit = self.byte_limit.saturating_add(RESULT_BUDGET_BYTES);
                self.row_limit = self.row_limit.saturating_add(RESULT_BUDGET_ROWS);
                self.budget_reached = false;
                Ok(QueryFlow::Continue)
            }
            Some(QueryCommand::Stop) => {
                self.stopped = true;
                Ok(QueryFlow::Stop)
            }
            None => Err(SqlError::Unavailable("connessione chiusa".into())),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamOutcome {
    Completed,
    Stopped,
}

fn stream_query(
    driver: &mut dyn crate::driver::Driver,
    sql: &str,
    events: EventSender,
    shared: &Shared,
    generation: &Arc<AtomicU64>,
    expected_generation: u64,
) -> Result<StreamOutcome, SqlError> {
    let mut sink = EventSink {
        events,
        generation,
        expected_generation,
        rows: 0,
        accounted_bytes: 0,
        budget_reached: false,
        shared,
        byte_limit: RESULT_BUDGET_BYTES,
        row_limit: RESULT_BUDGET_ROWS,
        stopped: false,
    };
    match driver.query_stream(sql, &mut sink) {
        Ok(_elapsed) if sink.stopped => {
            sink.terminal(QueryEvent::Stopped {
                rows: sink.rows,
                accounted_bytes: sink.accounted_bytes,
            })?;
            Ok(StreamOutcome::Stopped)
        }
        Ok(_elapsed) if sink.budget_reached => Ok(StreamOutcome::Completed),
        Ok(elapsed) => {
            sink.send(QueryEvent::Completed { elapsed })?;
            Ok(StreamOutcome::Completed)
        }
        Err(error) => {
            let _ = sink.terminal(QueryEvent::Failed(error.to_string()));
            Err(error)
        }
    }
}

fn wait_for_job(shared: &Shared) -> Option<Job> {
    let mut mailbox = shared.mailbox.lock().ok()?;
    while mailbox.pending.is_none() && !shared.shutdown.load(Ordering::Acquire) {
        mailbox = shared.wake.wait(mailbox).ok()?;
    }
    mailbox.pending.take()
}

#[cfg(test)]
mod async_events_tests;
#[cfg(test)]
mod backpressure_tests;
#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod scope_wake_tests;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::time::{Duration, Instant};

    use super::*;
    use crate::driver::{AuthKind, Driver, Engine, QueryFlow, QuerySink, ResultSet, Value};

    /// Doppio di prova: verifica il **ciclo di vita** del worker, non SQL.
    /// I motori veri hanno il loro test d'integrazione dietro variabile
    /// d'ambiente, perché servono credenziali.
    struct FakeDriver;

    struct PausingDriver;
    static PAUSING_FACTORY_CALLS: AtomicU64 = AtomicU64::new(0);

    impl Driver for PausingDriver {
        fn tables(&mut self) -> Result<Vec<String>, SqlError> {
            Ok(Vec::new())
        }
        fn columns(&mut self, _table: &str) -> Result<Vec<ColumnInfo>, SqlError> {
            Ok(Vec::new())
        }
        fn query_stream(
            &mut self,
            _sql: &str,
            sink: &mut dyn QuerySink,
        ) -> Result<Duration, SqlError> {
            sink.started(Vec::new())?;
            let wide = QueryBatch::new(vec![vec![Value::Bytes(vec![0; RESULT_BUDGET_BYTES])]]);
            if sink.batch(wide)? == QueryFlow::Stop {
                return Ok(Duration::ZERO);
            }
            let _ = sink.batch(QueryBatch::new(vec![vec![Value::Int(2)]]))?;
            Ok(Duration::from_millis(2))
        }
    }

    impl Driver for FakeDriver {
        fn tables(&mut self) -> Result<Vec<String>, SqlError> {
            Ok(vec!["users".into(), "orders".into()])
        }

        fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>, SqlError> {
            Ok(vec![ColumnInfo {
                name: format!("{table}_id"),
                type_name: "int".into(),
            }])
        }

        fn query_stream(
            &mut self,
            sql: &str,
            sink: &mut dyn QuerySink,
        ) -> Result<Duration, SqlError> {
            if sql == "multi" {
                sink.started(vec![ColumnInfo {
                    name: "one".into(),
                    type_name: "int".into(),
                }])?;
                sink.batch(QueryBatch::new(vec![vec![Value::Int(1)]]))?;
                sink.result_set_started(
                    1,
                    vec![ColumnInfo {
                        name: "two".into(),
                        type_name: "int".into(),
                    }],
                )?;
                sink.batch(QueryBatch::new(vec![vec![Value::Int(2)]]))?;
                return Ok(Duration::from_millis(2));
            }
            crate::stream_result_set(
                ResultSet {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    elapsed: Duration::from_millis(1),
                },
                sink,
            )
        }
    }

    fn ok_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        Ok(Box::new(FakeDriver))
    }

    fn pausing_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        PAUSING_FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(PausingDriver))
    }

    fn ko_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        Err(SqlError::Connect("host irraggiungibile".into()))
    }

    fn profile() -> ConnectionProfile {
        ConnectionProfile {
            id: "azure-prod".into(),
            label: "azure · prod".into(),
            engine: Engine::AzureSql,
            host: "acme.database.windows.net".into(),
            database: "prod".into(),
            auth: AuthKind::SqlLogin {
                user: "lume".into(),
            },
        }
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    fn open(factory: DriverFactory) -> Connection {
        Connection::open(profile(), Secret::new("segreto".into()), factory).unwrap()
    }

    #[test]
    fn una_richiesta_viene_servita_dal_thread_della_connessione() {
        let connection = open(ok_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = connection
            .submit(Request::Tables, generation, 1)
            .expect("submit");
        let response = futures::executor::block_on(receiver).unwrap().unwrap();
        assert_eq!(
            response,
            Response::Tables(vec!["users".into(), "orders".into()])
        );
        assert_eq!(connection.state(), State::Connected);
    }

    #[test]
    fn il_worker_conserva_i_confini_dei_result_set_multipli() {
        let connection = open(ok_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = connection
            .submit_query_stream("multi".into(), generation, 1)
            .expect("submit");
        let events: Vec<_> = receiver.iter().collect();
        assert!(
            matches!(events.first(), Some(QueryEvent::Started { columns }) if columns.first().is_some_and(|column| column.name == "one"))
        );
        assert!(
            matches!(events.get(1), Some(QueryEvent::Batch(batch)) if batch.rows.first().and_then(|row| row.first()) == Some(&Value::Int(1)))
        );
        assert!(
            matches!(events.get(2), Some(QueryEvent::ResultSetStarted { index: 1, columns }) if columns.first().is_some_and(|column| column.name == "two"))
        );
        assert!(
            matches!(events.get(3), Some(QueryEvent::Batch(batch)) if batch.rows.first().and_then(|row| row.first()) == Some(&Value::Int(2)))
        );
        assert!(matches!(events.get(4), Some(QueryEvent::Completed { .. })));
    }

    #[test]
    fn la_query_progressiva_consegna_eventi_sul_canale_limitato() {
        let connection = open(ok_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = connection
            .submit_query_stream("select 1".into(), generation, 1)
            .expect("submit stream");
        let events: Vec<_> = receiver.into_iter().collect();
        assert!(matches!(events.first(), Some(QueryEvent::Started { .. })));
        assert!(matches!(events.last(), Some(QueryEvent::Completed { .. })));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn il_budget_parcheggia_e_continua_senza_polling() {
        let connection = open(pausing_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = connection
            .submit_query_stream("select large".into(), generation, 1)
            .expect("submit stream");
        assert!(matches!(receiver.recv(), Ok(QueryEvent::Started { .. })));
        assert!(matches!(receiver.recv(), Ok(QueryEvent::Batch(_))));
        assert!(matches!(
            receiver.recv(),
            Ok(QueryEvent::BudgetReached { rows: 1, .. })
        ));
        connection.continue_query(1).expect("continue");
        assert!(matches!(receiver.recv(), Ok(QueryEvent::Batch(_))));
        assert!(matches!(receiver.recv(), Ok(QueryEvent::Completed { .. })));
    }

    #[test]
    fn fermare_invalida_il_driver_prima_della_richiesta_successiva() {
        PAUSING_FACTORY_CALLS.store(0, Ordering::SeqCst);
        let connection = open(pausing_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = connection
            .submit_query_stream("select large".into(), generation.clone(), 1)
            .expect("submit stream");
        let _ = receiver.recv();
        let _ = receiver.recv();
        assert!(matches!(
            receiver.recv(),
            Ok(QueryEvent::BudgetReached { .. })
        ));
        connection.stop_query(1).expect("stop");
        assert!(matches!(receiver.recv(), Ok(QueryEvent::Stopped { .. })));
        generation.store(2, Ordering::Release);
        let tables = connection
            .submit(Request::Tables, generation, 2)
            .expect("submit tables");
        assert!(futures::executor::block_on(tables).is_ok());
        assert!(PAUSING_FACTORY_CALLS.load(Ordering::Acquire) >= 2);
    }

    #[test]
    fn chiudere_la_connessione_termina_il_thread() {
        let connection = open(ok_factory);
        assert!(wait_until(|| connection.state() == State::Connected));
        let shared = connection.shared.clone();
        drop(connection);
        assert!(
            wait_until(|| shared.finished.load(Ordering::Acquire)),
            "il thread deve uscire alla chiusura: e' tutto il senso dell'ibernazione"
        );
    }

    #[test]
    fn una_richiesta_nuova_supera_quella_pendente() {
        let connection = open(ok_factory);
        let generation = Arc::new(AtomicU64::new(7));
        // due submit di fila: la prima puo' essere ancora in mailbox
        let first = connection
            .submit(Request::Columns("users".into()), generation.clone(), 6)
            .expect("submit");
        let second = connection
            .submit(Request::Tables, generation, 7)
            .expect("submit");
        // la generazione 6 e' superata dalla 7 in ogni caso: o l'ha scartata
        // la mailbox, o l'ha scartata il worker prima di eseguirla
        assert!(matches!(
            futures::executor::block_on(first).unwrap(),
            Err(SqlError::Superseded)
        ));
        assert!(futures::executor::block_on(second).unwrap().is_ok());
    }

    #[test]
    fn una_connessione_fallita_resta_osservabile_e_risponde() {
        let connection = open(ko_factory);
        assert!(wait_until(|| connection.state() == State::Failed));
        assert_eq!(
            connection.error().as_deref(),
            Some("connessione fallita: host irraggiungibile")
        );
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = connection
            .submit(Request::Tables, generation, 1)
            .expect("submit");
        assert!(matches!(
            futures::executor::block_on(receiver).unwrap(),
            Err(SqlError::Unavailable(_))
        ));
    }

    /// Doppio di **ciclo di vita** (come `FakeDriver`, non finge un database):
    /// una query che dura finché qualcuno non la interrompe, o al massimo 30 s.
    /// Il suo `Interrupter` si comporta come quello di Azure: fuori da una
    /// query non fa nulla.
    struct Gate {
        started: AtomicU64,
        interrupted: AtomicU64,
        factory_calls: AtomicU64,
        in_corso: AtomicBool,
        /// Se vero la query finisce subito da sola (per provare l'annullamento
        /// «a vuoto» senza aspettare).
        natural: AtomicBool,
        /// Se vero l'interruzione arriva ma il doppio la riporta come Azure
        /// quando il server non conferma l'Attention.
        non_confermata: AtomicBool,
        flag: Mutex<bool>,
        wake: Condvar,
    }

    impl Gate {
        const fn new() -> Self {
            Self {
                started: AtomicU64::new(0),
                interrupted: AtomicU64::new(0),
                factory_calls: AtomicU64::new(0),
                in_corso: AtomicBool::new(false),
                natural: AtomicBool::new(false),
                non_confermata: AtomicBool::new(false),
                flag: Mutex::new(false),
                wake: Condvar::new(),
            }
        }

        fn interrupt(&self) {
            if !self.in_corso.load(Ordering::SeqCst) {
                return;
            }
            self.interrupted.fetch_add(1, Ordering::SeqCst);
            *self.flag.lock().unwrap() = true;
            self.wake.notify_all();
        }

        /// Vero se è arrivata un'interruzione, falso se sono passati 30 s.
        fn wait(&self) -> bool {
            let guard = self.flag.lock().unwrap();
            let (mut guard, esito) = self
                .wake
                .wait_timeout_while(guard, Duration::from_secs(30), |interrotto| !*interrotto)
                .unwrap();
            *guard = false;
            !esito.timed_out()
        }
    }

    struct BlockingDriver(&'static Gate);

    impl Driver for BlockingDriver {
        fn tables(&mut self) -> Result<Vec<String>, SqlError> {
            Ok(vec!["t".into()])
        }
        fn columns(&mut self, _table: &str) -> Result<Vec<ColumnInfo>, SqlError> {
            Ok(Vec::new())
        }
        fn query_stream(
            &mut self,
            _sql: &str,
            sink: &mut dyn QuerySink,
        ) -> Result<Duration, SqlError> {
            let gate = self.0;
            gate.in_corso.store(true, Ordering::SeqCst);
            sink.started(Vec::new())?;
            gate.started.fetch_add(1, Ordering::SeqCst);
            let esito = if gate.natural.load(Ordering::SeqCst) {
                Ok(Duration::ZERO)
            } else if gate.wait() {
                if gate.non_confermata.load(Ordering::SeqCst) {
                    Err(SqlError::Query(format!(
                        "{}; il server non ha confermato entro 5 s",
                        crate::driver::QUERY_INTERROTTA
                    )))
                } else {
                    Err(SqlError::Query(crate::driver::QUERY_INTERROTTA.into()))
                }
            } else {
                Err(SqlError::Query("non interrotta entro 30 s".into()))
            };
            gate.in_corso.store(false, Ordering::SeqCst);
            esito
        }
        fn interrupter(&self) -> Option<crate::driver::Interrupter> {
            let gate = self.0;
            Some(crate::driver::Interrupter::new(move || gate.interrupt()))
        }
    }

    static GATE_CANCEL: Gate = Gate::new();
    static GATE_DROP: Gate = Gate::new();
    static GATE_IDLE: Gate = Gate::new();
    static GATE_NOCONF: Gate = Gate::new();
    static GATE_DOPPIO: Gate = Gate::new();
    static OK_CONTATI: AtomicU64 = AtomicU64::new(0);

    fn gate_cancel_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        GATE_CANCEL.factory_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(BlockingDriver(&GATE_CANCEL)))
    }

    fn gate_drop_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        GATE_DROP.factory_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(BlockingDriver(&GATE_DROP)))
    }

    fn gate_idle_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        GATE_IDLE.factory_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(BlockingDriver(&GATE_IDLE)))
    }

    fn gate_noconf_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        GATE_NOCONF.factory_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(BlockingDriver(&GATE_NOCONF)))
    }

    fn gate_doppio_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        GATE_DOPPIO.factory_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(BlockingDriver(&GATE_DOPPIO)))
    }

    fn ok_contata_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        OK_CONTATI.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeDriver))
    }

    /// Avvia una query sul doppio e aspetta che sia davvero in corso.
    fn avvia_query_bloccata(
        connection: &Connection,
        gate: &'static Gate,
        generation: &Arc<AtomicU64>,
    ) -> Receiver<QueryEvent> {
        let receiver = connection
            .submit_query_stream("select lento".into(), generation.clone(), 1)
            .expect("submit stream");
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(5)),
            Ok(QueryEvent::Started { .. })
        ));
        assert!(wait_until(|| gate.started.load(Ordering::SeqCst) == 1));
        receiver
    }

    #[test]
    fn cancel_running_interrompe_la_query_in_corso_e_il_driver_si_ricrea() {
        let connection = open(gate_cancel_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = avvia_query_bloccata(&connection, &GATE_CANCEL, &generation);

        connection.cancel_running();

        match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(QueryEvent::Failed(testo)) => {
                assert!(testo.contains(crate::driver::QUERY_INTERROTTA), "{testo}");
            }
            altro => panic!("doveva fallire con «interrotta su richiesta»: {altro:?}"),
        }
        assert_eq!(GATE_CANCEL.interrupted.load(Ordering::SeqCst), 1);

        // Dopo un'interruzione il driver si scarta: la richiesta dopo passa
        // dalla factory (stato TDS ambiguo) e funziona.
        generation.store(2, Ordering::Release);
        let tables = connection
            .submit(Request::Tables, generation, 2)
            .expect("submit tables");
        assert!(futures::executor::block_on(tables).unwrap().is_ok());
        assert_eq!(
            GATE_CANCEL.factory_calls.load(Ordering::SeqCst),
            2,
            "il driver interrotto va ricreato, non riusato"
        );
    }

    /// Il caso che oggi blocca: chiudere il progetto con una query in corso.
    /// Senza l'interruzione il thread (e il reaper) restano vivi fino ai 30 s
    /// del doppio; con l'interruzione escono subito.
    #[test]
    fn chiudere_la_connessione_interrompe_la_query_invece_di_aspettarla() {
        let connection = open(gate_drop_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let _receiver = avvia_query_bloccata(&connection, &GATE_DROP, &generation);
        let shared = connection.shared.clone();

        drop(connection);

        assert!(
            wait_until(|| shared.finished.load(Ordering::Acquire)),
            "il thread doveva uscire subito: la query è stata interrotta"
        );
        assert_eq!(GATE_DROP.interrupted.load(Ordering::SeqCst), 1);
    }

    /// Oracle (e ogni driver senza `Interrupter`): `cancel_running` non fa
    /// nulla, nemmeno segnare `cancelled`, altrimenti a ogni «Scarta» su una
    /// query già finita la connessione verrebbe buttata e rifatta.
    #[test]
    fn cancel_running_senza_interrupter_non_fa_nulla() {
        let connection = open(ok_contata_factory);
        assert!(wait_until(|| connection.state() == State::Connected));

        connection.cancel_running();

        assert!(!connection.shared.cancelled.load(Ordering::SeqCst));
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = connection
            .submit_query_stream("select 1".into(), generation.clone(), 1)
            .expect("submit stream");
        let eventi: Vec<_> = receiver.into_iter().collect();
        assert!(matches!(eventi.last(), Some(QueryEvent::Completed { .. })));
        generation.store(2, Ordering::Release);
        let tables = connection
            .submit(Request::Tables, generation, 2)
            .expect("submit tables");
        assert!(futures::executor::block_on(tables).unwrap().is_ok());
        assert_eq!(
            OK_CONTATI.load(Ordering::SeqCst),
            1,
            "nessuna riconnessione"
        );
    }

    /// Un'interruzione arrivata a query finita (l'utente preme «Scarta» un
    /// istante dopo) non deve costare il driver alla query successiva.
    #[test]
    fn un_annullamento_a_vuoto_non_tocca_la_query_dopo() {
        GATE_IDLE.natural.store(true, Ordering::SeqCst);
        let connection = open(gate_idle_factory);
        assert!(wait_until(|| connection.state() == State::Connected));

        connection.cancel_running();
        assert!(
            connection.shared.cancelled.load(Ordering::SeqCst),
            "l'interrupter c'è: il segno deve essere armato"
        );
        assert_eq!(GATE_IDLE.interrupted.load(Ordering::SeqCst), 0);

        let generation = Arc::new(AtomicU64::new(1));
        let receiver = connection
            .submit_query_stream("select 1".into(), generation.clone(), 1)
            .expect("submit stream");
        let eventi: Vec<_> = receiver.into_iter().collect();
        assert!(matches!(eventi.last(), Some(QueryEvent::Completed { .. })));

        generation.store(2, Ordering::Release);
        let tables = connection
            .submit(Request::Tables, generation, 2)
            .expect("submit tables");
        assert!(futures::executor::block_on(tables).unwrap().is_ok());
        assert_eq!(
            GATE_IDLE.factory_calls.load(Ordering::SeqCst),
            1,
            "il segno vecchio andava azzerato all'inizio del job"
        );
    }

    /// Il driver Azure, se il server non conferma l'Attention, chiude la
    /// query con un errore «… non ha confermato». Lo stato TDS è ancora più
    /// ambiguo: il driver non deve restare in uso.
    #[test]
    fn se_il_server_non_conferma_l_interruzione_il_driver_si_ricrea() {
        GATE_NOCONF.non_confermata.store(true, Ordering::SeqCst);
        let connection = open(gate_noconf_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = avvia_query_bloccata(&connection, &GATE_NOCONF, &generation);

        connection.cancel_running();

        match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(QueryEvent::Failed(testo)) => {
                assert!(testo.contains("non ha confermato"), "{testo}");
            }
            altro => panic!("doveva fallire «non ha confermato»: {altro:?}"),
        }

        generation.store(2, Ordering::Release);
        let tables = connection
            .submit(Request::Tables, generation, 2)
            .expect("submit tables");
        assert!(futures::executor::block_on(tables).unwrap().is_ok());
        assert_eq!(
            GATE_NOCONF.factory_calls.load(Ordering::SeqCst),
            2,
            "dopo un'interruzione non confermata il driver va ricreato"
        );
    }

    /// Idempotenza: un secondo `cancel_running` mentre la query si sta già
    /// chiudendo, o dopo che si è chiusa, non deve far scartare anche il
    /// driver nuovo né far fallire la richiesta successiva.
    #[test]
    fn un_doppio_annullamento_costa_una_sola_riconnessione() {
        let connection = open(gate_doppio_factory);
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = avvia_query_bloccata(&connection, &GATE_DOPPIO, &generation);

        connection.cancel_running();
        connection.cancel_running();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(5)),
            Ok(QueryEvent::Failed(_))
        ));
        // Tre interruzioni a query chiusa: si perdono.
        connection.cancel_running();
        connection.cancel_running();

        generation.store(2, Ordering::Release);
        let tables = connection
            .submit(Request::Tables, generation.clone(), 2)
            .expect("submit tables");
        assert!(futures::executor::block_on(tables).unwrap().is_ok());
        generation.store(3, Ordering::Release);
        let tables = connection
            .submit(Request::Tables, generation, 3)
            .expect("submit tables");
        assert!(futures::executor::block_on(tables).unwrap().is_ok());
        assert_eq!(
            GATE_DOPPIO.factory_calls.load(Ordering::SeqCst),
            2,
            "una sola riconnessione, non una per ogni richiesta"
        );
    }
}
