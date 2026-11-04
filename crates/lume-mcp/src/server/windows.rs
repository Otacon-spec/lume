//! Adattatore Windows: named pipe locale `\\.\pipe\lume-mcp-<pid>-<nonce8>`.
//! Il comportamento resta quello precedente al trasporto Unix.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::common::{Control, Shared, connection, handle_control};
use super::{Endpoint, MAX_INSTANCES, ServeConfig};
use crate::discovery::{self, DiscoveryFile, DiscoveryRecord};
use crate::events::McpEvent;
use crate::protocol::ServerInfo;

fn create(name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
    ServerOptions::new()
        // Solo la prima: se il nome è già preso, `create` fallisce invece
        // di servire accanto a un impostore.
        .first_pipe_instance(first)
        // È il default, ma si scrive: chiude la via SMB.
        .reject_remote_clients(true)
        // Conta **tutte** le istanze, anche quella in ascolto (doc tokio
        // `named_pipe.rs:2180-2206`); ogni `create` usa lo stesso valore.
        .max_instances(MAX_INSTANCES)
        .create(name)
}

/// Dopo la fine di una connessione Windows può liberare l'istanza della pipe
/// in modo asincrono: la `create` immediata fallisce con PIPE_BUSY. Si riprova
/// solo finché c'è un posto libero e manca il listener, con attesa crescente;
/// esauriti i tentativi il server lo dichiara invece di restare muto.
const RIPROVA_INIZIALE: Duration = Duration::from_millis(10);
const RIPROVA_MASSIMA: Duration = Duration::from_secs(1);
const RIPROVE: u32 = 20;

struct Riprova {
    attesa: Pin<Box<tokio::time::Sleep>>,
    ritardo: Duration,
    tentativi: u32,
}

impl Riprova {
    fn prima() -> Self {
        Self {
            attesa: Box::pin(tokio::time::sleep(RIPROVA_INIZIALE)),
            ritardo: RIPROVA_INIZIALE,
            tentativi: 1,
        }
    }

    fn successiva(self) -> Option<Self> {
        if self.tentativi >= RIPROVE {
            return None;
        }
        let ritardo = (self.ritardo * 2).min(RIPROVA_MASSIMA);
        Some(Self {
            attesa: Box::pin(tokio::time::sleep(ritardo)),
            ritardo,
            tentativi: self.tentativi + 1,
        })
    }
}

async fn attendi(riprova: &mut Option<Riprova>) {
    match riprova {
        Some(riprova) => riprova.attesa.as_mut().await,
        None => std::future::pending().await,
    }
}

async fn accept(listener: Option<&NamedPipeServer>) -> std::io::Result<()> {
    match listener {
        Some(pipe) => pipe.connect().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn serve(config: ServeConfig, events: mpsc::UnboundedSender<McpEvent>) {
    let ServeConfig {
        endpoint,
        token,
        root,
        server_version,
        availability,
        preamble_timeout,
        line_cap,
        ask_timeout,
    } = config;
    let Endpoint::Pipe {
        name: pipe_name,
        discovery_dir,
    } = endpoint;
    let mut listener = match create(&pipe_name, true) {
        Ok(pipe) => Some(pipe),
        Err(error) => {
            let _ = events.send(McpEvent::Failed(format!(
                "la pipe {pipe_name} non si crea (nome già preso?): {error}"
            )));
            return;
        }
    };
    let _discovery = match &discovery_dir {
        None => None,
        Some(dir) => {
            let _ = discovery::prune_stale(dir);
            let record = DiscoveryRecord {
                pid: std::process::id(),
                pipe: pipe_name.clone(),
                root: root.path().to_path_buf(),
                token: token.clone(),
                version: server_version.clone(),
                started_at: discovery::now_unix(),
            };
            match DiscoveryFile::write(dir, &record) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    let _ = events.send(McpEvent::Failed(format!(
                        "file di scoperta non scritto in {}: {error}",
                        dir.display()
                    )));
                    return;
                }
            }
        }
    };
    let (commands_tx, mut commands) = mpsc::unbounded_channel();
    if events.send(McpEvent::Ready(commands_tx)).is_err() {
        return;
    }
    let (availability_tx, availability_rx) = watch::channel(availability);
    let (internal_tx, mut internal) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        token,
        info: ServerInfo {
            version: server_version,
            project_root: root.path().display().to_string(),
        },
        root,
        preamble_timeout,
        line_cap,
        ask_timeout,
        next_ask: AtomicU64::new(0),
        internal: internal_tx,
        clients: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut pending: HashMap<u64, super::common::Reply> = HashMap::new();
    let mut next_conn = 0_u64;
    let mut connections: JoinSet<Option<u64>> = JoinSet::new();
    let mut riprova: Option<Riprova> = None;
    loop {
        tokio::select! {
            // `biased`: un `Connected` già in coda passa prima del
            // `ClientGone` della stessa connessione, o l'app conterebbe un
            // client fantasma.
            biased;
            command = commands.recv() => {
                if !handle_control(Control::Command(command), &mut pending, &events, &availability_tx) {
                    return;
                }
            }
            message = internal.recv() => {
                if !handle_control(Control::Internal(message), &mut pending, &events, &availability_tx) {
                    return;
                }
            }
            Some(joined) = connections.join_next(), if !connections.is_empty() => {
                if let Ok(Some(conn)) = joined
                    && events.send(McpEvent::ClientGone(conn)).is_err()
                {
                    return;
                }
                if listener.is_none() {
                    listener = create(&pipe_name, false).ok();
                    if listener.is_none() && riprova.is_none() {
                        riprova = Some(Riprova::prima());
                    }
                }
            }
            () = attendi(&mut riprova) => {
                let Some(corrente) = riprova.take() else { continue; };
                if listener.is_some() || connections.len() >= MAX_INSTANCES {
                    continue;
                }
                match create(&pipe_name, false) {
                    Ok(pipe) => listener = Some(pipe),
                    Err(error) => match corrente.successiva() {
                        Some(prossima) => riprova = Some(prossima),
                        None => {
                            let _ = events.send(McpEvent::Failed(format!(
                                "la pipe {pipe_name} non accetta nuovi client: l'istanza liberata non torna disponibile ({error})"
                            )));
                            return;
                        }
                    },
                }
            }
            accepted = accept(listener.as_ref()) => {
                let connected = listener.take();
                // Prima la prossima istanza, poi si passa via quella
                // connessa (doc tokio `named_pipe.rs:40-95`). Con quattro
                // istanze occupate `create` fallisce con PIPE_BUSY: si
                // riprova quando una connessione finisce.
                listener = create(&pipe_name, false).ok();
                if let (Ok(()), Some(pipe)) = (accepted, connected) {
                    next_conn += 1;
                    connections.spawn(connection(
                        pipe,
                        next_conn,
                        Arc::clone(&shared),
                        availability_rx.clone(),
                    ));
                }
                if listener.is_none() && connections.len() < MAX_INSTANCES && riprova.is_none() {
                    riprova = Some(Riprova::prima());
                }
            }
        }
    }
}
