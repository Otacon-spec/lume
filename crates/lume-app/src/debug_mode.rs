//! Il debugger nell'app (piano 17, design `debugger-dap.md` §2).
//!
//! - [`DebugSpawn`] è la chiave della subscription che **possiede** la
//!   sessione: finché l'app la produce, gdb vive; smesso di produrla, lo
//!   stream muore e con lui l'adattatore (`kill_on_drop`).
//! - [`run`] è l'attore dentro lo stream: avvia la sessione, inoltra gli
//!   eventi, esegue i comandi. A ogni arresto chiede da sé stack, scope,
//!   variabili e le righe di sorgente del frame scelto.
//! - [`DebugController`] è lo stato lato app: fase, storia degli arresti
//!   (la cascata Execution Path, D-DAP-10), console, valutazioni,
//!   punti di interruzione.
//!
//! Nessun timer, nessun polling: tutto parte da un gesto o da un evento
//! dell'adattatore.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lume_dap::{
    AdapterEvent, Breakpoint, BreakpointEvent, Capabilities, DebugSession, Event, FileBreakpoints,
    LaunchConfig, LaunchError, OutputEvent, Scope, ShutdownOutcome, SourceBreakpoint, StackFrame,
    StopHistory, StoppedEvent, Variable,
};
use tokio::sync::mpsc;

/// Righe e byte della console (design §6).
pub const CONSOLE_MAX_LINES: usize = 5000;
pub const CONSOLE_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const CONSOLE_MAX_LINE_BYTES: usize = 64 * 1024;
/// Valutazioni conservate.
pub const MAX_EVALUATIONS: usize = 50;
/// Righe di sorgente mostrate sopra e sotto la riga del frame.
pub const SNIPPET_RADIUS: i64 = 6;
/// Oltre questa dimensione il sorgente non si legge per l'anteprima.
pub const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
pub const COMMAND_CAPACITY: usize = 32;

// ── Lo stream ───────────────────────────────────────────────────────

/// Chiave della subscription: «quale sessione», non il suo contenuto.
#[derive(Debug, Clone)]
pub struct DebugSpawn {
    pub epoch: u64,
    pub adapter: PathBuf,
    pub config: Arc<LaunchConfig>,
    pub breakpoints: Arc<Vec<FileBreakpoints>>,
}

impl Hash for DebugSpawn {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "debug".hash(state);
        self.epoch.hash(state);
    }
}

#[derive(Debug, Clone)]
pub enum DebugCommand {
    Continue,
    Pause,
    Next,
    StepIn,
    StepOut,
    /// Dettaglio di un frame: sorgente sempre, scope e variabili solo se
    /// `frame_id` è dell'arresto corrente.
    Frame {
        frame_id: Option<i64>,
        path: Option<PathBuf>,
        line: i64,
    },
    Children(i64),
    Evaluate {
        expression: String,
        frame_id: Option<i64>,
    },
    SetBreakpoints(FileBreakpoints),
    Stop,
}

/// Le righe di sorgente attorno a un frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub path: PathBuf,
    pub first_line: i64,
    pub lines: Vec<String>,
    pub line: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeDetail {
    pub scope: Scope,
    pub variables: Vec<Variable>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub enum DebugEvent {
    Ready(mpsc::Sender<DebugCommand>),
    Started {
        capabilities: Capabilities,
        breakpoints: Vec<(PathBuf, Vec<Breakpoint>)>,
    },
    StartFailed(String),
    Stopped {
        stopped: StoppedEvent,
        thread_id: i64,
        frames: Vec<StackFrame>,
        /// Righe di codice di ogni frame, per il flusso del codice.
        code: Vec<Vec<(i64, String)>>,
    },
    Continued,
    FrameDetail {
        frame_id: Option<i64>,
        scopes: Vec<ScopeDetail>,
        snippet: Option<Snippet>,
        error: Option<String>,
    },
    Children {
        reference: i64,
        variables: Vec<Variable>,
        truncated: bool,
    },
    Evaluated {
        expression: String,
        result: Result<String, String>,
    },
    BreakpointsSet {
        path: PathBuf,
        breakpoints: Vec<Breakpoint>,
    },
    BreakpointChanged(Breakpoint),
    Output {
        category: Option<String>,
        text: String,
    },
    Exited(i64),
    CommandFailed(String),
    /// Ultimo evento: l'adattatore è stato raccolto.
    Closed(String),
}

/// Legge le righe attorno a `line` (I/O asincrono, mai sul thread della UI).
pub async fn read_snippet(path: &Path, line: i64) -> Option<Snippet> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let first_line = (line - SNIPPET_RADIUS).max(1);
    let skip = usize::try_from(first_line - 1).ok()?;
    let take = usize::try_from(SNIPPET_RADIUS * 2 + 1).ok()?;
    let lines: Vec<String> = text
        .lines()
        .skip(skip)
        .take(take)
        .map(|riga| riga.replace('\t', "    "))
        .collect();
    (!lines.is_empty()).then(|| Snippet {
        path: path.to_path_buf(),
        first_line,
        lines,
        line,
    })
}

