#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

//! Prove d'integrazione sul socket Unix reale e sulla scoperta reale.
//!
//! Non ci sono mock: il server ascolta su un socket filesystem, i client sono
//! `UnixStream` veri e i record sono file 0600 nella cartella privata.

use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use lume_mcp::server::{Endpoint, ServeConfig, serve};
use lume_mcp::unix::{self, DiscoveryFile, Identity, Record};
use lume_mcp::{
    AppAnswer, AppQuery, Availability, McpCommand, McpEvent, Root, TextSnapshot, Token,
};
use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, bind, listen, socket};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

fn cartella_privata(prefix: &str) -> tempfile::TempDir {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME serve per una cartella dati privata");
    tempfile::Builder::new()
        .prefix(prefix)
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(home)
        .expect("cartella privata di prova")
}

fn cartella_dati() -> tempfile::TempDir {
    cartella_privata("lume-mcp-dati-")
}

fn radice_progetto() -> tempfile::TempDir {
    let radice = tempfile::tempdir().unwrap();
    std::fs::write(radice.path().join("a.txt"), "ciao dal disco\n").unwrap();
    std::fs::create_dir(radice.path().join("src")).unwrap();
    std::fs::write(radice.path().join("src/lib.rs"), "pub fn ciao() {}\n").unwrap();
    radice
}

struct Server {
    task: JoinHandle<()>,
    endpoint: unix::Endpoint,
    token: Token,
    eventi: UnboundedReceiver<McpEvent>,
    posta: UnboundedSender<McpCommand>,
}

async fn avvia_in(dati: &Path, radice: &Path) -> Server {
    let endpoint = unix::Endpoint::new(dati).unwrap();
    avvia_endpoint(endpoint, radice).await
}

async fn avvia_endpoint(endpoint: unix::Endpoint, radice: &Path) -> Server {
    let token = Token::generate().unwrap();
    let (tx, mut eventi) = unbounded_channel();
    let config = ServeConfig {
        endpoint: Endpoint::Unix(endpoint.clone()),
        token: token.clone(),
        root: Root::new(radice).unwrap(),
        server_version: "0.0.0-prova".to_owned(),
        availability: Availability::default(),
        preamble_timeout: Duration::from_millis(300),
        line_cap: 4096,
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
        token,
        eventi,
        posta,
    }
}

struct Cliente {
    r: BufReader<tokio::io::ReadHalf<UnixStream>>,
    w: tokio::io::WriteHalf<UnixStream>,
}

impl Cliente {
    async fn connetti(server: &Server) -> Self {
        let stream = UnixStream::connect(server.endpoint.socket_path())
            .await
            .unwrap();
        let (r, mut w) = tokio::io::split(stream);
        w.write_all(format!("{}\n", lume_mcp::discovery::preamble(&server.token)).as_bytes())
            .await
            .unwrap();
        Self {
            r: BufReader::new(r),
            w,
        }
    }

    async fn manda(&mut self, messaggio: Value) {
        self.w
            .write_all(format!("{messaggio}\n").as_bytes())
            .await
            .unwrap();
        self.w.flush().await.unwrap();
    }

