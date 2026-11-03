#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod comune;

use std::time::Duration;

use comune::{avvia, prossimo_evento};
use lume_mcp::proxy::bridge;
use lume_mcp::{Availability, McpEvent, Token};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Client ↔ ponte ↔ listener in-process: stdin e stdout del ponte sono due
/// `duplex`, la pipe è quella vera.
#[tokio::test]
async fn il_ponte_porta_le_righe_nei_due_versi_e_finisce_con_stdin() {
    let mut server = avvia(Availability::default()).await;
    let (mut verso_ponte, stdin_ponte) = tokio::io::duplex(64 * 1024);
    let (stdout_ponte, dal_ponte) = tokio::io::duplex(64 * 1024);
    let (nome, token) = (server.nome.clone(), server.token.clone());
    let ponte = tokio::spawn(async move { bridge(stdin_ponte, stdout_ponte, &nome, &token).await });
    verso_ponte
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"p\",\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"ponte\"}}}\n")
        .await
        .unwrap();
    let mut righe = BufReader::new(dal_ponte).lines();
    let riga = tokio::time::timeout(Duration::from_secs(10), righe.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let risposta: Value = serde_json::from_str(&riga).unwrap();
    assert_eq!(risposta["id"], "p");
    verso_ponte
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    assert!(matches!(
        prossimo_evento(&mut server).await,
        McpEvent::ClientConnected { name, .. } if name == "ponte"
    ));
    drop(verso_ponte);
    let esito = tokio::time::timeout(Duration::from_secs(10), ponte)
        .await
        .unwrap()
        .unwrap();
    assert!(esito.is_ok(), "l'EOF di stdin chiude il ponte: {esito:?}");
}

#[tokio::test]
async fn con_il_token_sbagliato_il_ponte_vede_solo_la_chiusura() {
    let server = avvia(Availability::default()).await;
    let (_verso_ponte, stdin_ponte) = tokio::io::duplex(1024);
    let (stdout_ponte, mut dal_ponte) = tokio::io::duplex(1024);
    let falso = Token::generate().unwrap();
    let esito = tokio::time::timeout(
        Duration::from_secs(10),
        bridge(stdin_ponte, stdout_ponte, &server.nome, &falso),
    )
    .await
    .unwrap();
    assert!(esito.is_ok());
    let mut letto = Vec::new();
    let _ = dal_ponte.read_to_end(&mut letto).await;
    assert!(
        letto.is_empty(),
        "niente dal server a chi ha il token sbagliato"
    );
}
