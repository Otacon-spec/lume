#![cfg(all(unix, feature = "mcp"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! Il ponte vero `lume --mcp-stdio` contro un server MCP su socket Unix
//! reale. Il processo figlio è il binario costruito da Cargo; la scoperta è
//! nella `LUME_DATA_DIR` isolata del test. Nessun mock: initialize,
//! tools/list, `read_file` e `open_in_editor` attraversano il protocollo
//! JSON-RPC e la coda delle domande dell'app.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use lume_mcp::server::{Endpoint, ServeConfig, serve};
use lume_mcp::unix::{self, Identity};
use lume_mcp::{AppAnswer, AppQuery, Availability, McpCommand, McpEvent, Root, Token};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

fn cartella_privata(prefix: &str) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME per una cartella privata");
    tempfile::Builder::new()
        .prefix(prefix)
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(home)
        .expect("cartella privata di prova")
}

struct Server {
    task: JoinHandle<()>,
    endpoint: unix::Endpoint,
    eventi: UnboundedReceiver<McpEvent>,
    posta: UnboundedSender<McpCommand>,
}

async fn avvia_server(dati: &Path, progetto: &Path) -> Server {
    let endpoint = unix::Endpoint::new(dati).unwrap();
    let token = Token::generate().unwrap();
    let (tx, mut eventi) = unbounded_channel();
    let config = ServeConfig {
        endpoint: Endpoint::Unix(endpoint.clone()),
        token,
        root: Root::new(progetto).unwrap(),
        server_version: "0.0.0-prova".to_owned(),
        availability: Availability::default(),
        preamble_timeout: Duration::from_secs(5),
        line_cap: lume_mcp::framing::LINE_CAP,
        ask_timeout: lume_mcp::server::ASK_TIMEOUT,
    };
    let task = tokio::spawn(serve(config, tx));
    let posta = match tokio::time::timeout(Duration::from_secs(10), eventi.recv())
        .await
        .expect("Ready entro 10 s")
    {
        Some(McpEvent::Ready(posta)) => posta,
        altro => panic!("il primo evento dev'essere Ready, non {altro:?}"),
    };
    Server {
        task,
        endpoint,
        eventi,
        posta,
    }
}