    async fn ricevi(&mut self) -> Value {
        let mut riga = String::new();
        let letti = tokio::time::timeout(Duration::from_secs(10), self.r.read_line(&mut riga))
            .await
            .expect("risposta entro 10 s")
            .unwrap();
        assert!(letti > 0, "connessione chiusa invece di una risposta");
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

#[tokio::test]
async fn initialize_tools_list_e_lettura_su_socket_reale() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let mut server = avvia_in(dati.path(), radice.path()).await;
    assert_eq!(
        std::fs::symlink_metadata(server.endpoint.socket_path())
            .unwrap()
            .mode()
            & 0o777,
        0o600,
        "il socket nasce 0600"
    );
    let record = unix::read_records(server.endpoint.dir()).unwrap().records;
    assert_eq!(record.len(), 1);
    assert_eq!(record[0].id, server.endpoint.identity().to_string());

    let mut cliente = Cliente::connetti(&server).await;
    let risposta = cliente.inizializza("prova-unix").await;
    assert_eq!(risposta["id"], 1);
    assert_eq!(
        risposta["result"]["capabilities"]["tools"]["listChanged"],
        true
    );
    let canonica = lume_mcp::confine::canonical(radice.path()).unwrap();
    assert_eq!(
        risposta["result"]["serverInfo"]["project_root"],
        canonica.display().to_string()
    );
    match prossimo_evento(&mut server).await {
        McpEvent::ClientConnected { name, .. } => assert_eq!(name, "prova-unix"),
        altro => panic!("atteso ClientConnected, non {altro:?}"),
    }

    cliente
        .manda(json!({"jsonrpc": "2.0", "id": "lista", "method": "tools/list"}))
        .await;
    let lista = cliente.ricevi().await;
    assert_eq!(lista["id"], "lista");
    assert_eq!(
        lista["result"]["tools"].as_array().unwrap().len(),
        lume_mcp::catalog::tools_list(Availability::default())["tools"]
            .as_array()
            .unwrap()
            .len()
    );

    cliente
        .manda(json!({
            "jsonrpc": "2.0", "id": "lettura", "method": "tools/call",
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
    let lettura = cliente.ricevi().await;
    assert_eq!(lettura["id"], "lettura");
    assert_eq!(contenuto(&lettura)["text"], "ciao dal disco\n");
    assert_eq!(contenuto(&lettura)["source"], "disk");
}

#[tokio::test]
async fn aprire_nell_editor_passa_dall_app_e_risponde() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let mut server = avvia_in(dati.path(), radice.path()).await;
    let mut cliente = Cliente::connetti(&server).await;
    cliente.inizializza("apri").await;
    let _ = prossimo_evento(&mut server).await;

    cliente
        .manda(json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": "open_in_editor", "arguments": {"path": "src/lib.rs", "line": 2}}
        }))
        .await;
    let (id, _, query) = prossima_ask(&mut server).await;
    assert_eq!(
        query,
        AppQuery::OpenInEditor {
            relative: PathBuf::from("src/lib.rs"),
            line: Some(2)
        }
    );
    rispondi(
        &server,
        id,
        AppAnswer::Json(json!({"path": "src/lib.rs", "line": 2, "opened": true})),
    );
    let risposta = cliente.ricevi().await;
    assert_eq!(risposta["id"], 7);
    assert_eq!(contenuto(&risposta)["opened"], true);
}

#[tokio::test]
async fn un_token_sbagliato_o_un_preambolo_malformato_non_danno_byte() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let server = avvia_in(dati.path(), radice.path()).await;
    for preambolo in [
        format!(
            "{}\n",
            lume_mcp::discovery::preamble(&Token::generate().unwrap())
        ),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_owned(),
        "non-json\n".to_owned(),
    ] {
        let mut stream = UnixStream::connect(server.endpoint.socket_path())
            .await
            .unwrap();
        stream.write_all(preambolo.as_bytes()).await.unwrap();
        let mut letto = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut letto))
            .await
            .expect("il server chiude");
        assert!(letto.is_empty(), "nessun byte a chi non ha il token");
    }
}

#[tokio::test]
async fn un_client_muto_viene_chiuso_dopo_il_timeout() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let server = avvia_in(dati.path(), radice.path()).await;
    let mut stream = UnixStream::connect(server.endpoint.socket_path())
        .await
        .unwrap();
    let mut letto = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut letto))
        .await
        .expect("il server chiude un client muto");
    assert!(letto.is_empty());
}

#[tokio::test]
async fn una_seconda_istanza_sullo_stesso_endpoint_fallisce_senza_toccare_la_prima() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let prima = avvia_in(dati.path(), radice.path()).await;
    let endpoint = prima.endpoint.clone();
    let token = Token::generate().unwrap();
    let (tx, mut eventi) = unbounded_channel();
    tokio::spawn(serve(
        ServeConfig {
            endpoint: Endpoint::Unix(endpoint),
            token,
            root: Root::new(radice.path()).unwrap(),
            server_version: "0.0.0-prova".to_owned(),
            availability: Availability::default(),
            preamble_timeout: Duration::from_millis(300),
            line_cap: 4096,
            ask_timeout: lume_mcp::server::ASK_TIMEOUT,
        },
        tx,
    ));
    match tokio::time::timeout(Duration::from_secs(10), eventi.recv())
        .await
        .unwrap()
    {
        Some(McpEvent::Failed(motivo)) => assert!(motivo.contains("esiste già"), "{motivo}"),
        altro => panic!("atteso Failed, non {altro:?}"),
    }
    let mut cliente = Cliente::connetti(&prima).await;
    let risposta = cliente.inizializza("ancora-viva").await;
    assert_eq!(risposta["id"], 1);
}

#[tokio::test]
async fn la_scoperta_con_permessi_larghi_non_avvia_il_server() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o755);
    builder.create(&dir).unwrap();
    // La umask del runner può togliere i bit: il test vuole proprio 0755.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let token = Token::generate().unwrap();
    let (tx, mut eventi) = unbounded_channel();
    tokio::spawn(serve(
        ServeConfig {
            endpoint: Endpoint::Unix(unix::Endpoint::new(dati.path()).unwrap()),
            token,
            root: Root::new(radice.path()).unwrap(),
            server_version: "0.0.0-prova".to_owned(),
            availability: Availability::default(),
            preamble_timeout: Duration::from_millis(300),
            line_cap: 4096,
            ask_timeout: lume_mcp::server::ASK_TIMEOUT,
        },
        tx,
    ));
    match tokio::time::timeout(Duration::from_secs(10), eventi.recv())
        .await
        .unwrap()
    {
        Some(McpEvent::Failed(motivo)) => assert!(motivo.contains("0755"), "{motivo}"),
        altro => panic!("atteso Failed, non {altro:?}"),
    }
    assert_eq!(
        std::fs::symlink_metadata(&dir).unwrap().mode() & 0o7777,
        0o755,
        "nessun chmod implicito"
    );
    assert!(unix::read_records(&dir).unwrap().records.is_empty());
}

