use std::{
    collections::HashMap,
    marker::PhantomData,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
};

use crate::{
    FrameDecoder, RequestId, RpcMessage, RpcNotification, RpcRequest, RpcResponsePayload,
    WorkspaceTrust, encode_message,
};

pub const OUTBOUND_CAPACITY: usize = 256;
pub const OUTBOUND_BYTES_CAPACITY: usize = 32 * 1024 * 1024;
pub const EVENT_CAPACITY: usize = 512;
const DEFAULT_READ_BUFFER_BYTES: usize = 8 * 1024;

type PendingSender = oneshot::Sender<Result<RpcResponsePayload, ClientError>>;
type PendingRequests = Arc<Mutex<HashMap<RequestId, PendingSender>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueKind {
    Outbound,
    Events,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    #[error("{queue:?} queue overloaded at capacity {capacity}")]
    Overloaded { queue: QueueKind, capacity: usize },
    #[error("outbound message size {bytes} exceeds byte budget {capacity}")]
    MessageTooLarge { bytes: usize, capacity: usize },
    #[error("request {id:?} ({method}) timed out")]
    Timeout { id: RequestId, method: &'static str },
    #[error("request {id:?} was cancelled")]
    Cancelled { id: RequestId },
    #[error("transport disconnected")]
    Disconnected,
    #[error("transport protocol error: {0}")]
    Protocol(String),
    #[error("transport I/O error: {0}")]
    Io(String),
    #[error("could not serialize JSON-RPC parameters: {0}")]
    Serialize(String),
    #[error("could not deserialize JSON-RPC result: {0}")]
    Deserialize(String),
    #[error("server error {code}: {message}")]
    Server {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("JSON-RPC request ID space exhausted")]
    RequestIdExhausted,
    #[error("workspace trust is required ({decision:?})")]
    TrustRequired { decision: WorkspaceTrust },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportConfig {
    pub read_buffer_bytes: usize,
    pub outbound_bytes_capacity: usize,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            read_buffer_bytes: DEFAULT_READ_BUFFER_BYTES,
            outbound_bytes_capacity: OUTBOUND_BYTES_CAPACITY,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransportEvent {
    Request(RpcRequest),
    Notification(RpcNotification),
    Disconnected(ClientError),
}

pub struct TransportEvents {
    receiver: mpsc::Receiver<TransportEvent>,
    terminal: watch::Receiver<Option<ClientError>>,
    terminal_delivered: bool,
}

impl TransportEvents {
    pub async fn recv(&mut self) -> Option<TransportEvent> {
        loop {
            match self.try_recv() {
                Ok(event) => return Some(event),
                Err(mpsc::error::TryRecvError::Disconnected) => return None,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }

            tokio::select! {
                biased;
                event = self.receiver.recv() => {
                    return match event {
                        Some(event) => Some(event),
                        None => self.take_terminal_event(),
                    };
                }
                changed = self.terminal.changed() => {
                    if changed.is_err() {
                        return self.take_terminal_event();
                    }
                }
            }
        }
    }

    pub fn try_recv(&mut self) -> Result<TransportEvent, mpsc::error::TryRecvError> {
        match self.receiver.try_recv() {
            Ok(event) => Ok(event),
            Err(error) => match self.take_terminal_event() {
                Some(event) => Ok(event),
                None => Err(error),
            },
        }
    }

    fn take_terminal_event(&mut self) -> Option<TransportEvent> {
        if self.terminal_delivered {
            return None;
        }
        let error = self.terminal.borrow().clone()?;
        self.terminal_delivered = true;
        self.receiver.close();
        Some(TransportEvent::Disconnected(error))
    }
}

/// Osservazione del wire esclusivamente nei test; nessun payload nei log.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct ServerWireAudit {
    pub requests: Vec<crate::RpcRequest>,
    pub responses: Vec<crate::RpcResponse>,
    pub overflow: bool,
}

struct Shared {
    #[cfg(test)]
    server_wire_audit: Mutex<Option<Arc<Mutex<ServerWireAudit>>>>,
    pending: PendingRequests,
    closed: Mutex<Option<ClientError>>,
    next_id: AtomicI64,
    events: mpsc::Sender<TransportEvent>,
    terminal: watch::Sender<Option<ClientError>>,
    shutdown: watch::Sender<bool>,
    outbound_bytes: Arc<Semaphore>,
    outbound_bytes_capacity: usize,
}

struct QueuedRpcMessage {
    message: RpcMessage,
    _byte_permit: Option<OwnedSemaphorePermit>,
}

#[derive(Clone)]
pub struct ClientTransport {
    outbound: mpsc::Sender<QueuedRpcMessage>,
    shared: Arc<Shared>,
}

impl ClientTransport {
    #[cfg(test)]
    pub(crate) fn observe_server_wire_for_test(&self) -> Arc<Mutex<ServerWireAudit>> {
        let audit = Arc::new(Mutex::new(ServerWireAudit::default()));
        *lock(&self.shared.server_wire_audit) = Some(Arc::clone(&audit));
        audit
    }

    #[cfg(test)]
    pub(crate) async fn reserve_all_outbound_bytes_for_test(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError> {
        Arc::clone(&self.shared.outbound_bytes)
            .acquire_many_owned(self.shared.outbound_bytes_capacity as u32)
            .await
    }

    pub fn spawn<R, W>(reader: R, writer: W, config: TransportConfig) -> (Self, TransportEvents)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let outbound_bytes_capacity = config
            .outbound_bytes_capacity
            .clamp(1, Semaphore::MAX_PERMITS);
        let (outbound, outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
        let (events, event_rx) = mpsc::channel(EVENT_CAPACITY);
        let (terminal, terminal_rx) = watch::channel(None);
        let (shutdown, shutdown_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            #[cfg(test)]
            server_wire_audit: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            closed: Mutex::new(None),
            next_id: AtomicI64::new(0),
            events,
            terminal,
            shutdown,
            outbound_bytes: Arc::new(Semaphore::new(outbound_bytes_capacity)),
            outbound_bytes_capacity,
        });

        tokio::spawn(reader_task(
            reader,
            config.read_buffer_bytes.max(1),
            Arc::clone(&shared),
            shutdown_rx.clone(),
        ));
        tokio::spawn(writer_task(
            writer,
            outbound_rx,
            Arc::clone(&shared),
            shutdown_rx,
        ));

        (
            Self { outbound, shared },
            TransportEvents {
                receiver: event_rx,
                terminal: terminal_rx,
                terminal_delivered: false,
            },
        )
    }

    pub async fn request<P, R>(
        &self,
        method: &'static str,
        params: P,
        timeout: Duration,
    ) -> Result<R, ClientError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.request_handle(method, params)?.response(timeout).await
    }

    pub fn request_handle<P, R>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<RequestHandle<R>, ClientError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let params = serde_json::to_value(params)
            .map_err(|error| ClientError::Serialize(error.to_string()))?;
        let id = self.next_request_id()?;
        let (responder, receiver) = oneshot::channel();

        {
            let closed = lock(&self.shared.closed);
            if let Some(error) = closed.as_ref() {
                return Err(error.clone());
            }
            lock(&self.shared.pending).insert(id, responder);
        }

        if let Err(error) = send_outbound(
            &self.outbound,
            &self.shared,
            RpcMessage::request(id, method, params),
        ) {
            lock(&self.shared.pending).remove(&id);
            return Err(error);
        }

        Ok(RequestHandle {
            id,
            method,
            receiver: Some(receiver),
            outbound: self.outbound.clone(),
            shared: Arc::clone(&self.shared),
            completed: false,
            result: PhantomData,
        })
    }

    pub fn notify<P>(&self, method: &'static str, params: P) -> Result<(), ClientError>
    where
        P: Serialize,
    {
        let params = serde_json::to_value(params)
            .map_err(|error| ClientError::Serialize(error.to_string()))?;
        send_outbound(
            &self.outbound,
            &self.shared,
            RpcMessage::notification(method, params),
        )
    }

    /// Accoda una notifica aspettando spazio nel writer limitato.
    ///
    /// Da usare per aggiornamenti di documento: rifiutarli su `Full` farebbe
    /// perdere la revisione, mentre attendere propaga la pressione fino alla
    /// mailbox dell'attore che può coalescere i cambi ancora pendenti.
    pub async fn notify_wait<P>(&self, method: &'static str, params: P) -> Result<(), ClientError>
    where
        P: Serialize,
    {
        let params = serde_json::to_value(params)
            .map_err(|error| ClientError::Serialize(error.to_string()))?;
        let message = RpcMessage::notification(method, params);
        let bytes = encode_message(&message)
            .map_err(|error| ClientError::Protocol(error.to_string()))?
            .len();
        if bytes > self.shared.outbound_bytes_capacity {
            return Err(ClientError::MessageTooLarge {
                bytes,
                capacity: self.shared.outbound_bytes_capacity,
            });
        }
        let permits = u32::try_from(bytes).map_err(|_| ClientError::MessageTooLarge {
            bytes,
            capacity: self.shared.outbound_bytes_capacity,
        })?;
        let acquire = Arc::clone(&self.shared.outbound_bytes).acquire_many_owned(permits);
        let permit = tokio::select! {
            biased;
            result = acquire => result.map_err(|_| {
                current_error(&self.shared).unwrap_or(ClientError::Disconnected)
            })?,
            () = self.outbound.closed() => {
                return Err(current_error(&self.shared).unwrap_or(ClientError::Disconnected));
            }
        };
        send_outbound_wait(&self.outbound, &self.shared, message, permit).await
    }

    /// Risponde a una richiesta avviata dal server usando la coda outbound.
    pub fn respond(&self, id: RequestId, result: serde_json::Value) -> Result<(), ClientError> {
        send_outbound(
            &self.outbound,
            &self.shared,
            RpcMessage::success(id, result),
        )
    }

    pub fn respond_error(
        &self,
        id: RequestId,
        code: i64,
        message: impl Into<String>,
    ) -> Result<(), ClientError> {
        send_outbound(
            &self.outbound,
            &self.shared,
            RpcMessage::failure(id, code, message, None),
        )
    }

    pub fn pending_count(&self) -> usize {
        lock(&self.shared.pending).len()
    }

    fn next_request_id(&self) -> Result<RequestId, ClientError> {
        self.shared
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map(|previous| RequestId(previous + 1))
            .map_err(|_| ClientError::RequestIdExhausted)
    }
}

pub struct RequestHandle<R> {
    id: RequestId,
    method: &'static str,
    receiver: Option<oneshot::Receiver<Result<RpcResponsePayload, ClientError>>>,
    outbound: mpsc::Sender<QueuedRpcMessage>,
    shared: Arc<Shared>,
    completed: bool,
    result: PhantomData<fn() -> R>,
}

impl<R> RequestHandle<R>
where
    R: DeserializeOwned,
{
    pub fn id(&self) -> RequestId {
        self.id
    }

    #[allow(clippy::expect_used)] // receiver consumato una sola volta (self per valore)
    pub async fn response(mut self, timeout: Duration) -> Result<R, ClientError> {
        let receiver = self
            .receiver
            .take()
            .expect("request response receiver is available exactly once");
        let result = tokio::time::timeout(timeout, receiver).await;

        match result {
            Ok(Ok(Ok(payload))) => {
                self.completed = true;
                decode_response(payload)
            }
            Ok(Ok(Err(error))) => {
                self.completed = true;
                Err(error)
            }
            Ok(Err(_)) => {
                self.completed = true;
                Err(current_error(&self.shared).unwrap_or(ClientError::Disconnected))
            }
            Err(_) => {
                lock(&self.shared.pending).remove(&self.id);
                self.completed = true;
                Err(ClientError::Timeout {
                    id: self.id,
                    method: self.method,
                })
            }
        }
    }

    pub fn cancel(mut self) -> Result<(), ClientError> {
        let was_pending = lock(&self.shared.pending).remove(&self.id).is_some();
        self.completed = true;

        if !was_pending {
            return match current_error(&self.shared) {
                Some(error) => Err(error),
                None => Ok(()),
            };
        }

        send_outbound(
            &self.outbound,
            &self.shared,
            RpcMessage::notification("$/cancelRequest", serde_json::json!({ "id": self.id.0 })),
        )
    }
}

impl<R> Drop for RequestHandle<R> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }

        let was_pending = lock(&self.shared.pending).remove(&self.id).is_some();
        if was_pending {
            let _ = send_outbound(
                &self.outbound,
                &self.shared,
                RpcMessage::notification("$/cancelRequest", serde_json::json!({ "id": self.id.0 })),
            );
        }
    }
}

