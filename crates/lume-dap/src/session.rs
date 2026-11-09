//! Una sessione di debug: avvio dell'adattatore, sequenza di configurazione,
//! comandi.
//!
//! Sequenza (design §4): `initialize` → `launch` spedito senza attenderne la
//! risposta → evento `initialized` → `setBreakpoints` per file →
//! `configurationDone` (solo se l'adattatore lo dichiara) → risposta a
//! `launch`. Ogni passo ha una scadenza; un errore chiude l'adattatore e
//! riporta le ultime righe del suo stderr.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::adapters::AdapterId;
use crate::client::{AdapterEvent, DapClient, DapError};
use crate::history::STACK_LEVELS;
use crate::launch::LaunchConfig;
use crate::process::{AdapterProcess, SHUTDOWN_GRACE, ShutdownOutcome};
use crate::protocol::{
    Breakpoint, Capabilities, EvaluateResult, Response, Scope, SourceBreakpoint, StackFrame,
    Thread, Variable,
};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const INITIALIZED_TIMEOUT: Duration = Duration::from_secs(30);
/// Caricare i simboli di un binario grande può richiedere tempo.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);
pub const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Variabili accettate per risposta; oltre si tronca e lo si dice.
pub const MAX_VARIABLES: usize = 1000;

/// I punti di interruzione di un file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBreakpoints {
    pub path: PathBuf,
    pub breakpoints: Vec<SourceBreakpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SessionError {
    pub message: String,
}

impl SessionError {
    fn new(step: &str, error: impl std::fmt::Display, stderr: &str) -> Self {
        let stderr = stderr.trim();
        let message = if stderr.is_empty() {
            format!("{step}: {error}")
        } else {
            let coda: String = stderr
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            format!("{step}: {error}\n{coda}")
        };
        Self { message }
    }
}

/// Una sessione avviata, con i suoi eventi e lo stato iniziale dei
/// punti di interruzione.
pub struct StartedSession {
    pub session: DebugSession,
    pub events: mpsc::Receiver<AdapterEvent>,
    pub breakpoints: Vec<(PathBuf, Vec<Breakpoint>)>,
}

pub struct DebugSession {
    client: DapClient,
    process: Mutex<Option<AdapterProcess>>,
    capabilities: Capabilities,
    adapter: AdapterId,
}

fn body<T: DeserializeOwned>(response: Response, field: &str) -> Result<T, DapError> {
    let value = response
        .body
        .get(field)
        .cloned()
        .ok_or_else(|| DapError::InvalidBody {
            command: response.command.clone(),
            detail: format!("manca `{field}`"),
        })?;
    serde_json::from_value(value).map_err(|error| DapError::InvalidBody {
        command: response.command,
        detail: error.to_string(),
    })
}

fn breakpoints_arguments(file: &FileBreakpoints) -> Value {
    json!({
        "source": {
            "path": file.path.to_string_lossy(),
            "name": file.path.file_name().map(|nome| nome.to_string_lossy().into_owned()),
        },
        "breakpoints": file.breakpoints,
    })
}

impl DebugSession {
    /// Avvia l'adattatore `executable` e il programma di `config`.
    pub async fn start(
        executable: &Path,
        config: &LaunchConfig,
        breakpoints: &[FileBreakpoints],
    ) -> Result<StartedSession, SessionError> {
        let adapter = config.adapter;
        // gdb risponde con successo a `launch` anche senza programma: il
        // controllo va fatto qui, prima di avviare l'adattatore.
        if !config.program.is_file() {
            return Err(SessionError::new(
                "programma",
                format!("{} non esiste o non è un file", config.program.display()),
                "",
            ));
        }
        let command = adapter.command(executable, &config.cwd);
        let (process, stdout, stdin) = AdapterProcess::spawn(&command)
            .map_err(|error| SessionError::new("avvio dell'adattatore", error, ""))?;
        let (client, events) = DapClient::spawn(stdout, stdin, REQUEST_TIMEOUT);
        let mut session = Self {
            client,
            process: Mutex::new(Some(process)),
            capabilities: Capabilities::default(),
            adapter,
        };
        match session.configure(config, breakpoints).await {
            Ok(breakpoints) => Ok(StartedSession {
                session,
                events,
                breakpoints,
            }),
            Err((step, error)) => {
                let stderr = session.stderr_tail();
                session.close(false).await;
                Err(SessionError::new(step, error, &stderr))
            }
        }
    }

    async fn configure(
        &mut self,
        config: &LaunchConfig,
        breakpoints: &[FileBreakpoints],
    ) -> Result<Vec<(PathBuf, Vec<Breakpoint>)>, (&'static str, DapError)> {
        let initialize = self
            .client
            .request(
                "initialize",
                Some(json!({
                    "clientID": "lume",
                    "clientName": "Lume",
                    "adapterID": self.adapter.key(),
                    "locale": "it",
                    "linesStartAt1": true,
                    "columnsStartAt1": true,
                    "pathFormat": "path",
                })),
            )
            .await
            .map_err(|error| ("initialize", error))?;
        self.capabilities = serde_json::from_value(initialize.body).unwrap_or_default();
        let launch = self
            .client
            .send_request("launch", Some(self.adapter.launch_arguments(config)))
            .await
            .map_err(|error| ("launch", error))?;
        self.client
            .wait_initialized(INITIALIZED_TIMEOUT)
            .await
            .map_err(|error| ("initialized", error))?;
        let mut verified = Vec::with_capacity(breakpoints.len());
        for file in breakpoints {
            let stato = self
                .set_breakpoints(file)
                .await
                .map_err(|error| ("setBreakpoints", error))?;
            verified.push((file.path.clone(), stato));
        }
        if self.capabilities.supports_configuration_done_request {
            self.client
                .request("configurationDone", None)
                .await
                .map_err(|error| ("configurationDone", error))?;
        }
        launch
            .wait(LAUNCH_TIMEOUT)
            .await
            .map_err(|error| ("launch", error))?;
        Ok(verified)
    }

