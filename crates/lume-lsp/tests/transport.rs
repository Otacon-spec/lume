#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::{collections::VecDeque, future::Future, time::Duration};

use lume_lsp::{
    ClientError, ClientTransport, EVENT_CAPACITY, FrameDecoder, OUTBOUND_CAPACITY, QueueKind,
    RequestId, RpcMessage, TransportConfig, TransportEvent, encode_message,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf, duplex, split,
};

const TEST_WAIT: Duration = Duration::from_secs(2);

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(TEST_WAIT, future)
        .await
        .expect("asynchronous test operation timed out")
}

struct PeerReader<R> {
    reader: R,
    decoder: FrameDecoder,
    queued: VecDeque<Vec<u8>>,
}

impl<R: AsyncRead + Unpin> PeerReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            decoder: FrameDecoder::default(),
            queued: VecDeque::new(),
        }
    }

    async fn message(&mut self) -> RpcMessage {
        loop {
            if let Some(body) = self.queued.pop_front() {
                return serde_json::from_slice(&body).expect("transport emitted valid JSON-RPC");
            }

            let mut bytes = [0; 4096];
            let read = self.reader.read(&mut bytes).await.expect("read peer frame");
            assert_ne!(read, 0, "transport closed before emitting a message");
            self.queued.extend(
                self.decoder
                    .push(&bytes[..read])
                    .expect("decode peer frame"),
            );
        }
    }
}

struct Peer {
    reader: PeerReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

impl Peer {
    async fn message(&mut self) -> RpcMessage {
        self.reader.message().await
    }

    async fn send(&mut self, message: RpcMessage) {
        self.writer
            .write_all(&encode_message(&message).expect("encode peer message"))
            .await
            .expect("write peer message");
    }
}

fn spawn_transport(capacity: usize) -> (ClientTransport, lume_lsp::TransportEvents, Peer) {
    spawn_transport_with_config(capacity, TransportConfig::default())
}

fn spawn_transport_with_config(
    capacity: usize,
    config: TransportConfig,
) -> (ClientTransport, lume_lsp::TransportEvents, Peer) {
    let (client_stream, peer_stream) = duplex(capacity);
    let (client_reader, client_writer) = split(client_stream);
    let (peer_reader, peer_writer) = split(peer_stream);
    let (transport, events) = ClientTransport::spawn(client_reader, client_writer, config);
    (
        transport,
        events,
        Peer {
            reader: PeerReader::new(peer_reader),
            writer: peer_writer,
        },
    )
}

fn request_parts(message: RpcMessage) -> (RequestId, String, Value) {
    let RpcMessage::Request(request) = message else {
        panic!("expected request")
    };
    (request.id, request.method, request.params)
}

fn assert_value<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("valid typed value")
}

#[tokio::test]
async fn correlates_out_of_order_responses_with_monotonic_ids() {
    let (transport, _events, mut peer) = spawn_transport(64 * 1024);
    let first_client = transport.clone();
    let first = tokio::spawn(async move {
        first_client
            .request::<_, String>("first", json!({"slot": 1}), Duration::from_secs(1))
            .await
    });
    let second_client = transport.clone();
    let second = tokio::spawn(async move {
        second_client
            .request::<_, String>("second", json!({"slot": 2}), Duration::from_secs(1))
            .await
    });

    let mut requests = [
        request_parts(bounded(peer.message()).await),
        request_parts(bounded(peer.message()).await),
    ];
    requests.sort_by_key(|request| request.0.0);
    assert_eq!(requests[0].0, RequestId(1));
    assert_eq!(requests[1].0, RequestId(2));
    assert_eq!(requests[0].1, "first");
    assert_eq!(requests[1].1, "second");

    peer.send(RpcMessage::success(requests[1].0, json!("two")))
        .await;
    peer.send(RpcMessage::success(requests[0].0, json!("one")))
        .await;

    assert_eq!(bounded(first).await.unwrap().unwrap(), "one");
    assert_eq!(bounded(second).await.unwrap().unwrap(), "two");
    assert_eq!(transport.pending_count(), 0);
}

#[tokio::test]
async fn timeout_removes_pending_request() {
    let (transport, _events, _peer) = spawn_transport(64 * 1024);

    let error = transport
        .request::<_, Value>("slow", json!({}), Duration::from_millis(20))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        ClientError::Timeout {
            id: RequestId(1),
            method: "slow"
        }
    ));
    assert_eq!(transport.pending_count(), 0);
}

