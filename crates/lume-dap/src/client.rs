//! Il client DAP su un canale qualsiasi (`AsyncRead` + `AsyncWrite`).
//!
//! Generico come `ClientTransport` di `lume-lsp`: in prova gira su
//! `tokio::io::duplex`, nell'app sulle pipe dell'adattatore. Due task, uno
//! per lato; le code sono limitate:
//!
//! - in uscita al più [`OUTBOUND_CAPACITY`] frame;
//! - richieste in attesa di risposta al più [`PENDING_CAPACITY`];
//! - eventi verso chi consuma al più [`EVENT_CAPACITY`]: se la coda è piena
//!   il lettore aspetta, quindi l'adattatore rallenta invece di far crescere
//!   la memoria.
//!
//! Le richieste inverse (`runInTerminal`, `startDebugging`) ricevono una
//! risposta negativa e non eseguono nulla (D-DAP-7).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lume_lsp::FrameDecoder;
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::protocol::{self, Event, Incoming, Response};

pub const OUTBOUND_CAPACITY: usize = 64;
pub const PENDING_CAPACITY: usize = 64;
pub const EVENT_CAPACITY: usize = 256;
/// Limite di un singolo frame in uscita, lo stesso del frame in entrata.
pub const MAX_FRAME_BYTES: usize = lume_lsp::MAX_BODY_BYTES;

const READ_CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DapError {
    #[error("l'adattatore ha chiuso la connessione: {0}")]
    Closed(String),
    #[error("nessuna risposta a `{command}` entro {seconds} s")]
    Timeout { command: String, seconds: u64 },
    #[error("l'adattatore ha rifiutato `{command}`: {message}")]
    Rejected { command: String, message: String },
    #[error("troppe richieste in attesa ({0})")]
    TooManyPending(usize),
    #[error("richiesta `{0}` oltre il limite di dimensione")]
    TooLarge(String),
    #[error("risposta a `{command}` non valida: {detail}")]
    InvalidBody { command: String, detail: String },
}

/// Ciò che il client consegna a chi consuma, in ordine di arrivo.
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterEvent {
    Event(Event),
    /// La connessione è finita; il testo dice perché. È sempre l'ultimo.
    Closed(String),
}

struct Shared {
    seq: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<Response>>>,
    initialized: watch::Sender<bool>,
    closed: watch::Sender<Option<String>>,
}

impl Shared {
    fn next_seq(&self) -> i64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<i64, oneshot::Sender<Response>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self, reason: &str) {
        self.closed.send_if_modified(|stato| {
            if stato.is_some() {
                return false;
            }
            *stato = Some(reason.to_owned());
            true
        });
        // Chiudere i mittenti fa fallire le attese con `Closed`.
        self.pending().clear();
    }

    fn closed_reason(&self) -> Option<String> {
        self.closed.borrow().clone()
    }
}