fn decode_response<R>(payload: RpcResponsePayload) -> Result<R, ClientError>
where
    R: DeserializeOwned,
{
    match payload {
        RpcResponsePayload::Result(value) => serde_json::from_value(value)
            .map_err(|error| ClientError::Deserialize(error.to_string())),
        RpcResponsePayload::Error(error) => Err(ClientError::Server {
            code: error.code,
            message: error.message,
            data: error.data,
        }),
    }
}

fn send_outbound(
    outbound: &mpsc::Sender<QueuedRpcMessage>,
    shared: &Shared,
    message: RpcMessage,
) -> Result<(), ClientError> {
    if let Some(error) = current_error(shared) {
        return Err(error);
    }

    match outbound.try_send(QueuedRpcMessage {
        message,
        _byte_permit: None,
    }) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(_)) => Err(ClientError::Overloaded {
            queue: QueueKind::Outbound,
            capacity: OUTBOUND_CAPACITY,
        }),
        Err(mpsc::error::TrySendError::Closed(_)) => {
            Err(current_error(shared).unwrap_or(ClientError::Disconnected))
        }
    }
}

async fn send_outbound_wait(
    outbound: &mpsc::Sender<QueuedRpcMessage>,
    shared: &Shared,
    message: RpcMessage,
    permit: OwnedSemaphorePermit,
) -> Result<(), ClientError> {
    if let Some(error) = current_error(shared) {
        return Err(error);
    }
    outbound
        .send(QueuedRpcMessage {
            message,
            _byte_permit: Some(permit),
        })
        .await
        .map_err(|_| current_error(shared).unwrap_or(ClientError::Disconnected))
}

