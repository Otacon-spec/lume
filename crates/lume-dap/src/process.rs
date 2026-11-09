//! Il processo dell'adattatore: possesso, stderr limitato, chiusura con
//! scadenza.
//!
//! Limiti dichiarati (design §Rischi): su Unix il figlio è capo del proprio
//! gruppo, così i segnali del terminale di Lume non lo raggiungono; alla
//! chiusura si attende l'uscita per [`SHUTDOWN_GRACE`], poi si termina il solo
//! adattatore e se ne raccoglie lo stato. Il programma debuggato è figlio
//! dell'adattatore: con `disconnect` e `terminateDebuggee` gdb lo termina
//! (provato in `verifica_debugger_xvfb.py`); se gdb viene ucciso, la fine del
//! programma è attesa (`PTRACE_O_EXITKILL` su Linux) ma non provata. Non è un
//! contenitore di tutto l'albero come il Job Object di `lume-lsp`.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;

/// Attesa dell'uscita spontanea prima di terminare l'adattatore.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
/// Byte di stderr conservati (gli ultimi).
pub const STDERR_CAPACITY: usize = 64 * 1024;

/// Il comando che avvia un adattatore stdio. Argomenti come array, mai
/// attraverso una shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// Come è finito l'adattatore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// Uscito da sé entro la scadenza.
    Exited(Option<i32>),
    /// Terminato da Lume dopo la scadenza, poi raccolto.
    Killed(Option<i32>),
    /// Non è stato possibile raccoglierlo: l'errore dice perché.
    Failed(String),
}

#[derive(Default)]
struct StderrRing {
    bytes: VecDeque<u8>,
}

impl StderrRing {
    fn push(&mut self, data: &[u8]) {
        self.bytes.extend(data);
        while self.bytes.len() > STDERR_CAPACITY {
            self.bytes.pop_front();
        }
    }
}

pub struct AdapterProcess {
    child: Child,
    stderr: Arc<Mutex<StderrRing>>,
    stderr_task: Option<JoinHandle<()>>,
}

impl Drop for AdapterProcess {
    fn drop(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
        // `kill_on_drop` termina il figlio; tokio lo raccoglie.
    }
}

impl AdapterProcess {
    /// Avvia l'adattatore e restituisce le sue pipe.
    pub fn spawn(command: &AdapterCommand) -> io::Result<(Self, ChildStdout, ChildStdin)> {
        let mut builder = Command::new(&command.program);
        builder
            .args(&command.args)
            .current_dir(&command.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        builder.process_group(0);
        let mut child = builder.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("stdout dell'adattatore assente"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("stdin dell'adattatore assente"))?;
        let stderr = Arc::new(Mutex::new(StderrRing::default()));
        let stderr_task = child.stderr.take().map(|mut pipe| {
            let ring = Arc::clone(&stderr);
            tokio::spawn(async move {
                let mut chunk = vec![0_u8; 4096];
                loop {
                    match pipe.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(letti) => {
                            if let Some(dati) = chunk.get(..letti) {
                                ring.lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .push(dati);
                            }
                        }
                    }
                }
            })
        });
        Ok((
            Self {
                child,
                stderr,
                stderr_task,
            },
            stdout,
            stdin,
        ))
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Le ultime righe di stderr, per spiegare un errore.
    pub fn stderr_tail(&self) -> String {
        let ring = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes: Vec<u8> = ring.bytes.iter().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Attende l'uscita per `grace`, poi termina e raccoglie.
    pub async fn shutdown(mut self, grace: Duration) -> ShutdownOutcome {
        let code = |status: ExitStatus| status.code();
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(Ok(status)) => ShutdownOutcome::Exited(code(status)),
            Ok(Err(error)) => ShutdownOutcome::Failed(error.to_string()),
            Err(_) => {
                if let Err(error) = self.child.start_kill() {
                    return ShutdownOutcome::Failed(error.to_string());
                }
                match self.child.wait().await {
                    Ok(status) => ShutdownOutcome::Killed(code(status)),
                    Err(error) => ShutdownOutcome::Failed(error.to_string()),
                }
            }
        }
    }
}