#[tokio::test]
async fn explicit_cancellation_cleans_up_and_emits_cancel_notification() {
    let (transport, _events, mut peer) = spawn_transport(64 * 1024);
    let handle = transport
        .request_handle::<_, Value>("slow", json!({}))
        .unwrap();
    let (id, method, _) = request_parts(bounded(peer.message()).await);
    assert_eq!(method, "slow");

    handle.cancel().unwrap();

    let RpcMessage::Notification(cancel) = bounded(peer.message()).await else {
        panic!("expected cancellation notification")
    };
    assert_eq!(cancel.method, "$/cancelRequest");
    assert_eq!(cancel.params, json!({"id": id.0}));
    assert_eq!(transport.pending_count(), 0);
}

#[tokio::test]
async fn dropping_request_handle_cleans_up_and_emits_cancel_notification() {
    let (transport, _events, mut peer) = spawn_transport(64 * 1024);
    let handle = transport
        .request_handle::<_, Value>("abandoned", json!({}))
        .unwrap();
    let (id, method, _) = request_parts(bounded(peer.message()).await);
    assert_eq!(method, "abandoned");

    drop(handle);

    assert_eq!(transport.pending_count(), 0);
    let RpcMessage::Notification(cancel) = bounded(peer.message()).await else {
        panic!("expected cancellation notification")
    };
    assert_eq!(cancel.method, "$/cancelRequest");
    assert_eq!(cancel.params, json!({"id": id.0}));
}

#[tokio::test]
async fn inbound_notification_is_delivered_as_an_event() {
    let (_transport, mut events, mut peer) = spawn_transport(64 * 1024);
    peer.send(RpcMessage::notification(
        "textDocument/publishDiagnostics",
        json!({"uri": "file:///a.rs"}),
    ))
    .await;

    let event = bounded(events.recv()).await.expect("notification event");
    let TransportEvent::Notification(notification) = event else {
        panic!("expected notification event")
    };
    assert_eq!(notification.method, "textDocument/publishDiagnostics");
    assert_eq!(notification.params, json!({"uri": "file:///a.rs"}));
}

#[tokio::test]
async fn outbound_queue_overload_is_typed_and_bounded() {
    let (transport, _events, _peer) = spawn_transport(1);

    let error = (0..=OUTBOUND_CAPACITY)
        .find_map(|sequence| {
            transport
                .notify("queued", json!({"sequence": sequence}))
                .err()
        })
        .expect("bounded queue must reject overload");

    assert_eq!(
        error,
        ClientError::Overloaded {
            queue: QueueKind::Outbound,
            capacity: OUTBOUND_CAPACITY,
        }
    );
}

#[tokio::test]
async fn event_queue_saturation_delivers_typed_terminal_then_ends() {
    let (transport, mut events, mut peer) = spawn_transport(256 * 1024);
    let client = transport.clone();
    let pending = tokio::spawn(async move {
        client
            .request::<_, Value>("pending", json!({}), Duration::from_secs(5))
            .await
    });
    let _request = bounded(peer.message()).await;

    for sequence in 0..=EVENT_CAPACITY {
        bounded(peer.send(RpcMessage::notification(
            "event",
            json!({"sequence": sequence}),
        )))
        .await;
    }

    assert_eq!(
        bounded(pending).await.unwrap().unwrap_err(),
        ClientError::Overloaded {
            queue: QueueKind::Events,
            capacity: EVENT_CAPACITY,
        }
    );
    assert_eq!(transport.pending_count(), 0);

    for sequence in 0..EVENT_CAPACITY {
        let Some(TransportEvent::Notification(notification)) = bounded(events.recv()).await else {
            panic!("expected queued notification")
        };
        assert_eq!(notification.params, json!({"sequence": sequence}));
    }
    assert_eq!(
        bounded(events.recv()).await,
        Some(TransportEvent::Disconnected(ClientError::Overloaded {
            queue: QueueKind::Events,
            capacity: EVENT_CAPACITY,
        }))
    );
    assert_eq!(bounded(events.recv()).await, None);
}

#[tokio::test]
async fn malformed_peer_input_fails_closed() {
    let (transport, mut events, mut peer) = spawn_transport(64 * 1024);
    peer.writer
        .write_all(b"Content-Length: nope\r\n\r\n")
        .await
        .unwrap();

    let event = bounded(events.recv()).await.expect("disconnect event");
    assert!(matches!(
        event,
        TransportEvent::Disconnected(ClientError::Protocol(_))
    ));
    assert!(matches!(
        transport.notify("after-malformed", json!({})),
        Err(ClientError::Protocol(_))
    ));
    assert_eq!(transport.pending_count(), 0);
}