/// Righe di contorno attorno alla riga eseguita, nel flusso del codice.
pub const FLOW_CONTEXT: i64 = 1;

/// Le righe di codice di ogni frame, leggendo ogni file una volta sola.
async fn flow_code(frames: &[StackFrame]) -> Vec<Vec<(i64, String)>> {
    let mut letti: HashMap<PathBuf, Option<Vec<String>>> = HashMap::new();
    let mut byte_letti: u64 = 0;
    let mut code = Vec::with_capacity(frames.len());
    for frame in frames {
        let Some(path) = frame
            .source
            .as_ref()
            .and_then(|source| source.path.as_ref())
            .map(PathBuf::from)
        else {
            code.push(Vec::new());
            continue;
        };
        if !letti.contains_key(&path) {
            let righe = match tokio::fs::metadata(&path).await {
                Ok(meta) if meta.is_file() && byte_letti + meta.len() <= MAX_SOURCE_BYTES => {
                    byte_letti += meta.len();
                    tokio::fs::read(&path).await.ok().map(|bytes| {
                        String::from_utf8_lossy(&bytes)
                            .lines()
                            .map(|riga| riga.replace('\t', "    "))
                            .collect()
                    })
                }
                _ => None,
            };
            letti.insert(path.clone(), righe);
        }
        let righe = letti.get(&path).and_then(Option::as_ref);
        let prima = (frame.line - FLOW_CONTEXT).max(1);
        let ultima = frame.line + FLOW_CONTEXT;
        code.push(
            righe
                .map(|righe| {
                    (prima..=ultima)
                        .filter_map(|numero| {
                            let indice = usize::try_from(numero - 1).ok()?;
                            righe.get(indice).map(|testo| (numero, testo.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        );
    }
    code
}

fn describe_outcome(outcome: &ShutdownOutcome) -> String {
    match outcome {
        ShutdownOutcome::Exited(_) => "adattatore chiuso".to_owned(),
        ShutdownOutcome::Killed(_) => "adattatore terminato dopo la scadenza".to_owned(),
        ShutdownOutcome::Failed(error) => format!("chiusura dell'adattatore incompleta: {error}"),
    }
}

struct Actor {
    session: DebugSession,
    events: mpsc::Sender<DebugEvent>,
    thread_id: Option<i64>,
}

impl Actor {
    async fn send(&self, event: DebugEvent) -> bool {
        self.events.send(event).await.is_ok()
    }

    async fn frame_detail(
        &mut self,
        frame_id: Option<i64>,
        path: Option<PathBuf>,
        line: i64,
    ) -> DebugEvent {
        let snippet = match path {
            Some(path) => read_snippet(&path, line).await,
            None => None,
        };
        let mut scopes = Vec::new();
        let mut error = None;
        if let Some(frame_id) = frame_id {
            match self.session.scopes(frame_id).await {
                Ok(elenco) => {
                    for scope in elenco {
                        if scope.expensive || scope.variables_reference <= 0 {
                            scopes.push(ScopeDetail {
                                scope,
                                variables: Vec::new(),
                                truncated: false,
                            });
                            continue;
                        }
                        match self.session.variables(scope.variables_reference).await {
                            Ok((variables, truncated)) => scopes.push(ScopeDetail {
                                scope,
                                variables,
                                truncated,
                            }),
                            Err(errore) => error = Some(errore.to_string()),
                        }
                    }
                }
                Err(errore) => error = Some(errore.to_string()),
            }
        }
        DebugEvent::FrameDetail {
            frame_id,
            scopes,
            snippet,
            error,
        }
    }

    /// Gestisce un evento dell'adattatore; `false` se la sessione è finita.
    async fn on_adapter_event(&mut self, event: Event) -> bool {
        match event.event.as_str() {
            "stopped" => {
                let Ok(stopped) = serde_json::from_value::<StoppedEvent>(event.body) else {
                    return self
                        .send(DebugEvent::CommandFailed(
                            "evento stopped non valido".into(),
                        ))
                        .await;
                };
                let thread_id = match stopped.thread_id.or(self.thread_id) {
                    Some(thread) => thread,
                    None => match self.session.threads().await {
                        Ok(threads) => threads.first().map_or(1, |thread| thread.id),
                        Err(_) => 1,
                    },
                };
                self.thread_id = Some(thread_id);
                match self.session.stack_trace(thread_id).await {
                    Ok(frames) => {
                        let top = frames.first().map(|frame| {
                            (
                                frame.id,
                                frame
                                    .source
                                    .as_ref()
                                    .and_then(|source| source.path.as_ref())
                                    .map(PathBuf::from),
                                frame.line,
                            )
                        });
                        let code = flow_code(&frames).await;
                        if !self
                            .send(DebugEvent::Stopped {
                                stopped,
                                thread_id,
                                frames,
                                code,
                            })
                            .await
                        {
                            return false;
                        }
                        if let Some((frame_id, path, line)) = top {
                            let detail = self.frame_detail(Some(frame_id), path, line).await;
                            return self.send(detail).await;
                        }
                        true
                    }
                    Err(error) => {
                        self.send(DebugEvent::CommandFailed(format!("stack: {error}")))
                            .await
                    }
                }
            }
            "continued" => self.send(DebugEvent::Continued).await,
            "output" => match serde_json::from_value::<OutputEvent>(event.body) {
                Ok(output) => {
                    self.send(DebugEvent::Output {
                        category: output.category,
                        text: output.output,
                    })
                    .await
                }
                Err(_) => true,
            },
            "breakpoint" => match serde_json::from_value::<BreakpointEvent>(event.body) {
                Ok(changed) => {
                    self.send(DebugEvent::BreakpointChanged(changed.breakpoint))
                        .await
                }
                Err(_) => true,
            },
            "exited" => {
                let code = event
                    .body
                    .get("exitCode")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(-1);
                self.send(DebugEvent::Exited(code)).await
            }
            "terminated" => false,
            _ => true,
        }
    }

    async fn on_command(&mut self, command: DebugCommand) -> bool {
        let thread = self.thread_id.unwrap_or(1);
        let result = match command {
            DebugCommand::Stop => return false,
            DebugCommand::Continue => self.session.continue_(thread).await,
            DebugCommand::Pause => self.session.pause(thread).await,
            DebugCommand::Next => self.session.next(thread).await,
            DebugCommand::StepIn => self.session.step_in(thread).await,
            DebugCommand::StepOut => self.session.step_out(thread).await,
            DebugCommand::Frame {
                frame_id,
                path,
                line,
            } => {
                let detail = self.frame_detail(frame_id, path, line).await;
                return self.send(detail).await;
            }
            DebugCommand::Children(reference) => match self.session.variables(reference).await {
                Ok((variables, truncated)) => {
                    return self
                        .send(DebugEvent::Children {
                            reference,
                            variables,
                            truncated,
                        })
                        .await;
                }
                Err(error) => Err(error),
            },
            DebugCommand::Evaluate {
                expression,
                frame_id,
            } => {
                let result = self
                    .session
                    .evaluate(&expression, frame_id)
                    .await
                    .map(|valutato| valutato.result)
                    .map_err(|error| error.to_string());
                return self
                    .send(DebugEvent::Evaluated { expression, result })
                    .await;
            }
            DebugCommand::SetBreakpoints(file) => match self.session.set_breakpoints(&file).await {
                Ok(breakpoints) => {
                    return self
                        .send(DebugEvent::BreakpointsSet {
                            path: file.path,
                            breakpoints,
                        })
                        .await;
                }
                Err(error) => Err(error),
            },
        };
        match result {
            Ok(()) => true,
            Err(error) => {
                self.send(DebugEvent::CommandFailed(error.to_string()))
                    .await
            }
        }
    }
}

/// L'attore della sessione. Termina solo dopo aver raccolto l'adattatore e
/// consegnato [`DebugEvent::Closed`].
pub async fn run(
    spawn: DebugSpawn,
    mut commands: mpsc::Receiver<DebugCommand>,
    events: mpsc::Sender<DebugEvent>,
) {
    let started =
        DebugSession::start(&spawn.adapter, &spawn.config, spawn.breakpoints.as_slice()).await;
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            let _ = events.send(DebugEvent::StartFailed(error.message)).await;
            let _ = events
                .send(DebugEvent::Closed("avvio fallito".to_owned()))
                .await;
            return;
        }
    };
    let mut adapter_events = started.events;
    let mut actor = Actor {
        session: started.session,
        events,
        thread_id: None,
    };
    // Eventi arrivati mentre si attendeva una risposta.
    let mut backlog: VecDeque<AdapterEvent> = VecDeque::new();
    let mut running = actor
        .send(DebugEvent::Started {
            capabilities: actor.session.capabilities().clone(),
            breakpoints: started.breakpoints,
        })
        .await;
    while running {
        if let Some(pending) = backlog.pop_front() {
            running = match pending {
                AdapterEvent::Event(event) => actor.on_adapter_event(event).await,
                AdapterEvent::Closed(_) => false,
            };
            continue;
        }
        tokio::select! {
            event = adapter_events.recv() => {
                running = match event {
                    Some(AdapterEvent::Event(event)) => {
                        let lavoro = actor.on_adapter_event(event);
                        drain_while(lavoro, &mut adapter_events, &mut backlog).await
                    }
                    Some(AdapterEvent::Closed(_)) | None => false,
                };
            }
            command = commands.recv() => {
                running = match command {
                    Some(command) => {
                        let lavoro = actor.on_command(command);
                        drain_while(lavoro, &mut adapter_events, &mut backlog).await
                    }
                    None => false,
                };
            }
        }
    }
    let outcome = actor.session.close(true).await;
    let _ = actor
        .events
        .send(DebugEvent::Closed(describe_outcome(&outcome)))
        .await;
}

/// Esegue `work` continuando a ricevere gli eventi dell'adattatore: senza
/// questo, un programma che stampa molto riempirebbe la coda del client
/// mentre si attende una risposta, e la risposta non arriverebbe più.
async fn drain_while<F>(
    work: F,
    adapter_events: &mut mpsc::Receiver<AdapterEvent>,
    backlog: &mut VecDeque<AdapterEvent>,
) -> bool
where
    F: std::future::Future<Output = bool>,
{
    tokio::pin!(work);
    loop {
        tokio::select! {
            biased;
            esito = &mut work => return esito,
            event = adapter_events.recv(), if backlog.len() < lume_dap::client::EVENT_CAPACITY => {
                match event {
                    Some(event) => backlog.push_back(event),
                    None => backlog.push_back(AdapterEvent::Closed("eventi finiti".into())),
                }
            }
        }
    }
}

// ── Lo stato lato app ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nessuna sessione.
    Ferma,
    /// Avvio dell'adattatore e configurazione.
    Avvio,
    InEsecuzione,
    Fermo,
    /// Chiusura in corso: l'adattatore non è ancora stato raccolto.
    Arresto,
}

impl Phase {
    pub const fn is_live(self) -> bool {
        !matches!(self, Self::Ferma)
    }
}

/// Stato di un punto di interruzione.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointState {
    /// `None` finché l'adattatore non ha risposto (marker non verificato).
    pub verified: Option<bool>,
    pub message: Option<String>,
    pub id: Option<i64>,
}

#[derive(Debug, Default)]
pub struct Console {
    lines: VecDeque<(Option<String>, String)>,
    bytes: usize,
    partial: String,
}

impl Console {
    pub fn push(&mut self, category: Option<String>, text: &str) {
        self.partial.push_str(text);
        while let Some(fine) = self.partial.find('\n') {
            let riga: String = self.partial.drain(..=fine).collect();
            self.push_line(
                category.clone(),
                riga.trim_end_matches(['\n', '\r']).to_owned(),
            );
        }
        if self.partial.len() > CONSOLE_MAX_LINE_BYTES {
            let riga = std::mem::take(&mut self.partial);
            self.push_line(category, riga);
        }
    }

    fn push_line(&mut self, category: Option<String>, mut line: String) {
        if line.len() > CONSOLE_MAX_LINE_BYTES {
            let mut taglio = CONSOLE_MAX_LINE_BYTES;
            while !line.is_char_boundary(taglio) {
                taglio -= 1;
            }
            line.truncate(taglio);
            line.push('…');
        }
        self.bytes += line.len();
        self.lines.push_back((category, line));
        while self.lines.len() > CONSOLE_MAX_LINES || self.bytes > CONSOLE_MAX_BYTES {
            match self.lines.pop_front() {
                Some((_, scartata)) => self.bytes -= scartata.len(),
                None => break,
            }
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.bytes = 0;
        self.partial.clear();
    }

    pub fn lines(&self) -> impl Iterator<Item = &(Option<String>, String)> {
        self.lines.iter()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }
}

/// Le configurazioni lette da `.lume/launch.toml`, l'adattatore trovato e,
/// per il widget «Esegui», gli strumenti su `PATH` e il progetto Maven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub configs: Result<Vec<LaunchConfig>, LaunchError>,
    pub adapter: Option<PathBuf>,
    pub strumenti: crate::esecuzione::Strumenti,
    pub maven: Option<crate::esecuzione::Maven>,
    /// Tutte le configurazioni del file, come scritte (finestra e widget).
    pub configurazioni: Vec<crate::configurazioni::Configurazione>,
    /// Il `pom.xml` della radice, per il pannello Maven.
    pub pom: Option<Result<crate::esecuzione::Pom, String>>,
}

impl Setup {
    pub fn vuoto(configs: Result<Vec<LaunchConfig>, LaunchError>) -> Self {
        Self {
            configs,
            configurazioni: Vec::new(),
            pom: None,
            adapter: None,
            strumenti: crate::esecuzione::Strumenti::default(),
            maven: None,
        }
    }
}

/// Legge configurazioni, cerca gdb, gli strumenti e Maven. I/O bloccante
/// (solo `stat` e due letture piccole): va chiamata con `spawn_blocking`.
pub fn load_setup(workspace: &Path) -> Setup {
    let strumenti = crate::esecuzione::Strumenti::cerca();
    let lette = crate::configurazioni::leggi(workspace);
    let esiste = workspace.join(lume_dap::LAUNCH_FILE).is_file();
    let configs = lette.clone().and_then(|configurazioni| {
        let lanci = crate::configurazioni::lanci(&configurazioni, workspace)?;
        match (lanci.is_empty(), esiste) {
            (true, false) => Err(LaunchError::Missing),
            (true, true) => Err(LaunchError::Empty),
            (false, _) => Ok(lanci),
        }
    });
    Setup {
        configs,
        configurazioni: lette.unwrap_or_default(),
        pom: crate::esecuzione::carica_pom(workspace),
        adapter: strumenti.gdb.clone(),
        maven: crate::esecuzione::cerca_maven(workspace, &strumenti),
        strumenti,
    }
}

/// I segni del debugger per un file, pronti per l'overlay dell'editor.
/// Righe a base 0, ordinate.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EditorDebugMarks {
    pub breakpoints: Vec<(usize, lume_ui::editor::BreakpointMark)>,
    pub current_line: Option<usize>,
    pub executed: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct DebugController {
    pub epoch: u64,
    pub phase: Option<Phase>,
    pub spawn: Option<DebugSpawn>,
    pub commands: Option<mpsc::Sender<DebugCommand>>,
    pub setup: Option<Setup>,
    /// La radice per cui `setup` è stato letto.
    pub setup_root: Option<PathBuf>,
    pub selected_config: usize,
    /// Configurazione costruita dal widget «Esegui» (File corrente in C):
    /// vince su quella scelta da `.lume/launch.toml` finché è presente.
    pub override_config: Option<LaunchConfig>,
    /// Fiducia volatile della sessione di Lume (D-DAP-6, D-DAP-9): radice del
    /// progetto e configurazione fidate. Si perde al riavvio.
    pub trusted: HashSet<(PathBuf, String)>,
    /// La conferma di fiducia è aperta.
    pub asking_trust: bool,
    pub capabilities: Capabilities,
    pub history: StopHistory,
    /// Frame dell'arresto corrente, con gli id validi fino al prossimo passo.
    pub frames: Vec<StackFrame>,
    pub selected_frame: usize,
    pub scopes: Vec<ScopeDetail>,
    pub snippet: Option<Snippet>,
    pub detail_error: Option<String>,
    pub children: HashMap<i64, (Vec<Variable>, bool)>,
    pub expanded: BTreeSet<i64>,
    pub console: Console,
    pub evaluations: VecDeque<(String, Result<String, String>)>,
    pub breakpoints: BTreeMap<PathBuf, BTreeMap<i64, BreakpointState>>,
    pub exit_code: Option<i64>,
    pub last_error: Option<String>,
    pub stop_reason: Option<String>,
    /// Segni per file, ricalcolati a ogni cambiamento (`refresh_debug_marks`).
    pub editor_marks: HashMap<PathBuf, EditorDebugMarks>,
    /// Widget «Esegui»: etichette delle voci e voce scelta.
    pub etichette_esegui: Vec<String>,
    pub scelta_esegui: usize,
}

impl DebugController {
    pub fn phase(&self) -> Phase {
        self.phase.unwrap_or(Phase::Ferma)
    }

    pub fn is_live(&self) -> bool {
        self.phase().is_live()
    }

    pub fn configs(&self) -> &[LaunchConfig] {
        match self.setup.as_ref().map(|setup| &setup.configs) {
            Some(Ok(configs)) => configs,
            _ => &[],
        }
    }

    pub fn selected_config(&self) -> Option<&LaunchConfig> {
        self.override_config
            .as_ref()
            .or_else(|| self.configs().get(self.selected_config))
    }

    /// I punti di interruzione da mandare all'avvio.
    pub fn file_breakpoints(&self) -> Vec<FileBreakpoints> {
        self.breakpoints
            .iter()
            .map(|(path, lines)| file_breakpoints(path, lines.keys().copied()))
            .collect()
    }

    /// Prepara una nuova sessione; restituisce un messaggio se manca qualcosa.
    pub fn prepare_spawn(&mut self) -> Result<(), String> {
        if self.is_live() {
            return Err("una sessione di debug è già attiva".into());
        }
        let setup = self
            .setup
            .as_ref()
            .ok_or_else(|| "configurazioni non ancora lette".to_owned())?;
        let adapter = setup
            .adapter
            .clone()
            .ok_or_else(|| "gdb non trovato su PATH".to_owned())?;
        let config = self
            .selected_config()
            .cloned()
            .ok_or_else(|| "nessuna configurazione di lancio".to_owned())?;
        self.epoch += 1;
        self.spawn = Some(DebugSpawn {
            epoch: self.epoch,
            adapter,
            config: Arc::new(config),
            breakpoints: Arc::new(self.file_breakpoints()),
        });
        self.phase = Some(Phase::Avvio);
        self.commands = None;
        self.history.clear();
        self.frames.clear();
        self.selected_frame = 0;
        self.scopes.clear();
        self.snippet = None;
        self.detail_error = None;
        self.children.clear();
        self.expanded.clear();
        self.evaluations.clear();
        self.exit_code = None;
        self.last_error = None;
        self.stop_reason = None;
        for lines in self.breakpoints.values_mut() {
            for state in lines.values_mut() {
                state.verified = None;
                state.id = None;
            }
        }
        Ok(())
    }

    /// Manda un comando all'attore. `Err` se la sessione non può riceverlo.
    pub fn send(&self, command: DebugCommand) -> Result<(), String> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| "nessuna sessione di debug pronta".to_owned())?;
        commands
            .try_send(command)
            .map_err(|_| "sessione di debug occupata o chiusa".to_owned())
    }

    /// Il frame scelto, solo se l'utente guarda l'arresto corrente.
    pub fn selected_live_frame(&self) -> Option<&StackFrame> {
        if !self.history.is_at_latest() || self.phase() != Phase::Fermo {
            return None;
        }
        self.frames.get(self.selected_frame)
    }

    /// Applica un evento dello stream. Restituisce un avviso per la status
    /// bar, se ce n'è uno.
    pub fn apply(&mut self, epoch: u64, event: DebugEvent) -> Option<String> {
        if epoch != self.epoch {
            return None;
        }
        match event {
            DebugEvent::Ready(commands) => {
                self.commands = Some(commands);
                None
            }
            DebugEvent::Started {
                capabilities,
                breakpoints,
            } => {
                self.capabilities = capabilities;
                self.phase = Some(Phase::InEsecuzione);
                for (path, stati) in breakpoints {
                    self.apply_breakpoints(&path, &stati);
                }
                None
            }
            DebugEvent::StartFailed(message) => {
                self.last_error = Some(message.clone());
                self.phase = Some(Phase::Arresto);
                Some(format!("Debug non avviato: {message}"))
            }
            DebugEvent::Stopped {
                stopped,
                thread_id,
                frames,
                code,
            } => {
                self.phase = Some(Phase::Fermo);
                self.stop_reason = Some(stopped.reason.clone());
                let description = stopped.description.clone().or(stopped.text.clone());
                self.history
                    .push_with_code(stopped.reason, description, thread_id, &frames, code);
                self.frames = frames;
                self.selected_frame = 0;
                self.scopes.clear();
                self.children.clear();
                self.expanded.clear();
                self.detail_error = None;
                None
            }
            DebugEvent::Continued => {
                self.phase = Some(Phase::InEsecuzione);
                self.frames.clear();
                self.scopes.clear();
                self.children.clear();
                None
            }
            DebugEvent::FrameDetail {
                frame_id,
                scopes,
                snippet,
                error,
            } => {
                let attuale = self.selected_live_frame().map(|frame| frame.id);
                if frame_id.is_some() && frame_id != attuale {
                    // Risposta di un frame non più scelto o di un passo superato.
                    return None;
                }
                self.scopes = scopes;
                self.snippet = snippet;
                self.detail_error = error;
                None
            }
            DebugEvent::Children {
                reference,
                variables,
                truncated,
            } => {
                if self.expanded.contains(&reference) {
                    self.children.insert(reference, (variables, truncated));
                }
                None
            }
            DebugEvent::Evaluated { expression, result } => {
                self.evaluations.push_back((expression, result));
                while self.evaluations.len() > MAX_EVALUATIONS {
                    self.evaluations.pop_front();
                }
                None
            }
            DebugEvent::BreakpointsSet { path, breakpoints } => {
                self.apply_breakpoints(&path, &breakpoints);
                None
            }
            DebugEvent::BreakpointChanged(breakpoint) => {
                for lines in self.breakpoints.values_mut() {
                    for state in lines.values_mut() {
                        if state.id.is_some() && state.id == breakpoint.id {
                            state.verified = Some(breakpoint.verified);
                            state.message.clone_from(&breakpoint.message);
                        }
                    }
                }
                None
            }
            DebugEvent::Output { category, text } => {
                self.console.push(category, &text);
                None
            }
            DebugEvent::Exited(code) => {
                self.exit_code = Some(code);
                Some(format!("Programma terminato con codice {code}"))
            }
            DebugEvent::CommandFailed(message) => {
                self.last_error = Some(message.clone());
                Some(format!("Debug: {message}"))
            }
            DebugEvent::Closed(outcome) => {
                self.phase = Some(Phase::Ferma);
                self.spawn = None;
                self.commands = None;
                self.frames.clear();
                self.scopes.clear();
                self.children.clear();
                for lines in self.breakpoints.values_mut() {
                    for state in lines.values_mut() {
                        state.verified = None;
                        state.id = None;
                    }
                }
                // Una chiusura forzata o incompleta si dice (AGENTS §5).
                if outcome.contains("incompleta") || outcome.contains("terminato") {
                    self.last_error = Some(outcome.clone());
                    Some(format!("Debug: {outcome}"))
                } else {
                    None
                }
            }
        }
    }

    fn apply_breakpoints(&mut self, path: &Path, stati: &[Breakpoint]) {
        let Some(lines) = self.breakpoints.get_mut(path) else {
            return;
        };
        // setBreakpoints risponde nello stesso ordine delle righe chieste.
        for ((_, state), risposta) in lines.iter_mut().zip(stati) {
            state.verified = Some(risposta.verified);
            state.message.clone_from(&risposta.message);
            state.id = risposta.id;
        }
    }

    /// Alterna un punto di interruzione; restituisce i punti del file da
    /// mandare all'adattatore se la sessione è viva.
    pub fn toggle_breakpoint(&mut self, path: &Path, line: i64) -> Option<FileBreakpoints> {
        let lines = self.breakpoints.entry(path.to_path_buf()).or_default();
        if lines.remove(&line).is_none() {
            lines.insert(
                line,
                BreakpointState {
                    verified: None,
                    message: None,
                    id: None,
                },
            );
        }
        let snapshot = file_breakpoints(path, lines.keys().copied());
        if lines.is_empty() {
            self.breakpoints.remove(path);
        }
        self.is_live().then_some(snapshot)
    }

    pub fn has_breakpoint(&self, path: &Path, line: i64) -> bool {
        self.breakpoints
            .get(path)
            .is_some_and(|lines| lines.contains_key(&line))
    }

    #[cfg(test)]
    pub fn breakpoint_count(&self) -> usize {
        self.breakpoints.values().map(BTreeMap::len).sum()
    }

    /// I segni dell'editor per ogni file toccato dal debugger.
    pub fn compute_editor_marks(&self) -> HashMap<PathBuf, EditorDebugMarks> {
        use lume_ui::editor::BreakpointMark;
        let riga = |line: i64| usize::try_from(line - 1).ok();
        let mut marks: HashMap<PathBuf, EditorDebugMarks> = HashMap::new();
        for (path, lines) in &self.breakpoints {
            let voce = marks.entry(path.clone()).or_default();
            voce.breakpoints = lines
                .iter()
                .filter_map(|(line, state)| {
                    let segno = match state.verified {
                        Some(true) => BreakpointMark::Verified,
                        Some(false) => BreakpointMark::Rejected,
                        None => BreakpointMark::Unsent,
                    };
                    riga(*line).map(|r| (r, segno))
                })
                .collect();
        }
        if self.is_live() {
            for stop in self.history.iter() {
                if let Some(frame) = stop.top()
                    && let Some(path) = &frame.path
                    && let Some(r) = riga(frame.line)
                {
                    marks.entry(path.clone()).or_default().executed.push(r);
                }
            }
        }
        if self.phase() == Phase::Fermo
            && let Some(frame) = self.frames.get(self.selected_frame)
            && let Some(path) = frame
                .source
                .as_ref()
                .and_then(|source| source.path.as_ref())
        {
            marks.entry(PathBuf::from(path)).or_default().current_line = riga(frame.line);
        }
        for voce in marks.values_mut() {
            voce.executed.sort_unstable();
            voce.executed.dedup();
        }
        marks
    }

    /// La riga corrente del programma in `path`, se è fermo lì.
    #[cfg(test)]
    pub fn current_line(&self, path: &Path) -> Option<i64> {
        if self.phase() != Phase::Fermo {
            return None;
        }
        let frame = self.frames.get(self.selected_frame)?;
        let frame_path = frame.source.as_ref()?.path.as_deref()?;
        (Path::new(frame_path) == path).then_some(frame.line)
    }
}

