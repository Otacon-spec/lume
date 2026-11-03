//! Adattatore Unix: socket filesystem privato `<dati>/mcp-unix-v1/<id>.sock`.
//!
//! Il listener nasce solo dopo la verifica della cartella e della catena di
//! antenati; il record viene pubblicato solo dopo bind e verifica dei
//! permessi. Ogni connessione viene accettata solo se il kernel riporta
//! l'UID effettivo del peer.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::common::{Control, Shared, connection, handle_control};
use super::{Endpoint, MAX_PENDING_HANDSHAKES, ServeConfig};
use crate::events::McpEvent;
use crate::protocol::ServerInfo;
use crate::unix;

/// Senza console (release) stderr può mancare: la scrittura fallisce in
/// silenzio invece di andare in panic come farebbe `eprintln!`.
fn warn(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr(), "lume MCP: {message}");
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
    let Endpoint::Unix(endpoint) = endpoint;
    let (listener, _discovery, notices) =
        match unix::bind(&endpoint, &token, root.path(), &server_version).await {
            Ok(bound) => bound,
            Err(error) => {
                let _ = events.send(McpEvent::Failed(error));
                return;
            }
        };
    for notice in notices {
        warn(&format!(
            "record di scoperta ignorato in {}: {}",
            notice.path.display(),
            notice.reason
        ));
    }
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
            }
            // L'accept resta sempre attivo: al tetto il server accetta e
            // risponde con un errore esplicito dopo il preambolo, così il ponte
            // non resta in attesa nel backlog. Qui si limita solo il numero di
            // connessioni che non hanno ancora mandato il preambolo; oltre il
            // tetto si accetta e si chiude senza scrivere byte.
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        if connections.len() >= MAX_PENDING_HANDSHAKES {
                            // Limite di risorse: nessun byte, chiusura. Il
                            // rifiuto «occupato» è un altro: arriva dopo il
                            // preambolo, con la riga di tetto.
                            drop(stream);
                        } else {
                            match unix::peer_uid(&stream) {
                                Ok(uid) if uid == unix::effective_uid() => {
                                    next_conn += 1;
                                    connections.spawn(connection(
                                        stream,
                                        next_conn,
                                        Arc::clone(&shared),
                                        availability_rx.clone(),
                                    ));
                                }
                                // Un peer di un altro account, o senza
                                // credenziale kernel, viene chiuso senza che il
                                // protocollo veda un byte.
                                Ok(_) | Err(_) => {}
                            }
                        }
                    }
                    Err(error) => {
                        let _ = events.send(McpEvent::Failed(format!(
                            "il socket {} non accetta connessioni: {error}",
                            endpoint.socket_path().display()
                        )));
                        return;
                    }
                }
            }
        }
    }
}
