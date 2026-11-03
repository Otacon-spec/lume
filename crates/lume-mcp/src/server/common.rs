//! Dispatcher comune a pipe Windows e socket Unix.
//!
//! Autenticazione, framing, sessione MCP, tool, consenso e coda delle
//! chiamate vivono qui una volta sola. Gli adattatori di piattaforma
//! possiedono soltanto listener, accept, permessi e scoperta.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinSet};

use super::{MAX_INFLIGHT, MAX_INSTANCES, PREAMBLE_CAP};
use crate::catalog::{
    Args, ToolError, ToolName, arg_opt_bool, arg_opt_str, arg_opt_str_array, arg_opt_u64,
    arg_position, arg_str,
};
use crate::confine::Root;
use crate::discovery::{self, Token};
use crate::events::{AppAnswer, AppQuery, McpCommand, McpEvent};
use crate::framing::LineReader;
use crate::protocol::{
    INVALID_PARAMS, Outcome, SERVER_BUSY, ServerInfo, Session, list_changed_notification,
    reply_error, reply_ok, tool_result,
};
use crate::{McpId, edit, exec, tools};

const INTERNAL_ERROR: i64 = -32603;

pub(super) type Reply = oneshot::Sender<Result<AppAnswer, String>>;

pub(super) enum Internal {
    Ask {
        id: u64,
        conn: u64,
        query: AppQuery,
        reply: Reply,
    },
    AskGone(u64),
    Connected {
        conn: u64,
        name: String,
    },
}

pub(super) struct Shared {
    pub(super) token: Token,
    pub(super) root: Root,
    pub(super) info: ServerInfo,
    pub(super) preamble_timeout: Duration,
    pub(super) line_cap: usize,
    pub(super) ask_timeout: Duration,
    pub(super) next_ask: AtomicU64,
    pub(super) internal: mpsc::UnboundedSender<Internal>,
    /// Client che hanno superato il preambolo: è il solo conteggio che vale
    /// per `MAX_INSTANCES`. Le connessioni di prova della scoperta non lo
    /// toccano.
    pub(super) clients: AtomicUsize,
}

/// Il ciclo degli eventi comuni: domande dell'app, risposte, cambi dei tool.
/// L'adattatore di piattaforma aggiunge l'accept e la raccolta connessioni.
pub(super) enum Control {
    Command(Option<McpCommand>),
    Internal(Option<Internal>),
}

pub(super) fn handle_control(
    control: Control,
    pending: &mut HashMap<u64, Reply>,
    events: &mpsc::UnboundedSender<McpEvent>,
    availability_tx: &watch::Sender<crate::catalog::Availability>,
) -> bool {
    match control {
        Control::Command(Some(McpCommand::Answer { id, answer })) => {
            if let Some(reply) = pending.remove(&id) {
                // chi ha chiesto può essere già andato via
                let _ = reply.send(answer);
            }
        }
        Control::Command(Some(McpCommand::ToolsChanged(now))) => {
            let _ = availability_tx.send_if_modified(|current| {
                let changed = *current != now;
                *current = now;
                changed
            });
        }
        // l'app ha lasciato cadere la mailbox: lo stream sta morendo
        Control::Command(None) => return false,
        Control::Internal(Some(Internal::Ask {
            id,
            conn,
            query,
            reply,
        })) => {
            pending.insert(id, reply);
            if events.send(McpEvent::Ask { id, conn, query }).is_err() {
                return false;
            }
        }
        Control::Internal(Some(Internal::AskGone(id))) => {
            if pending.remove(&id).is_some() && events.send(McpEvent::AskGone(id)).is_err() {
                return false;
            }
        }
        Control::Internal(Some(Internal::Connected { conn, name })) => {
            if events
                .send(McpEvent::ClientConnected { conn, name })
                .is_err()
            {
                return false;
            }
        }
        Control::Internal(None) => return false,
    }
    true
}