#[tokio::test]
async fn la_scelta_rifiuta_ambiguita_e_accetta_l_istanza_esplicita() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let uno = avvia_in(dati.path(), radice.path()).await;
    let due = avvia_in(dati.path(), radice.path()).await;
    assert_ne!(uno.endpoint.identity(), due.endpoint.identity());
    let dir = unix::discovery_dir(dati.path());
    let canonica = lume_mcp::confine::canonical(radice.path()).unwrap();

    assert!(matches!(
        unix::select(&dir, Some(&canonica), None, None).await,
        Err(unix::SelectError::Ambiguous(_))
    ));
    let scelto = unix::select(
        &dir,
        Some(&canonica),
        None,
        Some(&uno.endpoint.identity().to_string()),
    )
    .await
    .unwrap();
    assert_eq!(scelto.record.id, uno.endpoint.identity().to_string());
    assert!(matches!(
        unix::select(&dir, None, None, Some("999-00000000")).await,
        Err(unix::SelectError::UnknownInstance(_))
    ));
    let altrove = tempfile::tempdir().unwrap();
    assert!(matches!(
        unix::select(
            &dir,
            Some(&lume_mcp::confine::canonical(altrove.path()).unwrap()),
            None,
            Some(&uno.endpoint.identity().to_string()),
        )
        .await,
        Err(unix::SelectError::NoMatch(_))
    ));
    uno.task.abort();
    due.task.abort();
    let _ = uno.task.await;
    let _ = due.task.await;
}

fn identita_di_prova(indice: u32) -> Identity {
    Identity::parse(&format!("{}-{indice:08x}", std::process::id())).unwrap()
}

/// Record valido il cui socket non esiste più: il caso di una rimozione
/// esterna o di un processo caduto dopo la pubblicazione.
fn pubblica_record_senza_socket(dir: &Path, radice: &Path, indice: u32) -> unix::Endpoint {
    let endpoint =
        unix::Endpoint::with_identity(dir.to_path_buf(), identita_di_prova(indice)).unwrap();
    let record = Record::new(
        &endpoint,
        radice.to_path_buf(),
        Token::generate().unwrap(),
        "0.0.0-prova".to_owned(),
        &std::fs::symlink_metadata(radice).unwrap(),
    );
    let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();
    std::mem::forget(guard);
    endpoint
}

/// Record valido con un socket rimasto su disco ma senza nessuno in ascolto:
/// la connessione restituisce `ECONNREFUSED`, il segnale certo di stantio.
fn pubblica_record_con_socket_morto(dir: &Path, radice: &Path, indice: u32) -> unix::Endpoint {
    let endpoint =
        unix::Endpoint::with_identity(dir.to_path_buf(), identita_di_prova(indice)).unwrap();
    let ascolto = std::os::unix::net::UnixListener::bind(endpoint.socket_path()).unwrap();
    std::fs::set_permissions(
        endpoint.socket_path(),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let record = Record::new(
        &endpoint,
        radice.to_path_buf(),
        Token::generate().unwrap(),
        "0.0.0-prova".to_owned(),
        &std::fs::symlink_metadata(endpoint.socket_path()).unwrap(),
    );
    let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();
    std::mem::forget(guard);
    drop(ascolto);
    endpoint
}

#[tokio::test]
async fn un_record_senza_socket_viene_rimosso_con_il_ricontrollo() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let endpoint = pubblica_record_senza_socket(&dir, radice.path(), 1);
    let report = unix::select_report(&dir, None, None, None).await;
    assert!(
        matches!(report.result, Err(unix::SelectError::NoInstance)),
        "{:?}",
        report.result
    );
    assert!(
        !endpoint.record_path().exists(),
        "un record senza socket non può descrivere un'istanza viva"
    );
    assert!(!endpoint.socket_path().exists());
}

#[tokio::test]
async fn un_record_con_socket_morto_viene_rimosso_dopo_il_rifiuto() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let endpoint = pubblica_record_con_socket_morto(&dir, radice.path(), 2);
    assert!(endpoint.socket_path().exists());
    let report = unix::select_report(&dir, None, None, None).await;
    assert!(
        matches!(report.result, Err(unix::SelectError::NoInstance)),
        "{:?}",
        report.result
    );
    assert!(!endpoint.record_path().exists(), "record stantio rimosso");
    assert!(!endpoint.socket_path().exists(), "socket stantio rimosso");
}