pub struct DapClient {
    outbound: mpsc::Sender<Vec<u8>>,
    shared: Arc<Shared>,
    timeout: Duration,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for DapClient {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Una richiesta spedita la cui risposta si attende dopo (per `launch`, che
/// molti adattatori completano solo dopo `configurationDone`).
pub struct PendingResponse {
    command: String,
    seq: i64,
    receiver: oneshot::Receiver<Response>,
    shared: Arc<Shared>,
}

impl PendingResponse {
    pub async fn wait(self, timeout: Duration) -> Result<Response, DapError> {
        let Self {
            command,
            seq,
            receiver,
            shared,
        } = self;
        match tokio::time::timeout(timeout, receiver).await {
            Err(_) => {
                shared.pending().remove(&seq);
                Err(DapError::Timeout {
                    command,
                    seconds: timeout.as_secs(),
                })
            }
            Ok(Err(_)) => Err(DapError::Closed(
                shared
                    .closed_reason()
                    .unwrap_or_else(|| "connessione chiusa".to_owned()),
            )),
            Ok(Ok(response)) if response.success => Ok(response),
            Ok(Ok(response)) => Err(DapError::Rejected {
                command,
                message: response
                    .message
                    .unwrap_or_else(|| "nessun messaggio".to_owned()),
            }),
        }
    }
}

fn encode(value: &Value) -> Result<Vec<u8>, DapError> {
    let body = serde_json::to_vec(value).map_err(|error| DapError::InvalidBody {
        command: "encode".to_owned(),
        detail: error.to_string(),
    })?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(DapError::TooLarge(
            value
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned(),
        ));
    }
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    Ok(frame)
}

fn request_message(seq: i64, command: &str, arguments: Option<Value>) -> Value {
    let mut map = Map::new();
    map.insert("seq".into(), Value::from(seq));
    map.insert("type".into(), Value::from("request"));
    map.insert("command".into(), Value::from(command));
    if let Some(arguments) = arguments {
        map.insert("arguments".into(), arguments);
    }
    Value::Object(map)
}

fn refusal_message(seq: i64, request_seq: i64, command: &str) -> Value {
    let mut map = Map::new();
    map.insert("seq".into(), Value::from(seq));
    map.insert("type".into(), Value::from("response"));
    map.insert("request_seq".into(), Value::from(request_seq));
    map.insert("success".into(), Value::from(false));
    map.insert("command".into(), Value::from(command));
    map.insert(
        "message".into(),
        Value::from("richiesta non supportata da Lume"),
    );
    Value::Object(map)
}

impl DapClient {
    /// Avvia i due task sul canale. `timeout` è l'attesa predefinita di una
    /// risposta.
    pub fn spawn<R, W>(
        reader: R,
        writer: W,
        timeout: Duration,
    ) -> (Self, mpsc::Receiver<AdapterEvent>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outbound, outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
        let (events, events_rx) = mpsc::channel(EVENT_CAPACITY);
        let shared = Arc::new(Shared {
            seq: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            initialized: watch::Sender::new(false),
            closed: watch::Sender::new(None),
        });
        let writer_task = tokio::spawn(write_loop(writer, outbound_rx, Arc::clone(&shared)));
        let reader_task = tokio::spawn(read_loop(
            reader,
            events,
            outbound.clone(),
            Arc::clone(&shared),
        ));
        (
            Self {
                outbound,
                shared,
                timeout,
                tasks: vec![writer_task, reader_task],
            },
            events_rx,
        )
    }

    pub fn default_timeout(&self) -> Duration {
        self.timeout
    }

    pub fn closed_reason(&self) -> Option<String> {
        self.shared.closed_reason()
    }

    /// Spedisce la richiesta senza attenderne la risposta.
    pub async fn send_request(
        &self,
        command: &str,
        arguments: Option<Value>,
    ) -> Result<PendingResponse, DapError> {
        if let Some(reason) = self.shared.closed_reason() {
            return Err(DapError::Closed(reason));
        }
        let seq = self.shared.next_seq();
        let frame = encode(&request_message(seq, command, arguments))?;
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.shared.pending();
            if pending.len() >= PENDING_CAPACITY {
                return Err(DapError::TooManyPending(pending.len()));
            }
            pending.insert(seq, sender);
        }
        if self.outbound.send(frame).await.is_err() {
            self.shared.pending().remove(&seq);
            return Err(DapError::Closed(
                self.shared
                    .closed_reason()
                    .unwrap_or_else(|| "scrittura chiusa".to_owned()),
            ));
        }
        Ok(PendingResponse {
            command: command.to_owned(),
            seq,
            receiver,
            shared: Arc::clone(&self.shared),
        })
    }

    /// Spedisce e attende con il timeout predefinito.
    pub async fn request(
        &self,
        command: &str,
        arguments: Option<Value>,
    ) -> Result<Response, DapError> {
        self.send_request(command, arguments)
            .await?
            .wait(self.timeout)
            .await
    }

    /// Attende l'evento `initialized`, che può arrivare prima o dopo la
    /// risposta a `launch`.
    pub async fn wait_initialized(&self, timeout: Duration) -> Result<(), DapError> {
        let mut initialized = self.shared.initialized.subscribe();
        let mut closed = self.shared.closed.subscribe();
        let attesa = async {
            tokio::select! {
                result = initialized.wait_for(|fatto| *fatto) => result.map(|_| ()).map_err(|_| ()),
                _ = closed.wait_for(Option::is_some) => Err(()),
            }
        };
        match tokio::time::timeout(timeout, attesa).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(())) => Err(DapError::Closed(
                self.shared
                    .closed_reason()
                    .unwrap_or_else(|| "connessione chiusa".to_owned()),
            )),
            Err(_) => Err(DapError::Timeout {
                command: "initialized".to_owned(),
                seconds: timeout.as_secs(),
            }),
        }
    }
}