/// Ciclo di una connessione. Il valore restituito è `Some(conn)` solo se il
/// preambolo è stato autenticato: le connessioni di sola verifica del ponte e
/// i token sbagliati non devono produrre un `ClientGone` per un client mai
/// entrato. Il tetto `MAX_INSTANCES` conta solo le connessioni autenticate: al
/// tetto il server legge il preambolo e risponde con un errore JSON-RPC
/// esplicito, così nessun byte esce prima del preambolo e il ponte distingue
/// «occupato» da una chiusura di trasporto.
pub(super) async fn connection<S>(
    stream: S,
    conn: u64,
    shared: Arc<Shared>,
    mut availability: watch::Receiver<crate::catalog::Availability>,
) -> Option<u64>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = LineReader::new(read, PREAMBLE_CAP);
    let authenticated = matches!(
        tokio::time::timeout(shared.preamble_timeout, reader.next_line()).await,
        Ok(Ok(Some(line))) if discovery::check_preamble(&line, &shared.token)
    );
    if !authenticated {
        return None;
    }
    let Some(_slot) = ClientSlot::claim(&shared) else {
        // Rifiuto esplicito al tetto: la riga di tetto arriva solo a chi ha
        // superato il preambolo, poi la connessione si chiude.
        let _ = write_line(&mut write, &crate::protocol::busy_line()).await;
        return None;
    };
    reader.set_max(shared.line_cap);
    let mut session = Session::new();
    let mut calls: JoinSet<(McpId, String)> = JoinSet::new();
    let mut aborts: HashMap<McpId, AbortHandle> = HashMap::new();
    loop {
        tokio::select! {
            line = reader.next_line() => {
                let Ok(Some(line)) = line else { break; };
                if line.iter().all(u8::is_ascii_whitespace) { continue; }
                let now = *availability.borrow();
                let out = match session.handle(&line, &shared.info, now) {
                    Outcome::Reply(reply) => Some(reply),
                    Outcome::Initialized { reply, client } => {
                        let _ = shared.internal.send(Internal::Connected { conn, name: client });
                        Some(reply)
                    }
                    Outcome::Call { id, tool, arguments } => {
                        if aborts.contains_key(&id) {
                            Some(reply_error(Some(&id), INVALID_PARAMS, "esiste già una chiamata in corso con questo id"))
                        } else if aborts.len() >= MAX_INFLIGHT {
                            Some(reply_error(Some(&id), SERVER_BUSY, "troppe chiamate in corso su questa connessione: riprova fra poco"))
                        } else {
                            let for_task = Arc::clone(&shared);
                            let call_id = id.clone();
                            let handle = calls.spawn(async move {
                                let result = run_tool(tool, arguments, &for_task, conn).await;
                                (call_id.clone(), reply_ok(&call_id, tool_result(result)))
                            });
                            aborts.insert(id, handle);
                            None
                        }
                    }
                    Outcome::Cancelled(id) => {
                        if let Some(handle) = aborts.remove(&id) { handle.abort(); }
                        None
                    }
                    Outcome::Nothing => None,
                };
                if let Some(out) = out && write_line(&mut write, &out).await.is_err() { break; }
            }
            Some(joined) = calls.join_next_with_id(), if !calls.is_empty() => {
                let out = match joined {
                    Ok((_, (id, line))) => { aborts.remove(&id); Some(line) }
                    Err(error) if error.is_cancelled() => None,
                    Err(error) => {
                        let orphan = aborts.iter().find(|(_, handle)| handle.id() == error.id()).map(|(id, _)| id.clone());
                        orphan.map(|id| {
                            aborts.remove(&id);
                            reply_error(Some(&id), INTERNAL_ERROR, "la chiamata si è interrotta")
                        })
                    }
                };
                if let Some(out) = out && write_line(&mut write, &out).await.is_err() { break; }
            }
            changed = availability.changed() => {
                if changed.is_err() { break; }
                if session.is_initialized() && write_line(&mut write, &list_changed_notification()).await.is_err() { break; }
            }
        }
    }
    calls.shutdown().await;
    Some(conn)
}

async fn write_line<W: AsyncWrite + Unpin>(write: &mut W, line: &str) -> std::io::Result<()> {
    write.write_all(line.as_bytes()).await?;
    write.write_all(b"\n").await?;
    write.flush().await
}

/// Slot di un client autenticato: il tetto vale per chi ha superato il
/// preambolo, non per le connessioni di prova della scoperta. Il `Drop` libera
/// lo slot anche se la connessione finisce per errore.
struct ClientSlot {
    shared: Arc<Shared>,
}