#[tokio::test]
async fn un_record_corrotto_non_impedisce_la_scelta_dell_istanza_sana() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let server = avvia_in(dati.path(), radice.path()).await;
    let dir = unix::discovery_dir(dati.path());
    let corrotto = dir.join(format!("{}-00000000.json", std::process::id()));
    std::fs::write(&corrotto, b"{non-json").unwrap();
    std::fs::set_permissions(&corrotto, std::fs::Permissions::from_mode(0o600)).unwrap();
    let report = unix::select_report(&dir, None, None, None).await;
    let scelto = report.result.expect("l'istanza sana viene scelta");
    assert_eq!(scelto.record.id, server.endpoint.identity().to_string());
    assert!(
        report
            .notices
            .iter()
            .any(|notice| notice.path == corrotto && !notice.reason.is_empty()),
        "la diagnostica nomina il file corrotto: {:?}",
        report.notices
    );
    assert!(
        corrotto.exists(),
        "un record illeggibile non viene cancellato: può essere in scrittura"
    );
    server.task.abort();
    let _ = server.task.await;
}

#[tokio::test]
async fn oltre_sessantaquattro_record_stantii_non_bloccano_una_istanza_sana() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let mut stantii = Vec::new();
    for indice in 0..65_u32 {
        stantii.push(pubblica_record_con_socket_morto(
            &dir,
            radice.path(),
            indice,
        ));
    }
    let server = avvia_in(dati.path(), radice.path()).await;
    let scelto = unix::select(&dir, None, None, None)
        .await
        .expect("i record stantii non bloccano l'istanza sana");
    assert_eq!(scelto.record.id, server.endpoint.identity().to_string());
    for endpoint in stantii {
        assert!(!endpoint.record_path().exists(), "record stantio potato");
        assert!(!endpoint.socket_path().exists(), "socket stantio potato");
    }
    server.task.abort();
    let _ = server.task.await;
}

#[tokio::test]
async fn il_server_pota_i_record_stantii_all_accensione() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let stantio = pubblica_record_con_socket_morto(&dir, radice.path(), 3);
    let server = avvia_in(dati.path(), radice.path()).await;
    assert!(
        !stantio.record_path().exists(),
        "potatura una volta al bind"
    );
    assert!(!stantio.socket_path().exists());
    assert!(server.endpoint.record_path().exists());
    server.task.abort();
    let _ = server.task.await;
}

#[tokio::test]
async fn un_record_con_permessi_larghi_viene_saltato_con_diagnostica() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let endpoint = pubblica_record_senza_socket(&dir, radice.path(), 4);
    std::fs::set_permissions(
        endpoint.record_path(),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(unix::read_records(&dir).unwrap().records.is_empty());
    let report = unix::select_report(&dir, None, None, None).await;
    assert!(
        matches!(report.result, Err(unix::SelectError::NoInstance)),
        "{:?}",
        report.result
    );
    assert!(
        report
            .notices
            .iter()
            .any(|notice| notice.reason.contains("0644")),
        "{:?}",
        report.notices
    );
    assert!(endpoint.record_path().exists(), "record non cancellato");
}

#[tokio::test]
async fn oltre_sessantaquattro_file_invalidi_non_bloccano_la_scelta() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let server = avvia_in(dati.path(), radice.path()).await;
    let dir = unix::discovery_dir(dati.path());
    for indice in 0..65 {
        let path = dir.join(format!("{}-{indice:08x}.json", std::process::id()));
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(unix::read_records(&dir).unwrap().records.len() == 1);
    let report = unix::select_report(&dir, None, None, None).await;
    let scelto = report.result.expect("l'istanza sana viene scelta");
    assert_eq!(scelto.record.id, server.endpoint.identity().to_string());
    assert!(
        report.notices.len() >= 65,
        "ogni file invalido produce una diagnostica: {}",
        report.notices.len()
    );
    server.task.abort();
    let _ = server.task.await;
}