async fn write_loop<W>(mut writer: W, mut frames: mpsc::Receiver<Vec<u8>>, shared: Arc<Shared>)
where
    W: AsyncWrite + Unpin,
{
    while let Some(frame) = frames.recv().await {
        let scritto = async {
            writer.write_all(&frame).await?;
            writer.flush().await
        };
        if let Err(error) = scritto.await {
            shared.close(&format!("scrittura verso l'adattatore fallita: {error}"));
            return;
        }
    }
}

async fn read_loop<R>(
    mut reader: R,
    events: mpsc::Sender<AdapterEvent>,
    outbound: mpsc::Sender<Vec<u8>>,
    shared: Arc<Shared>,
) where
    R: AsyncRead + Unpin,
{
    let reason = read_frames(&mut reader, &events, &outbound, &shared).await;
    shared.close(&reason);
    // Se chi consuma se n'è andato non c'è nessuno da avvisare.
    let _ = events.send(AdapterEvent::Closed(reason)).await;
}

async fn read_frames<R>(
    reader: &mut R,
    events: &mpsc::Sender<AdapterEvent>,
    outbound: &mpsc::Sender<Vec<u8>>,
    shared: &Shared,
) -> String
where
    R: AsyncRead + Unpin,
{
    let mut decoder = FrameDecoder::default();
    let mut chunk = vec![0_u8; READ_CHUNK];
    loop {
        let letti = match reader.read(&mut chunk).await {
            Ok(0) => return "l'adattatore ha chiuso l'uscita".to_owned(),
            Ok(letti) => letti,
            Err(error) => return format!("lettura dall'adattatore fallita: {error}"),
        };
        let Some(dati) = chunk.get(..letti) else {
            return "lettura oltre il buffer".to_owned();
        };
        let frames = match decoder.push(dati) {
            Ok(frames) => frames,
            Err(error) => return format!("framing DAP non valido: {error}"),
        };
        for frame in frames {
            let message = match protocol::decode(&frame) {
                Ok(message) => message,
                Err(error) => return format!("messaggio DAP non valido: {error}"),
            };
            match message {
                Incoming::Response(response) => {
                    let sender = shared.pending().remove(&response.request_seq);
                    if let Some(sender) = sender {
                        // Chi aspettava può essere andato in timeout.
                        let _ = sender.send(response);
                    }
                }
                Incoming::Event(event) => {
                    if event.event == "initialized" {
                        shared.initialized.send_replace(true);
                    }
                    if events.send(AdapterEvent::Event(event)).await.is_err() {
                        return "il consumatore degli eventi è chiuso".to_owned();
                    }
                }
                Incoming::Request { seq, command } => {
                    let reply = refusal_message(shared.next_seq(), seq, &command);
                    let Ok(frame) = encode(&reply) else {
                        return "risposta di rifiuto non codificabile".to_owned();
                    };
                    if outbound.send(frame).await.is_err() {
                        return "scrittura chiusa".to_owned();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader, DuplexStream};

    async fn read_frame(stream: &mut BufReader<DuplexStream>) -> Value {
        let mut length = 0;
        loop {
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            length = line.split(':').nth(1).unwrap().trim().parse().unwrap();
        }
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn write_frame(stream: &mut BufReader<DuplexStream>, value: Value) {
        let frame = encode(&value).unwrap();
        stream.get_mut().write_all(&frame).await.unwrap();
    }

    fn pair() -> (
        DapClient,
        mpsc::Receiver<AdapterEvent>,
        BufReader<DuplexStream>,
    ) {
        let (client_side, adapter_side) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client_side);
        let (client, events) = DapClient::spawn(reader, writer, Duration::from_secs(2));
        (client, events, BufReader::new(adapter_side))
    }

    #[tokio::test]
    async fn risposta_legata_alla_richiesta_ed_evento_in_ordine() {
        let (client, mut events, mut adapter) = pair();
        let pending = client.send_request("threads", None).await.unwrap();
        let request = read_frame(&mut adapter).await;
        assert_eq!(request["command"], "threads");
        let seq = request["seq"].as_i64().unwrap();
        write_frame(
            &mut adapter,
            serde_json::json!({"seq":1,"type":"event","event":"initialized"}),
        )
        .await;
        write_frame(
            &mut adapter,
            serde_json::json!({"seq":2,"type":"response","request_seq":seq,"success":true,"command":"threads","body":{"threads":[]}}),
        )
        .await;
        let response = pending.wait(Duration::from_secs(2)).await.unwrap();
        assert_eq!(response.command, "threads");
        client
            .wait_initialized(Duration::from_secs(1))
            .await
            .unwrap();
        let AdapterEvent::Event(event) = events.recv().await.unwrap() else {
            panic!("atteso l'evento initialized");
        };
        assert_eq!(event.event, "initialized");
    }

    #[tokio::test]
    async fn risposta_negativa_e_timeout_sono_errori_distinti() {
        let (client, _events, mut adapter) = pair();
        let pending = client.send_request("evaluate", None).await.unwrap();
        let request = read_frame(&mut adapter).await;
        write_frame(
            &mut adapter,
            serde_json::json!({"seq":1,"type":"response","request_seq":request["seq"],"success":false,"command":"evaluate","message":"no"}),
        )
        .await;
        assert_eq!(
            pending.wait(Duration::from_secs(2)).await,
            Err(DapError::Rejected {
                command: "evaluate".into(),
                message: "no".into()
            })
        );
        let pending = client.send_request("next", None).await.unwrap();
        assert!(matches!(
            pending.wait(Duration::from_millis(50)).await,
            Err(DapError::Timeout { .. })
        ));
    }

    #[tokio::test]
    async fn richiesta_inversa_rifiutata_senza_eseguire() {
        let (_client, _events, mut adapter) = pair();
        write_frame(
            &mut adapter,
            serde_json::json!({"seq":9,"type":"request","command":"runInTerminal","arguments":{"args":["sh"]}}),
        )
        .await;
        let reply = read_frame(&mut adapter).await;
        assert_eq!(reply["type"], "response");
        assert_eq!(reply["request_seq"], 9);
        assert_eq!(reply["success"], false);
    }

    #[tokio::test]
    async fn chiusura_sblocca_le_attese_e_arriva_per_ultima() {
        let (client, mut events, adapter) = pair();
        let pending = client.send_request("continue", None).await.unwrap();
        drop(adapter);
        assert!(matches!(
            pending.wait(Duration::from_secs(2)).await,
            Err(DapError::Closed(_))
        ));
        assert!(matches!(events.recv().await, Some(AdapterEvent::Closed(_))));
        assert!(matches!(
            client.request("next", None).await,
            Err(DapError::Closed(_))
        ));
    }
}