impl ClientSlot {
    fn claim(shared: &Arc<Shared>) -> Option<Self> {
        let mut current = shared.clients.load(Ordering::SeqCst);
        loop {
            if current >= MAX_INSTANCES {
                return None;
            }
            match shared.clients.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Some(Self {
                        shared: Arc::clone(shared),
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        self.shared.clients.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn run_tool(
    tool: ToolName,
    args: Args,
    shared: &Shared,
    conn: u64,
) -> Result<Value, ToolError> {
    match tool {
        ToolName::ListFiles => blocking(shared, args, tools::fs::list_files).await,
        ToolName::SearchText => blocking(shared, args, tools::fs::search_text).await,
        ToolName::GitStatus => blocking(shared, args, tools::git::git_status).await,
        ToolName::GitDiffFile => blocking(shared, args, tools::git::git_diff_file).await,
        ToolName::GitLog => blocking(shared, args, tools::git::git_log).await,
        ToolName::GitBlame => blocking(shared, args, tools::git::git_blame).await,
        ToolName::ReadFile => read_file(args, shared, conn).await,
        ToolName::ListOpenFiles => json_answer(ask(shared, conn, AppQuery::OpenFiles).await?),
        ToolName::GetActiveFile => json_answer(ask(shared, conn, AppQuery::ActiveFile).await?),
        ToolName::GetDiagnostics => {
            let relative = match arg_opt_str(&args, "path")? {
                Some(path) => Some(shared.root.resolve(path)?.relative),
                None => None,
            };
            let max_severity = match arg_opt_str(&args, "severity")? {
                None | Some("hint") => 4,
                Some("error") => 1,
                Some("warning") => 2,
                Some("info") => 3,
                Some(other) => {
                    return Err(ToolError::new(format!("severity sconosciuta: {other}")));
                }
            };
            json_answer(
                ask(
                    shared,
                    conn,
                    AppQuery::Diagnostics {
                        relative,
                        max_severity,
                    },
                )
                .await?,
            )
        }
        ToolName::GetHover | ToolName::GotoDefinition | ToolName::FindUsages => {
            let file = existing_file(arg_str(&args, "path")?, shared)?;
            let (line, column) = arg_position(&args)?;
            let query = match tool {
                ToolName::GetHover => AppQuery::Hover {
                    relative: file.relative,
                    line,
                    column,
                },
                ToolName::GotoDefinition => AppQuery::Definition {
                    relative: file.relative,
                    line,
                    column,
                },
                ToolName::FindUsages => AppQuery::References {
                    relative: file.relative,
                    line,
                    column,
                },
                _ => return Err(ToolError::new("tool LSP posizionale non riconosciuto")),
            };
            json_answer(ask(shared, conn, query).await?)
        }
        ToolName::GetDocumentSymbols => {
            let file = existing_file(arg_str(&args, "path")?, shared)?;
            json_answer(
                ask(
                    shared,
                    conn,
                    AppQuery::DocumentSymbols {
                        relative: file.relative,
                    },
                )
                .await?,
            )
        }
        ToolName::WriteFile => {
            let content = arg_str(&args, "content")?;
            edit::check_size(content).map_err(ToolError::new)?;
            let file = shared.root.resolve_for_write(arg_str(&args, "path")?)?;
            json_answer(
                ask(
                    shared,
                    conn,
                    AppQuery::WriteFile {
                        relative: file.relative,
                        content: content.to_owned(),
                    },
                )
                .await?,
            )
        }
        ToolName::ReplaceText => {
            let file = shared.root.resolve_for_write(arg_str(&args, "path")?)?;
            if !file.absolute.is_file() {
                return Err(ToolError::new(format!("non è un file: {}", file.slash())));
            }
            let old_text = arg_str(&args, "old_text")?;
            if old_text.is_empty() {
                return Err(ToolError::new("old_text è vuoto"));
            }
            let new_text = arg_str(&args, "new_text")?;
            edit::check_size(old_text).map_err(ToolError::new)?;
            edit::check_size(new_text).map_err(ToolError::new)?;
            json_answer(
                ask(
                    shared,
                    conn,
                    AppQuery::ReplaceText {
                        relative: file.relative,
                        old_text: old_text.to_owned(),
                        new_text: new_text.to_owned(),
                        all: arg_opt_bool(&args, "replace_all")?.unwrap_or(false),
                    },
                )
                .await?,
            )
        }
        ToolName::Rename => {
            let file = existing_file(arg_str(&args, "path")?, shared)?;
            let (line, column) = arg_position(&args)?;
            let new_name = arg_str(&args, "new_name")?;
            if new_name.is_empty() || new_name.chars().any(char::is_whitespace) {
                return Err(ToolError::new(
                    "new_name dev'essere un identificatore: non vuoto e senza spazi",
                ));
            }
            json_answer(
                ask(
                    shared,
                    conn,
                    AppQuery::Rename {
                        relative: file.relative,
                        line,
                        column,
                        new_name: new_name.to_owned(),
                    },
                )
                .await?,
            )
        }
        ToolName::RunCommand => run_command(&args, shared, conn).await,
        ToolName::OpenInEditor => {
            let file = existing_file(arg_str(&args, "path")?, shared)?;
            let line =
                arg_opt_u64(&args, "line")?.map(|n| usize::try_from(n).unwrap_or(usize::MAX));
            json_answer(
                ask(
                    shared,
                    conn,
                    AppQuery::OpenInEditor {
                        relative: file.relative,
                        line,
                    },
                )
                .await?,
            )
        }
    }
}

fn existing_file(requested: &str, shared: &Shared) -> Result<crate::confine::Confined, ToolError> {
    let file = shared.root.resolve(requested)?;
    if !file.absolute.is_file() {
        return Err(ToolError::new(format!("non è un file: {}", file.slash())));
    }
    Ok(file)
}

async fn run_command(args: &Args, shared: &Shared, conn: u64) -> Result<Value, ToolError> {
    let program = arg_str(args, "program")?.trim().to_owned();
    if program.is_empty() {
        return Err(ToolError::new("program è vuoto"));
    }
    let argv = arg_opt_str_array(args, "args")?.unwrap_or_default();
    let cwd = shared
        .root
        .resolve(arg_opt_str(args, "cwd")?.unwrap_or("."))?;
    if !cwd.absolute.is_dir() {
        return Err(ToolError::new(format!(
            "cwd non è una cartella: {}",
            cwd.slash()
        )));
    }
    let timeout = exec::timeout_from(arg_opt_u64(args, "timeout_secs")?).map_err(ToolError::new)?;
    match ask(
        shared,
        conn,
        AppQuery::RunCommand {
            program: program.clone(),
            args: argv.clone(),
            cwd: cwd.relative,
            timeout_secs: timeout.as_secs(),
        },
    )
    .await?
    {
        AppAnswer::Granted => {}
        AppAnswer::Json(_) | AppAnswer::Buffer(_) => {
            return Err(ToolError::new("risposta inattesa dall'app"));
        }
    }
    exec::run(exec::RunSpec {
        program,
        args: argv,
        cwd: cwd.absolute,
        timeout,
    })
    .await
    .map(|report| report.to_json())
    .map_err(ToolError::new)
}

/// Un lavoro sincrono che finisce da sé (walk, ricerca col tetto, gix).
async fn blocking(
    shared: &Shared,
    args: Args,
    run: fn(&Root, &Args) -> Result<Value, ToolError>,
) -> Result<Value, ToolError> {
    let root = shared.root.clone();
    tokio::task::spawn_blocking(move || run(&root, &args))
        .await
        .map_err(|error| ToolError::new(format!("il lavoro si è interrotto: {error}")))?
}

pub(super) async fn ask(
    shared: &Shared,
    conn: u64,
    query: AppQuery,
) -> Result<AppAnswer, ToolError> {
    let id = shared.next_ask.fetch_add(1, Ordering::Relaxed) + 1;
    let (reply, answer) = oneshot::channel();
    shared
        .internal
        .send(Internal::Ask {
            id,
            conn,
            query,
            reply,
        })
        .map_err(|_| ToolError::new("Lume sta fermando il server"))?;
    let mut guard = AskGuard {
        id,
        internal: &shared.internal,
        armed: true,
    };
    match tokio::time::timeout(shared.ask_timeout, answer).await {
        Ok(Ok(result)) => {
            guard.armed = false;
            result.map_err(ToolError::new)
        }
        Ok(Err(_)) => {
            guard.armed = false;
            Err(ToolError::new("Lume non ha risposto"))
        }
        Err(_) => Err(ToolError::new(format!(
            "nessuna risposta entro {} s: la richiesta è scaduta",
            shared.ask_timeout.as_secs()
        ))),
    }
}

struct AskGuard<'a> {
    id: u64,
    internal: &'a mpsc::UnboundedSender<Internal>,
    armed: bool,
}
impl Drop for AskGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.internal.send(Internal::AskGone(self.id));
        }
    }
}

fn json_answer(answer: AppAnswer) -> Result<Value, ToolError> {
    match answer {
        AppAnswer::Json(value) => Ok(value),
        AppAnswer::Buffer(_) | AppAnswer::Granted => {
            Err(ToolError::new("risposta inattesa dall'app"))
        }
    }
}

/// Il buffer vince anche se sporco (`dirty: true`); se il file non è
/// aperto si legge il disco, fuori dal thread dell'interfaccia.
async fn read_file(args: Args, shared: &Shared, conn: u64) -> Result<Value, ToolError> {
    let file = existing_file(arg_str(&args, "path")?, shared)?;
    let (snapshot, source) = match ask(
        shared,
        conn,
        AppQuery::Buffer {
            relative: file.relative.clone(),
        },
    )
    .await?
    {
        AppAnswer::Buffer(Some(snapshot)) => (snapshot, "buffer"),
        AppAnswer::Buffer(None) => {
            let path = file.absolute.clone();
            let snapshot = tokio::task::spawn_blocking(move || tools::fs::read_disk(&path))
                .await
                .map_err(|error| ToolError::new(format!("lettura interrotta: {error}")))??;
            (snapshot, "disk")
        }
        AppAnswer::Json(_) | AppAnswer::Granted => {
            return Err(ToolError::new("risposta inattesa dall'app"));
        }
    };
    tools::fs::render_read(&file.slash(), source, &snapshot, &args)
}