#[tokio::test]
async fn un_record_linkato_non_viene_seguito() {
    let dati = cartella_dati();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let vero = dir.join("1-00000000.json");
    std::fs::write(&vero, b"{}").unwrap();
    std::fs::set_permissions(&vero, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.join("2-00000000.json");
    std::os::unix::fs::symlink(&vero, &link).unwrap();
    assert!(unix::read_records(&dir).unwrap().records.is_empty());
    let report = unix::select_report(&dir, None, None, None).await;
    assert!(matches!(report.result, Err(unix::SelectError::NoInstance)));
    assert!(
        report.notices.iter().any(|notice| notice.path == link),
        "il link viene nominato nella diagnostica: {:?}",
        report.notices
    );
    assert!(link.exists(), "il link non viene cancellato");
}

/// Il ponte vero (`bridge_socket`) su una connessione già selezionata: stdin e
/// stdout sono due duplex, il socket è quello del server. Serve per fare
/// handshake reali, non solo connessioni di prova.
struct Ponte {
    verso: tokio::io::DuplexStream,
    dal: tokio::io::BufReader<tokio::io::DuplexStream>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Ponte {
    fn apri(scelto: unix::Selected) -> Self {
        let (verso, stdin_ponte) = tokio::io::duplex(64 * 1024);
        let (stdout_ponte, dal) = tokio::io::duplex(64 * 1024);
        let token = scelto.record.token.clone();
        let task = tokio::spawn(async move {
            lume_mcp::proxy::bridge_socket(stdin_ponte, stdout_ponte, scelto.stream, &token).await
        });
        Self {
            verso,
            dal: tokio::io::BufReader::new(dal),
            task,
        }
    }

    async fn manda(&mut self, messaggio: Value) {
        self.verso
            .write_all(format!("{messaggio}\n").as_bytes())
            .await
            .unwrap();
        self.verso.flush().await.unwrap();
    }

    /// Prossima riga JSON del server; `None` se il ponte chiude senza righe.
    async fn ricevi(&mut self) -> Option<Value> {
        let mut riga = String::new();
        let letti = tokio::time::timeout(Duration::from_secs(10), self.dal.read_line(&mut riga))
            .await
            .expect("riga del ponte entro 10 s")
            .unwrap();
        if letti == 0 {
            return None;
        }
        Some(serde_json::from_str(&riga).unwrap())
    }

    async fn inizializza(&mut self, nome: &str) -> Option<Value> {
        self.manda(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": nome}}
        }))
        .await;
        self.ricevi().await
    }

    /// Chiude stdin e attende l'esito del ponte.
    async fn chiudi(self) -> std::io::Result<()> {
        let Self { verso, dal, task } = self;
        drop(verso);
        let esito = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("il ponte esce entro 10 s")
            .expect("il task del ponte non va in panico");
        drop(dal);
        esito
    }
}

/// Con tre client autenticati, una nuova selezione e un handshake completo del
/// quarto riescono, ripetutamente nello stesso test: le connessioni di prova
/// della scoperta (liveness e probe) non consumano il tetto dei client e non
/// possono far rifiutare la connessione reale.
#[tokio::test]
async fn con_tre_client_attivi_il_quarto_entra_a_ogni_giro() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let mut server = avvia_in(dati.path(), radice.path()).await;
    let mut occupanti = Vec::new();
    for indice in 0..3 {
        let mut cliente = Cliente::connetti(&server).await;
        cliente.inizializza(&format!("attivo-{indice}")).await;
        match prossimo_evento(&mut server).await {
            McpEvent::ClientConnected { .. } => {}
            altro => panic!("atteso ClientConnected, non {altro:?}"),
        }
        occupanti.push(cliente);
    }
    for giro in 0..4 {
        let scelto = tokio::time::timeout(
            Duration::from_secs(10),
            unix::select(&unix::discovery_dir(dati.path()), None, None, None),
        )
        .await
        .expect("la selezione non resta appesa")
        .expect("l'istanza con tre client attivi resta selezionabile");
        assert_eq!(scelto.record.id, server.endpoint.identity().to_string());
        let mut ponte = Ponte::apri(scelto);
        let risposta = tokio::time::timeout(Duration::from_secs(10), ponte.inizializza("quarto"))
            .await
            .expect("l'handshake del quarto client non resta appeso")
            .expect("il quarto client riceve una risposta");
        assert_eq!(
            risposta["result"]["protocolVersion"], "2025-06-18",
            "giro {giro}: {risposta}"
        );
        match prossimo_evento(&mut server).await {
            McpEvent::ClientConnected { name, .. } => assert_eq!(name, "quarto"),
            altro => panic!("giro {giro}: atteso ClientConnected, non {altro:?}"),
        }
        let esito = tokio::time::timeout(Duration::from_secs(10), ponte.chiudi())
            .await
            .expect("il quarto ponte esce");
        assert!(esito.is_ok(), "giro {giro}: {esito:?}");
        match prossimo_evento(&mut server).await {
            McpEvent::ClientGone(_) => {}
            altro => panic!("giro {giro}: atteso ClientGone, non {altro:?}"),
        }
    }
    drop(occupanti);
    server.task.abort();
    let _ = server.task.await;
}