#[allow(clippy::indexing_slicing)] // read <= buffer.len() (contratto di AsyncRead)
async fn reader_task<R>(
    mut reader: R,
    read_buffer_bytes: usize,
    shared: Arc<Shared>,
    mut shutdown: watch::Receiver<bool>,
) where
    R: AsyncRead + Unpin,
{
    let mut decoder = FrameDecoder::default();
    let mut buffer = vec![0; read_buffer_bytes];

    loop {
        let read = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
            result = reader.read(&mut buffer) => result,
        };

        let read = match read {
            Ok(0) => {
                disconnect(&shared, ClientError::Disconnected);
                return;
            }
            Ok(read) => read,
            Err(error) => {
                disconnect(&shared, ClientError::Io(error.to_string()));
                return;
            }
        };

        let frames = match decoder.push(&buffer[..read]) {
            Ok(frames) => frames,
            Err(error) => {
                disconnect(&shared, ClientError::Protocol(error.to_string()));
                return;
            }
        };

        for frame in frames {
            let message = match serde_json::from_slice::<RpcMessage>(&frame) {
                Ok(message) => message,
                Err(error) => {
                    disconnect(&shared, ClientError::Protocol(error.to_string()));
                    return;
                }
            };

            match message {
                RpcMessage::Response(response) => {
                    let responder = lock(&shared.pending).remove(&response.id);
                    if let Some(responder) = responder {
                        let _ = responder.send(Ok(response.payload));
                    }
                }
                RpcMessage::Request(request) => {
                    #[cfg(test)]
                    if let Some(audit) = lock(&shared.server_wire_audit).as_ref() {
                        let mut audit = lock(audit);
                        if audit.requests.len() < 256 {
                            audit.requests.push(request.clone());
                        } else {
                            audit.overflow = true;
                        }
                    }
                    if !send_event(&shared, TransportEvent::Request(request)) {
                        return;
                    }
                }
                RpcMessage::Notification(notification) => {
                    if !send_event(&shared, TransportEvent::Notification(notification)) {
                        return;
                    }
                }
            }
        }
    }
}

