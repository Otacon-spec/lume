//! Aiuti comuni ai test d'integrazione su named pipe.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use lume_mcp::discovery::preamble;
use lume_mcp::server::{Endpoint, ServeConfig, serve};
use lume_mcp::{Availability, McpCommand, McpEvent, Root, Token};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

static CONTATORE: AtomicU32 = AtomicU32::new(0);

/// I test girano in parallelo: con `first_pipe_instance(true)` due server
/// sullo stesso nome si escluderebbero a vicenda.
pub fn nome_unico() -> String {
    format!(
        r"\\.\pipe\lume-mcp-test-{}-{}",
        std::process::id(),
        CONTATORE.fetch_add(1, Ordering::Relaxed)
    )
}

pub struct Server {
    pub nome: String,
    pub token: Token,
    pub eventi: UnboundedReceiver<McpEvent>,
    pub posta: UnboundedSender<McpCommand>,
    pub radice: tempfile::TempDir,
}

pub fn configurazione(
    nome: &str,
    token: &Token,
    radice: &std::path::Path,
    availability: Availability,
) -> ServeConfig {
    ServeConfig {
        endpoint: Endpoint::Pipe {
            name: nome.to_owned(),
            discovery_dir: None,
        },
        token: token.clone(),
        root: Root::new(radice).unwrap(),
        server_version: "0.0.0-prova".to_owned(),
        availability,
        preamble_timeout: Duration::from_millis(300),
        line_cap: 4096,
        ask_timeout: lume_mcp::server::ASK_TIMEOUT,
    }
}

pub async fn avvia(availability: Availability) -> Server {
    avvia_con_timeout(availability, lume_mcp::server::ASK_TIMEOUT).await
}

pub async fn avvia_con_timeout(availability: Availability, ask_timeout: Duration) -> Server {
    let radice = tempfile::tempdir().unwrap();
    std::fs::write(radice.path().join("a.txt"), "ciao dal disco\n").unwrap();
    let nome = nome_unico();
    let token = Token::generate().unwrap();
    let (tx, mut eventi) = unbounded_channel();
    // Nei test il server gira su una task del runtime di prova; nell'app
    // lo possiede lo stream della subscription.
    let mut config = configurazione(&nome, &token, radice.path(), availability);
    config.ask_timeout = ask_timeout;
    tokio::spawn(serve(config, tx));
    let Some(McpEvent::Ready(posta)) = eventi.recv().await else {
        panic!("il primo evento dev'essere Ready")
    };
    Server {
        nome,
        token,
        eventi,
        posta,
        radice,
    }
}

pub fn tutto_acceso() -> Availability {
    Availability {
        git: true,
        diagnostics: true,
        hover: true,
        definition: true,
        references: true,
        symbols: true,
        rename: true,
    }
}

pub async fn apri(nome: &str) -> NamedPipeClient {
    for _ in 0..200 {
        if let Ok(pipe) = ClientOptions::new().open(nome) {
            return pipe;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("la pipe {nome} non si apre")
}

pub struct Cliente {
    pub r: BufReader<ReadHalf<NamedPipeClient>>,
    pub w: WriteHalf<NamedPipeClient>,
}

impl Cliente {
    pub async fn connetti(server: &Server) -> Self {
        let (r, mut w) = tokio::io::split(apri(&server.nome).await);
        w.write_all(format!("{}\n", preamble(&server.token)).as_bytes())
            .await
            .unwrap();
        Self {
            r: BufReader::new(r),
            w,
        }
    }

    pub async fn manda(&mut self, messaggio: Value) {
        self.w
            .write_all(format!("{messaggio}\n").as_bytes())
            .await
            .unwrap();
        self.w.flush().await.unwrap();
    }

    pub async fn ricevi(&mut self) -> Value {
        let mut riga = String::new();
        let letti = tokio::time::timeout(Duration::from_secs(10), self.r.read_line(&mut riga))
            .await
            .expect("risposta entro 10 s")
            .unwrap();
        assert!(letti > 0, "connessione chiusa invece di una risposta");
        serde_json::from_str(&riga).unwrap()
    }

    pub async fn inizializza(&mut self, nome: &str) -> Value {
        self.manda(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": nome}}
        }))
        .await;
        let risposta = self.ricevi().await;
        self.manda(json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        }))
        .await;
        risposta
    }

    pub async fn chiama(&mut self, id: u64, tool: &str, argomenti: Value) -> Value {
        self.manda(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": tool, "arguments": argomenti}
        }))
        .await;
        self.ricevi().await
    }

    pub async fn manda_chiamata(&mut self, id: u64, tool: &str, argomenti: Value) {
        self.manda(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": argomenti} })).await;
    }
}

/// Il JSON dentro il testo del risultato di un tool.
pub fn contenuto(risposta: &Value) -> Value {
    serde_json::from_str(risposta["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

pub async fn prossimo_evento(server: &mut Server) -> McpEvent {
    tokio::time::timeout(Duration::from_secs(10), server.eventi.recv())
        .await
        .expect("evento entro 10 s")
        .expect("canale aperto")
}

pub async fn prossima_ask(server: &mut Server) -> (u64, u64, lume_mcp::AppQuery) {
    match prossimo_evento(server).await {
        McpEvent::Ask { id, conn, query } => (id, conn, query),
        altro => panic!("attesa una Ask, non {altro:?}"),
    }
}

pub fn testo_errore(risposta: &Value) -> String {
    assert_eq!(risposta["result"]["isError"], true, "{risposta}");
    risposta["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}