/// Al tetto il server risponde con la riga di tetto dopo il preambolo: il
/// ponte la inoltra al client ed esce «occupato», servizio non disponibile.
#[tokio::test]
async fn il_quinto_client_riceve_la_riga_di_tetto() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let mut server = avvia_in(dati.path(), radice.path()).await;
    let mut occupanti = Vec::new();
    for indice in 0..4 {
        let mut cliente = Cliente::connetti(&server).await;
        cliente.inizializza(&format!("occupante-{indice}")).await;
        match prossimo_evento(&mut server).await {
            McpEvent::ClientConnected { .. } => {}
            altro => panic!("atteso ClientConnected, non {altro:?}"),
        }
        occupanti.push(cliente);
    }
    let scelto = tokio::time::timeout(
        Duration::from_secs(10),
        unix::select(&unix::discovery_dir(dati.path()), None, None, None),
    )
    .await
    .expect("la selezione non resta appesa")
    .expect("le prove della scoperta non consumano il tetto dei client");
    let mut ponte = Ponte::apri(scelto);
    let riga = tokio::time::timeout(Duration::from_secs(10), ponte.inizializza("quinto"))
        .await
        .expect("il quinto ponte non resta appeso")
        .expect("il client riceve la riga di tetto, non una chiusura muta");
    assert!(
        riga["id"].is_null(),
        "nessuna richiesta è stata letta: {riga}"
    );
    assert_eq!(riga["error"]["code"], -32000, "{riga}");
    assert!(
        riga["error"]["message"]
            .as_str()
            .is_some_and(|testo| testo.contains("occupate")),
        "{riga}"
    );
    let errore = ponte
        .chiudi()
        .await
        .expect_err("il quinto client dev'essere rifiutato");
    assert_eq!(
        errore.kind(),
        std::io::ErrorKind::ConnectionRefused,
        "{errore}"
    );
    assert!(
        errore.to_string().contains("occupate"),
        "il messaggio dice perché: {errore}"
    );
    drop(occupanti);
    server.task.abort();
    let _ = server.task.await;
}

/// Un token sbagliato chiude la connessione senza byte dal server: non è
/// «occupato», è un rifiuto con il suo messaggio.
#[tokio::test]
async fn un_token_sbagliato_non_viene_chiamato_occupato() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let server = avvia_in(dati.path(), radice.path()).await;
    let stream = UnixStream::connect(server.endpoint.socket_path())
        .await
        .unwrap();
    let (verso, stdin_ponte) = tokio::io::duplex(4096);
    let (stdout_ponte, mut dal_ponte) = tokio::io::duplex(4096);
    let falso = Token::generate().unwrap();
    let esito = tokio::time::timeout(
        Duration::from_secs(10),
        lume_mcp::proxy::bridge_socket(stdin_ponte, stdout_ponte, stream, &falso),
    )
    .await
    .expect("il ponte termina da sé");
    let errore = esito.expect_err("un token rifiutato non è un successo");
    assert_eq!(errore.kind(), std::io::ErrorKind::UnexpectedEof, "{errore}");
    assert!(
        errore.to_string().contains("prima di rispondere"),
        "il messaggio dice cos'è successo: {errore}"
    );
    assert!(
        !errore.to_string().contains("occupate"),
        "un token sbagliato non è un tetto raggiunto: {errore}"
    );
    let mut letto = Vec::new();
    let _ = dal_ponte.read_to_end(&mut letto).await;
    assert!(
        letto.is_empty(),
        "nessun byte a chi non ha il token: {letto:?}"
    );
    drop(verso);
    server.task.abort();
    let _ = server.task.await;
}

/// Errore di scrittura a metà sessione (il lato server ha smesso di leggere):
/// resta un errore di I/O del ponte, non un «occupato».
#[tokio::test]
async fn un_errore_a_meta_sessione_resta_un_errore_di_io() {
    let (client, mut servitore) = UnixStream::pair().unwrap();
    let token = Token::generate().unwrap();
    let (mut verso, stdin_ponte) = tokio::io::duplex(4096);
    let (stdout_ponte, dal_ponte) = tokio::io::duplex(4096);
    let mut dal_ponte = BufReader::new(dal_ponte);
    let ponte = tokio::spawn({
        let token = token.clone();
        async move { lume_mcp::proxy::bridge_socket(stdin_ponte, stdout_ponte, client, &token).await }
    });
    // Lato server vero (un socket del kernel): legge il preambolo, risponde e
    // chiude la lettura. La scrittura successiva del client riceve `EPIPE`,
    // mentre il verso di lettura resta aperto: non è EOF.
    let mut lettore = BufReader::new(&mut servitore);
    let mut preambolo = Vec::new();
    let letti = tokio::time::timeout(
        Duration::from_secs(10),
        lettore.read_until(b'\n', &mut preambolo),
    )
    .await
    .expect("il preambolo arriva")
    .unwrap();
    assert!(letti > 0, "il ponte manda il preambolo");
    drop(lettore);
    assert!(
        lume_mcp::discovery::check_preamble(
            preambolo.strip_suffix(b"\n").unwrap_or(&preambolo),
            &token
        ),
        "il preambolo del ponte è quello del token"
    );
    servitore
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n")
        .await
        .unwrap();
    servitore.flush().await.unwrap();
    rustix::net::shutdown(servitore.as_fd(), rustix::net::Shutdown::Read).unwrap();
    let mut riga = String::new();
    let letti = tokio::time::timeout(Duration::from_secs(10), dal_ponte.read_line(&mut riga))
        .await
        .expect("la prima risposta arriva")
        .unwrap();
    assert!(letti > 0, "la sessione è iniziata: {riga}");
    verso
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n")
        .await
        .unwrap();
    verso.flush().await.unwrap();
    let errore = tokio::time::timeout(Duration::from_secs(10), ponte)
        .await
        .expect("il ponte termina da sé")
        .expect("il task del ponte non va in panico")
        .expect_err("la scrittura a metà sessione fallisce");
    assert_ne!(
        errore.kind(),
        std::io::ErrorKind::ConnectionRefused,
        "un errore a metà sessione non è un tetto: {errore}"
    );
    assert!(
        !errore.to_string().contains("occupate"),
        "un errore a metà sessione non è un tetto: {errore}"
    );
}