    pub fn adapter(&self) -> AdapterId {
        self.adapter
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    pub fn stderr_tail(&self) -> String {
        self.process
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(AdapterProcess::stderr_tail)
            .unwrap_or_default()
    }

    pub async fn set_breakpoints(
        &self,
        file: &FileBreakpoints,
    ) -> Result<Vec<Breakpoint>, DapError> {
        let response = self
            .client
            .request("setBreakpoints", Some(breakpoints_arguments(file)))
            .await?;
        body(response, "breakpoints")
    }

    pub async fn threads(&self) -> Result<Vec<Thread>, DapError> {
        let response = self.client.request("threads", None).await?;
        body(response, "threads")
    }

    pub async fn stack_trace(&self, thread_id: i64) -> Result<Vec<StackFrame>, DapError> {
        let response = self
            .client
            .request(
                "stackTrace",
                Some(json!({ "threadId": thread_id, "startFrame": 0, "levels": STACK_LEVELS })),
            )
            .await?;
        let mut frames: Vec<StackFrame> = body(response, "stackFrames")?;
        frames.truncate(STACK_LEVELS);
        Ok(frames)
    }

    pub async fn scopes(&self, frame_id: i64) -> Result<Vec<Scope>, DapError> {
        let response = self
            .client
            .request("scopes", Some(json!({ "frameId": frame_id })))
            .await?;
        body(response, "scopes")
    }

    /// Le variabili di un riferimento; il booleano dice se sono state
    /// troncate a [`MAX_VARIABLES`].
    pub async fn variables(&self, reference: i64) -> Result<(Vec<Variable>, bool), DapError> {
        let response = self
            .client
            .request(
                "variables",
                Some(json!({ "variablesReference": reference })),
            )
            .await?;
        let mut variables: Vec<Variable> = body(response, "variables")?;
        let troncate = variables.len() > MAX_VARIABLES;
        variables.truncate(MAX_VARIABLES);
        Ok((variables, troncate))
    }

    async fn thread_command(&self, command: &str, thread_id: i64) -> Result<(), DapError> {
        self.client
            .request(command, Some(json!({ "threadId": thread_id })))
            .await
            .map(|_| ())
    }

    pub async fn continue_(&self, thread_id: i64) -> Result<(), DapError> {
        self.thread_command("continue", thread_id).await
    }

    pub async fn next(&self, thread_id: i64) -> Result<(), DapError> {
        self.thread_command("next", thread_id).await
    }

    pub async fn step_in(&self, thread_id: i64) -> Result<(), DapError> {
        self.thread_command("stepIn", thread_id).await
    }

    pub async fn step_out(&self, thread_id: i64) -> Result<(), DapError> {
        self.thread_command("stepOut", thread_id).await
    }

    pub async fn pause(&self, thread_id: i64) -> Result<(), DapError> {
        self.thread_command("pause", thread_id).await
    }

    /// Valuta un'espressione nel frame dato. È un gesto esplicito: può
    /// modificare il programma (un'assegnazione, una chiamata).
    ///
    /// Contesto `watch`, mai `repl`: in gdb `repl` esegue comandi del
    /// debugger (`n` diventa `next`, `shell ...` lancerebbe una shell).
    pub async fn evaluate(
        &self,
        expression: &str,
        frame_id: Option<i64>,
    ) -> Result<EvaluateResult, DapError> {
        let mut arguments = Map::new();
        arguments.insert("expression".into(), Value::from(expression));
        arguments.insert("context".into(), Value::from("watch"));
        if let Some(frame_id) = frame_id {
            arguments.insert("frameId".into(), Value::from(frame_id));
        }
        let response = self
            .client
            .request("evaluate", Some(Value::Object(arguments)))
            .await?;
        serde_json::from_value(response.body).map_err(|error| DapError::InvalidBody {
            command: "evaluate".into(),
            detail: error.to_string(),
        })
    }

    /// Chiude la sessione: `disconnect` (con terminazione del programma se
    /// chiesta e dichiarata), poi attesa e raccolta dell'adattatore.
    pub async fn close(&self, terminate_debuggee: bool) -> ShutdownOutcome {
        let terminate = terminate_debuggee && self.capabilities.support_terminate_debuggee;
        if self.client.closed_reason().is_none()
            && let Ok(pending) = self
                .client
                .send_request(
                    "disconnect",
                    Some(json!({ "terminateDebuggee": terminate })),
                )
                .await
        {
            // L'esito del disconnect non cambia cosa fare dopo: si raccoglie
            // comunque l'adattatore.
            let _ = pending.wait(DISCONNECT_TIMEOUT).await;
        }
        let process = self
            .process
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match process {
            Some(process) => process.shutdown(SHUTDOWN_GRACE).await,
            None => ShutdownOutcome::Failed("sessione già chiusa".into()),
        }
    }
}
