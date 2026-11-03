#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use lume_lsp::{FrameDecoder, ProtocolError, RequestId, RpcMessage, encode_message};
use serde_json::json;

fn framed(payload: &[u8]) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n", payload.len())
        .into_bytes()
        .into_iter()
        .chain(payload.iter().copied())
        .collect()
}

#[test]
fn fragmented_multibyte_frame_decodes_once() {
    let payload = r#"{"jsonrpc":"2.0","method":"x","params":{"s":"日本"}}"#.as_bytes();
    let wire = framed(payload);
    let mut decoder = FrameDecoder::default();
    assert!(decoder.push(&wire[..17]).unwrap().is_empty());
    assert_eq!(decoder.push(&wire[17..]).unwrap(), vec![payload.to_vec()]);
}

#[test]
fn split_inside_utf8_body_waits_for_complete_frame() {
    let payload = r#"{"jsonrpc":"2.0","method":"x","params":{"s":"日本"}}"#.as_bytes();
    let wire = framed(payload);
    let body_start = wire
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 4;
    let multibyte_start = payload
        .windows("日".len())
        .position(|window| window == "日".as_bytes())
        .unwrap();
    let split = body_start + multibyte_start + 1;

    let mut decoder = FrameDecoder::default();
    assert!(decoder.push(&wire[..split]).unwrap().is_empty());
    assert_eq!(
        decoder.push(&wire[split..]).unwrap(),
        vec![payload.to_vec()]
    );
}

#[test]
fn multiple_frames_and_case_insensitive_length_decode() {
    let one = br#"{"jsonrpc":"2.0","method":"one"}"#;
    let two = br#"{"jsonrpc":"2.0","method":"two"}"#;
    let mut wire = format!("content-length: {}\r\nX-Test: yes\r\n\r\n", one.len()).into_bytes();
    wire.extend_from_slice(one);
    wire.extend_from_slice(&framed(two));

    assert_eq!(
        FrameDecoder::default().push(&wire).unwrap(),
        vec![one.to_vec(), two.to_vec()]
    );
}

#[test]
fn malformed_and_unbounded_frames_fail_closed() {
    let duplicate = b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}";
    assert!(matches!(
        FrameDecoder::default().push(duplicate),
        Err(ProtocolError::DuplicateContentLength)
    ));
    assert!(matches!(
        FrameDecoder::default().push(b"X: y\r\n\r\n{}"),
        Err(ProtocolError::MissingContentLength)
    ));
    assert!(matches!(
        FrameDecoder::default().push(b"Content-Length: nope\r\n\r\n"),
        Err(ProtocolError::InvalidContentLength)
    ));
    assert!(matches!(
        FrameDecoder::default().push(b"Content-Length: 16777217\r\n\r\n"),
        Err(ProtocolError::BodyTooLarge { .. })
    ));
    assert!(matches!(
        FrameDecoder::default().push(&vec![b'x'; 8193]),
        Err(ProtocolError::HeaderTooLarge { .. })
    ));
}

#[test]
fn rpc_roundtrip_preserves_ids_results_and_errors() {
    let messages = [
        RpcMessage::request(7, "initialize", json!({"rootUri": null})),
        RpcMessage::notification("initialized", json!({})),
        RpcMessage::success(7, json!({"capabilities": {}})),
        RpcMessage::failure(9, -32601, "missing", Some(json!({"method": "x"}))),
    ];

    for message in messages {
        let wire = encode_message(&message).unwrap();
        let body = FrameDecoder::default().push(&wire).unwrap().pop().unwrap();
        assert_eq!(
            serde_json::from_slice::<RpcMessage>(&body).unwrap(),
            message
        );
    }
}

#[test]
fn malformed_json_is_rejected_by_typed_decoder() {
    let body = FrameDecoder::default()
        .push(&framed(b"{"))
        .unwrap()
        .pop()
        .unwrap();
    assert!(serde_json::from_slice::<RpcMessage>(&body).is_err());
}

#[test]
fn request_ids_are_transparent_i64_newtypes() {
    assert_eq!(serde_json::to_value(RequestId(42)).unwrap(), json!(42));
    assert!(serde_json::from_value::<RequestId>(json!("42")).is_err());
}

#[test]
fn null_result_remains_present_after_roundtrip() {
    let wire = json!({"jsonrpc": "2.0", "id": 1, "result": null});
    let message = serde_json::from_value::<RpcMessage>(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), wire);
}

#[test]
fn response_with_result_and_error_is_rejected() {
    let wire = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": null,
        "error": {"code": -32603, "message": "boom"},
    });
    assert!(
        serde_json::from_value::<RpcMessage>(wire).is_err(),
        "a response must not contain both result and error"
    );
}

#[test]
fn response_without_result_or_error_is_rejected() {
    let wire = json!({"jsonrpc": "2.0", "id": 1});
    assert!(serde_json::from_value::<RpcMessage>(wire).is_err());
}

#[test]
fn method_messages_with_invalid_ids_are_not_reclassified_as_notifications() {
    for id in [json!("server-id"), json!(null), json!(1.5)] {
        let wire = json!({"jsonrpc": "2.0", "id": id, "method": "workspace/configuration"});
        assert!(
            serde_json::from_value::<RpcMessage>(wire).is_err(),
            "method message with a non-i64 id was accepted"
        );
    }
}

#[test]
fn hybrid_request_response_shapes_are_rejected() {
    let hybrids = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "x", "result": null}),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "x",
            "error": {"code": -32603, "message": "boom"},
        }),
        json!({"jsonrpc": "2.0", "method": "x", "result": 1}),
    ];
    for message in hybrids {
        assert!(
            serde_json::from_value::<RpcMessage>(message).is_err(),
            "hybrid message was accepted"
        );
    }
}

#[test]
fn unknown_envelope_fields_are_rejected() {
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "x",
            "unexpected": true,
        }),
        json!({
            "jsonrpc": "2.0",
            "method": "x",
            "unexpected": true,
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": 1,
            "unexpected": true,
        }),
    ];
    for message in messages {
        assert!(
            serde_json::from_value::<RpcMessage>(message).is_err(),
            "message with an unknown envelope field was accepted"
        );
    }
}