/// Un candidato incoerente (inode diverso dal record) viene escluso con un
/// avviso: l'istanza sana viene scelta e il record resta al suo posto.
#[tokio::test]
async fn un_candidato_incoerente_viene_escluso_con_avviso() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let server = avvia_in(dati.path(), radice.path()).await;
    let dir = unix::discovery_dir(dati.path());
    let canonica = lume_mcp::confine::canonical(radice.path()).unwrap();
    let endpoint = unix::Endpoint::with_identity(dir.clone(), identita_di_prova(9)).unwrap();
    let ascolto = std::os::unix::net::UnixListener::bind(endpoint.socket_path()).unwrap();
    std::fs::set_permissions(
        endpoint.socket_path(),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    // Il record dichiara inode/dev della radice, non del socket che ha quel
    // percorso: il socket è vivo ma non è l'endpoint descritto.
    let record = Record::new(
        &endpoint,
        canonica.clone(),
        Token::generate().unwrap(),
        "0.0.0-prova".to_owned(),
        &std::fs::symlink_metadata(radice.path()).unwrap(),
    );
    let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();
    std::mem::forget(guard);
    let report = unix::select_report(&dir, Some(&canonica), None, None).await;
    let scelto = report
        .result
        .expect("l'istanza sana viene scelta nonostante il record incoerente");
    assert_eq!(scelto.record.id, server.endpoint.identity().to_string());
    let percorso = endpoint.record_path();
    assert!(
        report
            .notices
            .iter()
            .any(|notice| notice.path == percorso && notice.reason.contains("endpoint")),
        "la diagnostica nomina il record incoerente: {:?}",
        report.notices
    );
    assert!(
        percorso.exists(),
        "un candidato incoerente non viene cancellato"
    );
    drop(ascolto);
    server.task.abort();
    let _ = server.task.await;
}

#[tokio::test]
async fn stop_rimuove_record_e_socket() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let server = avvia_in(dati.path(), radice.path()).await;
    let socket = server.endpoint.socket_path();
    let record = server.endpoint.record_path();
    assert!(socket.exists() && record.exists());
    server.task.abort();
    let _ = server.task.await;
    assert!(!record.exists(), "il record sparisce con lo stream");
    assert!(!socket.exists(), "il socket sparisce con lo stream");
}

/// Il testo del buffer sporco vince sul disco, come già sul trasporto
/// Windows: il trasporto non deve cambiare la semantica dei tool.
#[tokio::test]
async fn la_lettura_preferisce_il_buffer_sporco() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let mut server = avvia_in(dati.path(), radice.path()).await;
    let mut cliente = Cliente::connetti(&server).await;
    cliente.inizializza("buffer").await;
    let _ = prossimo_evento(&mut server).await;
    cliente
        .manda(json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "read_file", "arguments": {"path": "a.txt"}}
        }))
        .await;
    let (id, _, _) = prossima_ask(&mut server).await;
    rispondi(
        &server,
        id,
        AppAnswer::Buffer(Some(TextSnapshot {
            text: "modificato\n".to_owned(),
            dirty: true,
            truncated: false,
        })),
    );
    let risposta = cliente.ricevi().await;
    let corpo = contenuto(&risposta);
    assert_eq!(corpo["text"], "modificato\n");
    assert_eq!(corpo["source"], "buffer");
    assert_eq!(corpo["dirty"], true);
}