async fn writer_task<W>(
    mut writer: W,
    mut outbound: mpsc::Receiver<QueuedRpcMessage>,
    shared: Arc<Shared>,
    mut shutdown: watch::Receiver<bool>,
) where
    W: AsyncWrite + Unpin,
{
    loop {
        let message = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
            message = outbound.recv() => message,
        };

        let Some(message) = message else {
            disconnect(&shared, ClientError::Disconnected);
            return;
        };
        let bytes = match encode_message(&message.message) {
            Ok(bytes) => bytes,
            Err(error) => {
                disconnect(&shared, ClientError::Protocol(error.to_string()));
                return;
            }
        };
        if let Err(error) = writer.write_all(&bytes).await {
            disconnect(&shared, ClientError::Io(error.to_string()));
            return;
        }
        // La sola enqueue non costituisce evidenza di scrittura nel pipe.
        #[cfg(test)]
        if let RpcMessage::Response(response) = &message.message
            && let Some(audit) = lock(&shared.server_wire_audit).as_ref()
        {
            let mut audit = lock(audit);
            if audit.responses.len() < 256 {
                audit.responses.push(response.clone());
            } else {
                audit.overflow = true;
            }
        }
    }
}

fn send_event(shared: &Shared, event: TransportEvent) -> bool {
    match shared.events.try_send(event) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            disconnect(
                shared,
                ClientError::Overloaded {
                    queue: QueueKind::Events,
                    capacity: EVENT_CAPACITY,
                },
            );
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            disconnect(shared, ClientError::Disconnected);
            false
        }
    }
}

fn disconnect(shared: &Shared, error: ClientError) {
    {
        let mut closed = lock(&shared.closed);
        if closed.is_some() {
            return;
        }
        *closed = Some(error.clone());
    }

    let pending = std::mem::take(&mut *lock(&shared.pending));
    for (_, responder) in pending {
        let _ = responder.send(Err(error.clone()));
    }
    shared.terminal.send_replace(Some(error));
    let _ = shared.shutdown.send(true);
}

fn current_error(shared: &Shared) -> Option<ClientError> {
    lock(&shared.closed).clone()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