fn file_breakpoints(path: &Path, lines: impl Iterator<Item = i64>) -> FileBreakpoints {
    FileBreakpoints {
        path: path.to_path_buf(),
        breakpoints: lines
            .map(|line| SourceBreakpoint {
                line,
                condition: None,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use lume_dap::Source;

    fn frame(id: i64, name: &str, line: i64) -> StackFrame {
        StackFrame {
            id,
            name: name.into(),
            source: Some(Source {
                name: Some("prova.c".into()),
                path: Some("/p/prova.c".into()),
                source_reference: None,
            }),
            line,
            column: 0,
            instruction_pointer_reference: None,
        }
    }

    fn stopped(reason: &str) -> StoppedEvent {
        serde_json::from_value(serde_json::json!({"reason": reason, "threadId": 1})).unwrap()
    }

    #[test]
    fn console_limitata_in_righe_e_byte_con_righe_spezzate() {
        let mut console = Console::default();
        console.push(None, "ab");
        console.push(None, "c\nde\n");
        let righe: Vec<_> = console.lines().map(|(_, r)| r.clone()).collect();
        assert_eq!(righe, ["abc", "de"]);
        for numero in 0..(CONSOLE_MAX_LINES + 5) {
            console.push(None, &format!("{numero}\n"));
        }
        assert_eq!(console.len(), CONSOLE_MAX_LINES);
        console.clear();
        console.push(None, &"x".repeat(CONSOLE_MAX_LINE_BYTES * 2));
        assert_eq!(console.len(), 1);
    }

    #[test]
    fn eventi_di_un_epoch_superato_sono_ignorati_e_frame_stantii_rifiutati() {
        let mut debug = DebugController {
            epoch: 2,
            phase: Some(Phase::Avvio),
            ..DebugController::default()
        };
        debug.apply(1, DebugEvent::Exited(3));
        assert_eq!(debug.exit_code, None);
        debug.apply(
            2,
            DebugEvent::Stopped {
                stopped: stopped("breakpoint"),
                thread_id: 1,
                frames: vec![frame(0, "quadrato", 4), frame(1, "somma", 11)],
                code: vec![vec![(4, "int r = x * x;".into())]],
            },
        );
        assert_eq!(debug.phase(), Phase::Fermo);
        assert_eq!(debug.current_line(Path::new("/p/prova.c")), Some(4));
        let cima = debug.history.current().unwrap().top().unwrap();
        assert_eq!(cima.code, [(4, "int r = x * x;".to_owned())]);
        // Dettaglio del frame 1 mentre è scelto lo 0: scartato.
        debug.apply(
            2,
            DebugEvent::FrameDetail {
                frame_id: Some(1),
                scopes: Vec::new(),
                snippet: None,
                error: Some("vecchio".into()),
            },
        );
        assert_eq!(debug.detail_error, None);
        debug.apply(2, DebugEvent::Continued);
        assert_eq!(debug.phase(), Phase::InEsecuzione);
        assert_eq!(debug.current_line(Path::new("/p/prova.c")), None);
        debug.apply(2, DebugEvent::Closed("adattatore chiuso".into()));
        assert_eq!(debug.phase(), Phase::Ferma);
        assert!(debug.commands.is_none());
        assert_eq!(debug.history.len(), 1, "la storia resta consultabile");
    }

    #[test]
    fn breakpoint_alternati_e_verificati_dalla_risposta() {
        let mut debug = DebugController::default();
        let path = Path::new("/p/prova.c");
        assert!(
            debug.toggle_breakpoint(path, 4).is_none(),
            "sessione ferma: solo dati"
        );
        debug.toggle_breakpoint(path, 9);
        assert_eq!(debug.breakpoint_count(), 2);
        debug.phase = Some(Phase::Fermo);
        let risposta: Vec<Breakpoint> = serde_json::from_value(serde_json::json!([
            {"id": 1, "verified": true, "line": 4},
            {"id": 2, "verified": false, "message": "pending"}
        ]))
        .unwrap();
        debug.apply(
            0,
            DebugEvent::BreakpointsSet {
                path: path.into(),
                breakpoints: risposta,
            },
        );
        assert_eq!(debug.breakpoints[path][&4].verified, Some(true));
        assert_eq!(debug.breakpoints[path][&9].verified, Some(false));
        let da_mandare = debug.toggle_breakpoint(path, 4).unwrap();
        assert_eq!(da_mandare.breakpoints.len(), 1);
        assert!(!debug.has_breakpoint(path, 4));
        debug.toggle_breakpoint(path, 9);
        assert_eq!(debug.breakpoint_count(), 0);
        assert!(debug.breakpoints.is_empty());
    }

    #[test]
    fn senza_adattatore_o_configurazione_l_avvio_dice_cosa_manca() {
        let mut debug = DebugController::default();
        assert!(
            debug
                .prepare_spawn()
                .unwrap_err()
                .contains("configurazioni")
        );
        debug.setup = Some(Setup::vuoto(Err(LaunchError::Missing)));
        assert!(debug.prepare_spawn().unwrap_err().contains("gdb"));
        debug.setup = Some(Setup {
            adapter: Some("/usr/bin/gdb".into()),
            ..Setup::vuoto(Err(LaunchError::Missing))
        });
        assert!(
            debug
                .prepare_spawn()
                .unwrap_err()
                .contains("configurazione")
        );
        assert!(debug.spawn.is_none());
    }
}