#[tokio::test]
async fn un_socket_con_permessi_larghi_e_un_errore_esplicito() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let endpoint =
        unix::Endpoint::with_identity(dir.clone(), Identity::generate().unwrap()).unwrap();
    let socket_path = endpoint.socket_path();
    let ascolto = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    std::fs::set_permissions(&socket_path, PermissionsExt::from_mode(0o666)).unwrap();
    let record = Record::new(
        &endpoint,
        lume_mcp::confine::canonical(radice.path()).unwrap(),
        Token::generate().unwrap(),
        "0.0.0-prova".to_owned(),
        &std::fs::symlink_metadata(&socket_path).unwrap(),
    );
    let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();
    let errore = unix::select(&dir, None, None, None).await.unwrap_err();
    assert!(
        matches!(&errore, unix::SelectError::Unsafe(motivo) if motivo.contains("0666")),
        "{errore}"
    );
    drop(guard);
    drop(ascolto);
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn un_record_con_inode_diverso_non_e_l_endpoint() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let endpoint =
        unix::Endpoint::with_identity(dir.clone(), Identity::generate().unwrap()).unwrap();
    let ascolto = std::os::unix::net::UnixListener::bind(endpoint.socket_path()).unwrap();
    std::fs::set_permissions(endpoint.socket_path(), PermissionsExt::from_mode(0o600)).unwrap();
    // Il record descrive l'inode sbagliato: il path non è l'endpoint che
    // dichiara, quindi non va scelto nemmeno se il socket esiste.
    let record = Record::new(
        &endpoint,
        lume_mcp::confine::canonical(radice.path()).unwrap(),
        Token::generate().unwrap(),
        "0.0.0-prova".to_owned(),
        &std::fs::symlink_metadata(radice.path()).unwrap(),
    );
    let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();
    // È l'unico candidato: la selezione fallisce, ma con un avviso sul record
    // incoerente e senza cancellarlo.
    let report = unix::select_report(&dir, None, None, None).await;
    assert!(
        matches!(&report.result, Err(unix::SelectError::Unreachable(ids)) if ids.len() == 1),
        "{:?}",
        report.result
    );
    assert!(
        report
            .notices
            .iter()
            .any(|notice| notice.path == endpoint.record_path()
                && notice.reason.contains("endpoint")),
        "{:?}",
        report.notices
    );
    assert!(
        endpoint.record_path().exists(),
        "il record non viene cancellato"
    );
    drop(guard);
    drop(ascolto);
}

/// Coda di un listener davvero satura: il selettore non può connettersi.
/// Il record e il socket restano; un tentativo nuovo, dopo aver liberato la
/// coda, riesce.
#[tokio::test]
async fn un_candidato_con_la_coda_occupata_non_viene_scelto_ne_cancellato() {
    let dati = cartella_dati();
    let radice = radice_progetto();
    let dir = unix::discovery_dir(dati.path());
    unix::ensure_private_dir(&dir).unwrap();
    let endpoint =
        unix::Endpoint::with_identity(dir.clone(), Identity::generate().unwrap()).unwrap();
    let socket_path = endpoint.socket_path();
    let fd = socket(AddressFamily::UNIX, SocketType::STREAM, None).unwrap();
    let address = SocketAddrUnix::new(socket_path.as_os_str().as_bytes()).unwrap();
    bind(&fd, &address).unwrap();
    listen(&fd, 0).unwrap();
    std::fs::set_permissions(&socket_path, PermissionsExt::from_mode(0o600)).unwrap();
    let record = Record::new(
        &endpoint,
        lume_mcp::confine::canonical(radice.path()).unwrap(),
        Token::generate().unwrap(),
        "0.0.0-prova".to_owned(),
        &std::fs::symlink_metadata(&socket_path).unwrap(),
    );
    let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();

    // Backlog 0: la prima connessione riempie la coda, la seconda resta
    // sospesa nel kernel finché il listener non viene chiuso.
    let riempimento = std::os::unix::net::UnixStream::connect(&socket_path).unwrap();
    let percorso = socket_path.clone();
    let sospeso = std::thread::spawn(move || {
        let _ = std::os::unix::net::UnixStream::connect(percorso);
    });
    std::thread::sleep(Duration::from_millis(100));

    let esito = tokio::time::timeout(
        Duration::from_secs(10),
        unix::select(&dir, None, None, None),
    )
    .await
    .expect("il selettore termina con la coda occupata");
    assert!(
        matches!(esito, Err(unix::SelectError::Busy(_))),
        "{esito:?}"
    );
    assert!(endpoint.record_path().exists());
    assert!(socket_path.exists());

    drop(fd);
    let _ = sospeso.join();
    drop(riempimento);
    drop(guard);

    let server = avvia_in(dati.path(), radice.path()).await;
    let scelto = unix::select(&dir, None, None, None).await.unwrap();
    assert_eq!(
        scelto.record.id,
        server.endpoint.identity().to_string(),
        "un tentativo nuovo, dopo la deadline, sceglie l'istanza vera"
    );
    server.task.abort();
    let _ = server.task.await;
}

#[test]
fn la_scoperta_non_segue_un_link_nella_cartella_dati() {
    let dati = cartella_privata("lume-mcp-link-");
    let reale = dati.path().join("reale");
    std::fs::create_dir(&reale).unwrap();
    let link = dati.path().join("link");
    std::os::unix::fs::symlink(&reale, &link).unwrap();
    let errore = unix::ensure_private_dir(&unix::discovery_dir(&link))
        .unwrap_err()
        .to_string();
    assert!(errore.contains("link simbolico"), "{errore}");
}