struct Ponte {
    figlio: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Ponte {
    fn avvia(dati: &Path, argomenti: &[&str]) -> Self {
        let mut figlio = Command::new(env!("CARGO_BIN_EXE_lume"))
            .arg("--mcp-stdio")
            .args(argomenti)
            .env("LUME_DATA_DIR", dati)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("il binario lume parte");
        let stdin = figlio.stdin.take().expect("stdin del ponte");
        let stdout = BufReader::new(figlio.stdout.take().expect("stdout del ponte"));
        Self {
            figlio,
            stdin,
            stdout,
        }
    }

    async fn manda(&mut self, messaggio: Value) {
        self.stdin
            .write_all(format!("{messaggio}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn ricevi(&mut self) -> Value {
        let mut riga = String::new();
        let letti = tokio::time::timeout(Duration::from_secs(20), self.stdout.read_line(&mut riga))
            .await
            .expect("risposta del ponte entro 20 s")
            .unwrap();
        assert!(letti > 0, "il ponte ha chiuso invece di rispondere");
        serde_json::from_str(&riga).unwrap()
    }

    async fn inizializza(&mut self, nome: &str) -> Value {
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

    async fn chiudi_e_attendi(mut self) -> (Option<i32>, String) {
        drop(self.stdin);
        let esito = tokio::time::timeout(Duration::from_secs(20), self.figlio.wait())
            .await
            .expect("il ponte esce dopo EOF di stdin")
            .expect("wait del ponte");
        let mut stderr = String::new();
        if let Some(mut canale) = self.figlio.stderr.take() {
            use tokio::io::AsyncReadExt as _;
            let _ = canale.read_to_string(&mut stderr).await;
        }
        (esito.code(), stderr)
    }

    /// Attende l'uscita del ponte **tenendo stdin aperto**: il ponte esce da
    /// sé quando il server rifiuta, senza che il client chiuda.
    async fn attendi_uscita(mut self) -> (Option<i32>, String) {
        let esito = tokio::time::timeout(Duration::from_secs(20), self.figlio.wait())
            .await
            .expect("il ponte esce da sé")
            .expect("wait del ponte");
        let mut stderr = String::new();
        if let Some(mut canale) = self.figlio.stderr.take() {
            use tokio::io::AsyncReadExt as _;
            let _ = canale.read_to_string(&mut stderr).await;
        }
        drop(self.stdin);
        (esito.code(), stderr)
    }
}

async fn prossimo_evento(server: &mut Server) -> McpEvent {
    tokio::time::timeout(Duration::from_secs(10), server.eventi.recv())
        .await
        .expect("evento entro 10 s")
        .expect("canale aperto")
}

async fn prossima_ask(server: &mut Server) -> (u64, u64, AppQuery) {
    match prossimo_evento(server).await {
        McpEvent::Ask { id, conn, query } => (id, conn, query),
        altro => panic!("attesa una Ask, non {altro:?}"),
    }
}

fn rispondi(server: &Server, id: u64, answer: AppAnswer) {
    server
        .posta
        .send(McpCommand::Answer {
            id,
            answer: Ok(answer),
        })
        .unwrap();
}

fn contenuto(risposta: &Value) -> Value {
    serde_json::from_str(risposta["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn progetto(radice: &Path) {
    std::fs::write(radice.join("a.txt"), "dal progetto reale\n").unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn il_ponte_reale_inizializza_e_usa_i_tool_sul_socket() {
    let casa = cartella_privata("lume-mcp-ponte-");
    let dati = casa.path().join("dati");
    std::fs::create_dir(&dati).unwrap();
    std::fs::set_permissions(&dati, PermissionsExt::from_mode(0o700)).unwrap();
    let radice = casa.path().join("progetto");
    std::fs::create_dir(&radice).unwrap();
    progetto(&radice);

    let mut server = avvia_server(&dati, &radice).await;
    let mut ponte = Ponte::avvia(&dati, &[]);
    let risposta = ponte.inizializza("ponte-reale").await;
    assert_eq!(risposta["id"], 1);
    assert_eq!(risposta["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        risposta["result"]["serverInfo"]["project_root"],
        radice.canonicalize().unwrap().display().to_string()
    );
    match prossimo_evento(&mut server).await {
        McpEvent::ClientConnected { name, .. } => assert_eq!(name, "ponte-reale"),
        altro => panic!("atteso ClientConnected, non {altro:?}"),
    }

    ponte
        .manda(json!({"jsonrpc": "2.0", "id": "lista", "method": "tools/list"}))
        .await;
    let lista = ponte.ricevi().await;
    assert!(
        lista["result"]["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()),
        "{lista}"
    );

    ponte
        .manda(json!({
            "jsonrpc": "2.0", "id": "leggi", "method": "tools/call",
            "params": {"name": "read_file", "arguments": {"path": "a.txt"}}
        }))
        .await;
    let (id, _, query) = prossima_ask(&mut server).await;
    assert_eq!(
        query,
        AppQuery::Buffer {
            relative: PathBuf::from("a.txt")
        }
    );
    rispondi(&server, id, AppAnswer::Buffer(None));
    let lettura = ponte.ricevi().await;
    assert_eq!(contenuto(&lettura)["text"], "dal progetto reale\n");

    ponte
        .manda(json!({
            "jsonrpc": "2.0", "id": "apri", "method": "tools/call",
            "params": {"name": "open_in_editor", "arguments": {"path": "a.txt"}}
        }))
        .await;
    let (id, _, query) = prossima_ask(&mut server).await;
    assert_eq!(
        query,
        AppQuery::OpenInEditor {
            relative: PathBuf::from("a.txt"),
            line: None
        }
    );
    rispondi(
        &server,
        id,
        AppAnswer::Json(json!({"path": "a.txt", "opened": true})),
    );
    let apertura = ponte.ricevi().await;
    assert_eq!(contenuto(&apertura)["opened"], true);

    let (codice, stderr) = ponte.chiudi_e_attendi().await;
    assert_eq!(codice, Some(0), "stderr: {stderr}");
    server.task.abort();
    let _ = server.task.await;
    assert!(!server.endpoint.record_path().exists());
    assert!(!server.endpoint.socket_path().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn due_istanze_rifiutano_la_scelta_implicita_e_accettano_instance() {
    let casa = cartella_privata("lume-mcp-due-");
    let dati = casa.path().join("dati");
    std::fs::create_dir(&dati).unwrap();
    std::fs::set_permissions(&dati, PermissionsExt::from_mode(0o700)).unwrap();
    let radice = casa.path().join("progetto");
    std::fs::create_dir(&radice).unwrap();
    progetto(&radice);

    let uno = avvia_server(&dati, &radice).await;
    let due = avvia_server(&dati, &radice).await;
    let identita = uno.endpoint.identity().to_string();
    assert_ne!(identita, due.endpoint.identity().to_string());

    let ambiguo = Ponte::avvia(&dati, &[]);
    let (codice, stderr) = ambiguo.chiudi_e_attendi().await;
    assert_eq!(codice, Some(69), "stderr: {stderr}");
    assert!(
        stderr.contains("più istanze") && stderr.contains("--instance"),
        "{stderr}"
    );

    let mut scelto = Ponte::avvia(&dati, &["--instance", &identita]);
    let risposta = scelto.inizializza("istanza-esplicita").await;
    assert_eq!(risposta["id"], 1);
    let (codice, stderr) = scelto.chiudi_e_attendi().await;
    assert_eq!(codice, Some(0), "stderr: {stderr}");

    uno.task.abort();
    due.task.abort();
    let _ = uno.task.await;
    let _ = due.task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_record_senza_socket_viene_potato_dal_ponte() {
    let casa = cartella_privata("lume-mcp-stantio-");
    let dati = casa.path().join("dati");
    std::fs::create_dir(&dati).unwrap();
    std::fs::set_permissions(&dati, PermissionsExt::from_mode(0o700)).unwrap();
    let radice = casa.path().join("progetto");
    std::fs::create_dir(&radice).unwrap();
    progetto(&radice);

    let dir = unix::discovery_dir(&dati);
    unix::ensure_private_dir(&dir).unwrap();
    let endpoint =
        unix::Endpoint::with_identity(dir.clone(), Identity::generate().unwrap()).unwrap();
    let record = unix::Record::new(
        &endpoint,
        radice.clone(),
        Token::generate().unwrap(),
        "0.0.0-prova".to_owned(),
        &std::fs::symlink_metadata(&radice).unwrap(),
    );
    let guard = unix::DiscoveryFile::publish(&endpoint, &record).unwrap();
    std::mem::forget(guard);

    let ponte = Ponte::avvia(&dati, &[]);
    let (codice, stderr) = ponte.chiudi_e_attendi().await;
    assert_eq!(codice, Some(69), "stderr: {stderr}");
    assert!(stderr.contains("mcp-unix-v1"), "{stderr}");
    assert!(stderr.contains("rimedio"), "{stderr}");
    assert!(
        !endpoint.record_path().exists() && !endpoint.socket_path().exists(),
        "il ponte pota il record senza socket con il ricontrollo"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_record_con_permessi_larghi_e_segnalato_e_saltato_dal_ponte() {
    let casa = cartella_privata("lume-mcp-permessi-");
    let dati = casa.path().join("dati");
    std::fs::create_dir(&dati).unwrap();
    std::fs::set_permissions(&dati, PermissionsExt::from_mode(0o700)).unwrap();
    let radice = casa.path().join("progetto");
    std::fs::create_dir(&radice).unwrap();
    progetto(&radice);

    let dir = unix::discovery_dir(&dati);
    unix::ensure_private_dir(&dir).unwrap();
    let endpoint =
        unix::Endpoint::with_identity(dir.clone(), Identity::generate().unwrap()).unwrap();
    let record = unix::Record::new(
        &endpoint,
        radice.clone(),
        Token::generate().unwrap(),
        "0.0.0-prova".to_owned(),
        &std::fs::symlink_metadata(&radice).unwrap(),
    );
    let guard = unix::DiscoveryFile::publish(&endpoint, &record).unwrap();
    std::mem::forget(guard);
    std::fs::set_permissions(endpoint.record_path(), PermissionsExt::from_mode(0o644)).unwrap();

    let ponte = Ponte::avvia(&dati, &[]);
    let (codice, stderr) = ponte.chiudi_e_attendi().await;
    assert_eq!(codice, Some(69), "stderr: {stderr}");
    assert!(stderr.contains("0644"), "{stderr}");
    assert!(stderr.contains("mcp-unix-v1"), "{stderr}");
    assert!(stderr.contains("rimedio"), "{stderr}");
    assert!(
        endpoint.record_path().exists(),
        "un record illeggibile non viene cancellato"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn il_ponte_segnala_un_record_corrotto_e_usa_l_istanza_sana() {
    let casa = cartella_privata("lume-mcp-corrotto-");
    let dati = casa.path().join("dati");
    std::fs::create_dir(&dati).unwrap();
    std::fs::set_permissions(&dati, PermissionsExt::from_mode(0o700)).unwrap();
    let radice = casa.path().join("progetto");
    std::fs::create_dir(&radice).unwrap();
    progetto(&radice);

    let mut server = avvia_server(&dati, &radice).await;
    let dir = unix::discovery_dir(&dati);
    let corrotto = dir.join(format!("{}-00000000.json", std::process::id()));
    std::fs::write(&corrotto, b"{non-json").unwrap();
    std::fs::set_permissions(&corrotto, PermissionsExt::from_mode(0o600)).unwrap();

    let mut ponte = Ponte::avvia(&dati, &[]);
    let risposta = ponte.inizializza("con-record-corrotto").await;
    assert_eq!(risposta["id"], 1);
    assert!(matches!(
        prossimo_evento(&mut server).await,
        McpEvent::ClientConnected { .. }
    ));
    let (codice, stderr) = ponte.chiudi_e_attendi().await;
    assert_eq!(codice, Some(0), "stderr: {stderr}");
    assert!(
        stderr.contains(&corrotto.display().to_string()),
        "il ponte nomina il file corrotto: {stderr}"
    );
    assert!(stderr.contains("record di scoperta ignorato"), "{stderr}");
    assert!(corrotto.exists(), "il record corrotto non viene cancellato");
    server.task.abort();
    let _ = server.task.await;
}

/// Un record con il token sbagliato (riscritto dall'esterno) porta al rifiuto
/// del preambolo, non al tetto: il ponte esce con servizio non disponibile e
/// lo dice, senza chiamare la chiusura «occupato».
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_token_rifiutato_dal_server_non_e_un_occupato() {
    let casa = cartella_privata("lume-mcp-falso-token-");
    let dati = casa.path().join("dati");
    std::fs::create_dir(&dati).unwrap();
    std::fs::set_permissions(&dati, PermissionsExt::from_mode(0o700)).unwrap();
    let radice = casa.path().join("progetto");
    std::fs::create_dir(&radice).unwrap();
    progetto(&radice);

    let server = avvia_server(&dati, &radice).await;
    let record = unix::read_records(server.endpoint.dir())
        .unwrap()
        .records
        .into_iter()
        .next()
        .expect("il server pubblica il suo record");
    let mut falsificato = record;
    falsificato.token = Token::generate().unwrap();
    std::fs::write(
        server.endpoint.record_path(),
        serde_json::to_vec(&falsificato).unwrap(),
    )
    .unwrap();

    let ponte = Ponte::avvia(&dati, &[]);
    let (codice, stderr) = ponte.attendi_uscita().await;
    assert_eq!(codice, Some(69), "stderr: {stderr}");
    assert!(
        stderr.contains("prima di rispondere"),
        "il messaggio dice cos'è successo: {stderr}"
    );
    assert!(
        !stderr.contains("occupate"),
        "un token rifiutato non è un tetto: {stderr}"
    );
    server.task.abort();
    let _ = server.task.await;
}

/// Quattro ponteggi veri tengono le quattro connessioni; il quinto riceve la
/// riga di tetto e un codice di servizio non disponibile, con il messaggio
/// «occupate» su stderr.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn il_quinto_ponte_esce_con_servizio_non_disponibile() {
    let casa = cartella_privata("lume-mcp-quinto-");
    let dati = casa.path().join("dati");
    std::fs::create_dir(&dati).unwrap();
    std::fs::set_permissions(&dati, PermissionsExt::from_mode(0o700)).unwrap();
    let radice = casa.path().join("progetto");
    std::fs::create_dir(&radice).unwrap();
    progetto(&radice);

    let mut server = avvia_server(&dati, &radice).await;
    let mut occupanti = Vec::new();
    for indice in 0..4 {
        let mut ponte = Ponte::avvia(&dati, &[]);
        let risposta = ponte.inizializza(&format!("occupante-{indice}")).await;
        assert_eq!(risposta["id"], 1);
        match prossimo_evento(&mut server).await {
            McpEvent::ClientConnected { .. } => {}
            altro => panic!("atteso ClientConnected, non {altro:?}"),
        }
        occupanti.push(ponte);
    }

    let mut quinto = Ponte::avvia(&dati, &[]);
    let riga = quinto.ricevi().await;
    assert!(riga["id"].is_null(), "{riga}");
    assert_eq!(riga["error"]["code"], -32000, "{riga}");
    let (codice, stderr) = quinto.chiudi_e_attendi().await;
    assert_eq!(codice, Some(69), "stderr: {stderr}");
    assert!(stderr.contains("occupate"), "{stderr}");

    for (indice, ponte) in occupanti.into_iter().enumerate() {
        let (codice, stderr) = ponte.chiudi_e_attendi().await;
        assert_eq!(codice, Some(0), "occupante {indice}: {stderr}");
    }
    server.task.abort();
    let _ = server.task.await;
}