#[tokio::test]
async fn eof_fails_multiple_pending_requests_without_leaks() {
    let (transport, _events, mut peer) = spawn_transport(64 * 1024);
    let mut pending = Vec::new();
    for index in 0..3 {
        let client = transport.clone();
        pending.push(tokio::spawn(async move {
            client
                .request::<_, Value>("wait", json!({"index": index}), Duration::from_secs(5))
                .await
        }));
    }
    for _ in 0..3 {
        let _request = bounded(peer.message()).await;
    }

    drop(peer);

    for request in pending {
        assert!(matches!(
            bounded(request).await.unwrap(),
            Err(ClientError::Disconnected)
        ));
    }
    assert_eq!(transport.pending_count(), 0);
}

#[tokio::test]
async fn server_errors_are_correlated_and_typed() {
    let (transport, _events, mut peer) = spawn_transport(64 * 1024);
    let client = transport.clone();
    let pending = tokio::spawn(async move {
        client
            .request::<_, Value>("missing", json!({}), Duration::from_secs(1))
            .await
    });
    let (id, _, _) = request_parts(bounded(peer.message()).await);
    peer.send(RpcMessage::failure(
        id,
        -32601,
        "missing method",
        Some(json!({"method": "missing"})),
    ))
    .await;

    let error = bounded(pending).await.unwrap().unwrap_err();
    let ClientError::Server {
        code,
        message,
        data,
    } = error
    else {
        panic!("expected typed server error")
    };
    assert_eq!(code, -32601);
    assert_eq!(message, "missing method");
    assert_value::<Value>(data.unwrap());
    assert_eq!(transport.pending_count(), 0);
}

#[tokio::test]
async fn notify_wait_propaga_la_contropressione_e_consegna_quando_il_writer_si_sblocca() {
    let (transport, _events, mut peer) = spawn_transport(32);

    // Blocca il writer a metà del primo frame: da questo momento non può
    // liberare altri slot nella coda outbound finché il peer non legge.
    transport
        .notify("blocker", json!({"first": true}))
        .expect("notifica iniziale accettata");
    let mut first_byte = [0_u8; 1];
    bounded(peer.reader.reader.read_exact(&mut first_byte))
        .await
        .unwrap();
    peer.reader
        .decoder
        .push(&first_byte)
        .expect("prefisso del frame valido");

    let mut overloaded = false;
    for index in 0..=OUTBOUND_CAPACITY {
        match transport.notify("queued", json!({"index": index})) {
            Ok(()) => {}
            Err(ClientError::Overloaded {
                queue: QueueKind::Outbound,
                ..
            }) => {
                overloaded = true;
                break;
            }
            Err(error) => panic!("errore inatteso dal transport: {error}"),
        }
    }
    assert!(overloaded, "la coda outbound di prova deve essere piena");

    let waiting = transport.notify_wait("latest", json!({"revision": 999}));
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err(),
        "la notifica resta in attesa finché il writer non libera uno slot"
    );

    let _first = bounded(peer.message()).await;
    bounded(&mut waiting).await.expect("slot outbound liberato");
    let mut delivered_latest = false;
    for _ in 0..=OUTBOUND_CAPACITY {
        let message = bounded(peer.message()).await;
        if let RpcMessage::Notification(notification) = message
            && notification.method == "latest"
        {
            delivered_latest = true;
            break;
        }
    }
    assert!(delivered_latest, "la notifica in attesa arriva sul filo");
}

#[tokio::test]
async fn notify_wait_limita_i_byte_in_coda_oltre_al_numero_di_messaggi() {
    let message_text = "x".repeat(700);
    let params = json!({"text": message_text});
    let bytes = encode_message(&RpcMessage::notification("large", params.clone()))
        .expect("notifica serializzabile")
        .len();
    let byte_budget = bytes * 2 - 1;
    let (transport, _events, mut peer) = spawn_transport_with_config(
        32,
        TransportConfig {
            read_buffer_bytes: 8 * 1024,
            outbound_bytes_capacity: byte_budget,
        },
    );

    transport
        .notify_wait("large", params.clone())
        .await
        .expect("il primo messaggio rientra nel budget");
    let second = transport.notify_wait("large", params);
    tokio::pin!(second);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut second)
            .await
            .is_err(),
        "il budget byte deve applicarsi anche se la coda ha slot liberi"
    );

    let _first = bounded(peer.message()).await;
    bounded(&mut second)
        .await
        .expect("il writer rilascia il budget dopo la scrittura");
    let second_message = bounded(peer.message()).await;
    assert!(matches!(
        second_message,
        RpcMessage::Notification(notification) if notification.method == "large"
    ));
    assert!(matches!(
        transport
            .notify_wait("large", json!({"text": "y".repeat(3_000)}))
            .await,
        Err(ClientError::MessageTooLarge { .. })
    ));
}
