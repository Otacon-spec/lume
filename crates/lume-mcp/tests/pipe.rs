#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod comune;

use std::path::PathBuf;
use std::time::Duration;

use comune::{
    Cliente, avvia, avvia_con_timeout, configurazione, contenuto, nome_unico, prossima_ask,
    prossimo_evento, testo_errore, tutto_acceso,
};
use lume_mcp::discovery::{DiscoveryRecord, preamble, record_path};
use lume_mcp::server::{Endpoint, serve};
use lume_mcp::{AppAnswer, AppQuery, Availability, McpCommand, McpEvent, TextSnapshot, Token};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn initialize_e_tools_list_rispondono_sulla_pipe() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = Cliente::connetti(&server).await;
    let risposta = cliente.inizializza("prova").await;
    assert_eq!(risposta["id"], 1);
    assert_eq!(
        risposta["result"]["capabilities"]["tools"]["listChanged"],
        true
    );
    let radice = lume_mcp::confine::canonical(server.radice.path()).unwrap();
    assert_eq!(
        risposta["result"]["serverInfo"]["project_root"],
        radice.display().to_string()
    );
    match prossimo_evento(&mut server).await {
        McpEvent::ClientConnected { name, .. } => assert_eq!(name, "prova"),
        altro => panic!("atteso ClientConnected, non {altro:?}"),
    }
    cliente
        .manda(json!({"jsonrpc": "2.0", "id": "due", "method": "tools/list"}))
        .await;
    let lista = cliente.ricevi().await;
    assert_eq!(lista["id"], "due");
    assert_eq!(
        lista["result"]["tools"].as_array().unwrap().len(),
        lume_mcp::catalog::tools_list(Availability::default())["tools"]
            .as_array()
            .unwrap()
            .len()
    );
}

#[tokio::test]
async fn un_token_sbagliato_chiude_senza_scrivere_un_byte() {
    let server = avvia(Availability::default()).await;
    let (mut r, mut w) = tokio::io::split(comune::apri(&server.nome).await);
    let falso = Token::generate().unwrap();
    let _ = w
        .write_all(format!("{}\n", preamble(&falso)).as_bytes())
        .await;
    let _ = w
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
        .await;
    let mut letto = Vec::new();
    // EOF o pipe rotta: entrambi vanno bene, conta che non arrivi niente
    let _ = tokio::time::timeout(Duration::from_secs(10), r.read_to_end(&mut letto))
        .await
        .expect("il server chiude");
    assert!(
        letto.is_empty(),
        "nessun byte a chi non ha il token: {letto:?}"
    );
}

/// Review Focus: un client muto non tiene occupata un'istanza per sempre.
#[tokio::test]
async fn senza_preambolo_la_connessione_cade_dopo_il_timeout() {
    let server = avvia(Availability::default()).await;
    let mut pipe = comune::apri(&server.nome).await;
    let mut letto = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), pipe.read_to_end(&mut letto))
        .await
        .expect("chiusa dal server dopo 300 ms");
    assert!(letto.is_empty());
}

#[tokio::test]
async fn un_nome_gia_preso_e_failed_senza_ready() {
    let nome = nome_unico();
    let _abusivo = tokio::net::windows::named_pipe::ServerOptions::new()
        .first_pipe_instance(true)
        .create(&nome)
        .unwrap();
    let radice = tempfile::tempdir().unwrap();
    let (tx, mut eventi) = tokio::sync::mpsc::unbounded_channel();
    let token = Token::generate().unwrap();
    serve(
        configurazione(&nome, &token, radice.path(), Availability::default()),
        tx,
    )
    .await;
    assert!(matches!(eventi.recv().await, Some(McpEvent::Failed(_))));
}

#[tokio::test]
async fn il_file_di_scoperta_vive_finche_vive_il_server() {
    let dati = tempfile::tempdir().unwrap();
    let radice = tempfile::tempdir().unwrap();
    let nome = nome_unico();
    let token = Token::generate().unwrap();
    let mut config = configurazione(&nome, &token, radice.path(), Availability::default());
    let Endpoint::Pipe { discovery_dir, .. } = &mut config.endpoint;
    *discovery_dir = Some(dati.path().to_path_buf());
    let (tx, mut eventi) = tokio::sync::mpsc::unbounded_channel();
    let compito = tokio::spawn(serve(config, tx));
    // Il mittente dei comandi resta vivo: lasciarlo cadere vuol dire «l'app
    // è uscita» e il server si chiude prima della connessione di prova.
    let Some(McpEvent::Ready(_comandi)) = eventi.recv().await else {
        panic!("il primo evento deve essere Ready");
    };
    let file = record_path(dati.path(), std::process::id());
    let letto: DiscoveryRecord = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(letto.pipe, nome);
    assert_eq!(letto.token, token);

    // Mantieni una connessione autenticata e inizializzata durante il drop
    // del future proprietario: la JoinSet del server deve chiuderla insieme
    // al listener e alla guardia del file di discovery.
    let client = comune::apri(&nome).await;
    let (reader, mut writer) = tokio::io::split(client);
    let mut reader = BufReader::new(reader);
    writer
        .write_all(format!("{}\n", preamble(&token)).as_bytes())
        .await
        .unwrap();
    writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"clientInfo\":{\"name\":\"teardown\"}}}\n")
        .await
        .unwrap();
    let mut riga = String::new();
    let letti = tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut riga))
        .await
        .expect("risposta initialize entro 10 s")
        .unwrap();
    assert!(letti > 0, "il client è inizializzato prima del teardown");
    writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    assert!(
        matches!(eventi.recv().await, Some(McpEvent::ClientConnected { name, .. }) if name == "teardown")
    );

    compito.abort();
    let _ = compito.await;
    let mut residuo = Vec::new();
    let letti = tokio::time::timeout(
        Duration::from_secs(10),
        reader.get_mut().read_to_end(&mut residuo),
    )
    .await
    .expect("il teardown chiude la connessione appartenente al server")
    .unwrap();
    assert_eq!(letti, 0, "la connessione raggiunge EOF senza byte residui");
    assert!(
        residuo.is_empty(),
        "nessun messaggio dopo il teardown: {residuo:?}"
    );
    assert!(
        !file.exists(),
        "la guardia lo cancella quando lo stream muore"
    );
}

#[tokio::test]
async fn list_files_gira_nello_stream_senza_domandare_all_app() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = Cliente::connetti(&server).await;
    cliente.inizializza("prova").await;
    let _ = prossimo_evento(&mut server).await;
    let risposta = cliente.chiama(2, "list_files", json!({})).await;
    assert_eq!(risposta["result"]["isError"], false);
    assert_eq!(contenuto(&risposta)["files"], json!(["a.txt"]));
    assert!(
        server.eventi.try_recv().is_err(),
        "nessuna domanda all'app per un tool puro"
    );
}

#[tokio::test]
async fn un_percorso_fuori_radice_e_un_errore_del_tool_non_del_protocollo() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = Cliente::connetti(&server).await;
    cliente.inizializza("prova").await;
    let _ = prossimo_evento(&mut server).await;
    let risposta = cliente
        .chiama(3, "read_file", json!({"path": "../fuori.txt"}))
        .await;
    assert_eq!(risposta["result"]["isError"], true);
    assert!(risposta.get("error").is_none());
}

#[tokio::test]
async fn una_domanda_all_app_torna_al_client_con_la_risposta() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = Cliente::connetti(&server).await;
    cliente.inizializza("prova").await;
    let _ = prossimo_evento(&mut server).await;
    cliente
        .manda(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "list_open_files"}}))
        .await;
    let McpEvent::Ask { id, query, .. } = prossimo_evento(&mut server).await else {
        panic!("attesa una Ask")
    };
    assert_eq!(query, AppQuery::OpenFiles);
    server
        .posta
        .send(McpCommand::Answer {
            id,
            answer: Ok(AppAnswer::Json(json!({"files": [], "outside_root": 0}))),
        })
        .unwrap();
    let risposta = cliente.ricevi().await;
    assert_eq!(risposta["id"], 4);
    assert_eq!(contenuto(&risposta)["outside_root"], 0);
}

#[tokio::test]
async fn read_file_preferisce_il_buffer_e_poi_il_disco() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = Cliente::connetti(&server).await;
    cliente.inizializza("prova").await;
    let _ = prossimo_evento(&mut server).await;
    let chiedi = json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "read_file", "arguments": {"path": "a.txt"}}});
    cliente.manda(chiedi.clone()).await;
    let McpEvent::Ask { id, query, .. } = prossimo_evento(&mut server).await else {
        panic!("attesa una Ask")
    };
    assert_eq!(
        query,
        AppQuery::Buffer {
            relative: PathBuf::from("a.txt")
        }
    );
    let buffer = TextSnapshot {
        text: "dal buffer\n".to_owned(),
        dirty: true,
        truncated: false,
    };
    server
        .posta
        .send(McpCommand::Answer {
            id,
            answer: Ok(AppAnswer::Buffer(Some(buffer))),
        })
        .unwrap();
    let dal_buffer = contenuto(&cliente.ricevi().await);
    assert_eq!(dal_buffer["source"], "buffer");
    assert_eq!(dal_buffer["dirty"], true);
    assert_eq!(dal_buffer["text"], "dal buffer\n");

    cliente.manda(chiedi).await;
    let McpEvent::Ask { id, .. } = prossimo_evento(&mut server).await else {
        panic!("attesa una Ask")
    };
    server
        .posta
        .send(McpCommand::Answer {
            id,
            answer: Ok(AppAnswer::Buffer(None)),
        })
        .unwrap();
    let dal_disco = contenuto(&cliente.ricevi().await);
    assert_eq!(dal_disco["source"], "disk");
    assert_eq!(dal_disco["text"], "ciao dal disco\n");
}

#[tokio::test]
async fn tools_changed_manda_list_changed_solo_quando_cambia() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = Cliente::connetti(&server).await;
    cliente.inizializza("prova").await;
    let _ = prossimo_evento(&mut server).await;
    // ClientConnected nasce con `initialize`: un ping sullo stesso stream
    // garantisce che il server abbia letto anche `notifications/initialized`
    // prima del cambio di disponibilità.
    cliente
        .manda(json!({"jsonrpc": "2.0", "id": 5, "method": "ping"}))
        .await;
    assert_eq!(cliente.ricevi().await["id"], 5);
    let git = Availability {
        git: true,
        diagnostics: false,
        ..Availability::default()
    };
    server.posta.send(McpCommand::ToolsChanged(git)).unwrap();
    let notifica = cliente.ricevi().await;
    assert_eq!(notifica["method"], "notifications/tools/list_changed");
    assert!(notifica.get("id").is_none());
    cliente
        .manda(json!({"jsonrpc": "2.0", "id": 6, "method": "tools/list"}))
        .await;
    assert_eq!(
        cliente.ricevi().await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        lume_mcp::catalog::tools_list(git)["tools"]
            .as_array()
            .unwrap()
            .len()
    );
    server.posta.send(McpCommand::ToolsChanged(git)).unwrap();
    cliente
        .manda(json!({"jsonrpc": "2.0", "id": 7, "method": "ping"}))
        .await;
    assert_eq!(
        cliente.ricevi().await["id"],
        7,
        "la stessa disponibilità non produce rumore"
    );
}

#[tokio::test]
async fn una_riga_oltre_il_tetto_chiude_la_connessione() {
    let server = avvia(Availability::default()).await;
    let mut cliente = Cliente::connetti(&server).await;
    cliente.inizializza("prova").await;
    let lunga = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\",\"x\":\"{}\"}}\n",
        "x".repeat(5000)
    );
    let _ = cliente.w.write_all(lunga.as_bytes()).await;
    let mut resto = String::new();
    let letti = tokio::time::timeout(Duration::from_secs(10), cliente.r.read_line(&mut resto))
        .await
        .expect("chiusa entro 10 s")
        .unwrap_or(0);
    assert_eq!(
        letti, 0,
        "nessuna risposta a una riga oltre il tetto: {resto}"
    );
}

/// Review Focus: `max_instances(4)` conta anche l'istanza in ascolto. Con
/// quattro client connessi il quinto trova occupato, il server risponde agli
/// altri e, appena uno se ne va, riapre l'ascolto.
#[tokio::test]
async fn con_quattro_client_il_quinto_trova_occupato_e_gli_altri_rispondono() {
    let mut server = avvia(Availability::default()).await;
    let mut clienti = Vec::new();
    for n in 0..4 {
        let mut cliente = Cliente::connetti(&server).await;
        cliente.inizializza(&format!("c{n}")).await;
        let _ = prossimo_evento(&mut server).await;
        clienti.push(cliente);
    }
    let quinto = tokio::net::windows::named_pipe::ClientOptions::new().open(&server.nome);
    assert!(quinto.is_err(), "nessuna istanza libera oltre le quattro");
    clienti[0]
        .manda(json!({"jsonrpc": "2.0", "id": 9, "method": "ping"}))
        .await;
    assert_eq!(clienti[0].ricevi().await["id"], 9);
    drop(clienti.pop());
    assert!(matches!(
        prossimo_evento(&mut server).await,
        McpEvent::ClientGone(_)
    ));
    let mut nuovo = Cliente::connetti(&server).await;
    assert_eq!(nuovo.inizializza("dopo").await["id"], 1);
}

async fn cliente_pronto(server: &mut comune::Server) -> Cliente {
    let mut cliente = Cliente::connetti(server).await;
    cliente.inizializza("prova").await;
    let _ = prossimo_evento(server).await;
    cliente
}

#[tokio::test]
async fn write_file_chiede_all_app_un_percorso_confinato_e_non_scrive_nello_stream() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = cliente_pronto(&mut server).await;
    cliente
        .manda_chiamata(
            2,
            "write_file",
            json!({"path":"nuovo.txt", "content":"ciao"}),
        )
        .await;
    let (id, _conn, query) = prossima_ask(&mut server).await;
    assert_eq!(
        query,
        AppQuery::WriteFile {
            relative: PathBuf::from("nuovo.txt"),
            content: "ciao".into()
        }
    );
    assert!(!server.radice.path().join("nuovo.txt").exists());
    server
        .posta
        .send(McpCommand::Answer {
            id,
            answer: Ok(AppAnswer::Json(json!({"path":"nuovo.txt"}))),
        })
        .unwrap();
    assert_eq!(contenuto(&cliente.ricevi().await)["path"], "nuovo.txt");
}

#[tokio::test]
async fn scrittura_fuori_radice_e_sotto_git_non_domanda_all_app() {
    let mut server = avvia(Availability::default()).await;
    std::fs::create_dir(server.radice.path().join(".git")).unwrap();
    let mut cliente = cliente_pronto(&mut server).await;
    for (id, path) in [(2, "../fuori"), (3, ".git/config")] {
        let result = cliente
            .chiama(id, "write_file", json!({"path":path,"content":"x"}))
            .await;
        // Errore del tool (`..` o `.git`), non del protocollo, e mai all'app.
        assert_eq!(result["result"]["isError"], true, "{result}");
        assert!(!testo_errore(&result).is_empty(), "{result}");
    }
    assert!(server.eventi.try_recv().is_err());
}

#[tokio::test]
async fn replace_text_convalida_i_parametri_e_inoltra_la_modifica() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = cliente_pronto(&mut server).await;
    let invalid = cliente
        .chiama(
            2,
            "replace_text",
            json!({"path":"a.txt","old_text":"","new_text":"b"}),
        )
        .await;
    assert!(testo_errore(&invalid).contains("old_text"));
    cliente
        .manda_chiamata(
            3,
            "replace_text",
            json!({"path":"a.txt","old_text":"ciao","new_text":"addio","replace_all":true}),
        )
        .await;
    let (id, _, query) = prossima_ask(&mut server).await;
    assert_eq!(
        query,
        AppQuery::ReplaceText {
            relative: PathBuf::from("a.txt"),
            old_text: "ciao".into(),
            new_text: "addio".into(),
            all: true
        }
    );
    server
        .posta
        .send(McpCommand::Answer {
            id,
            answer: Ok(AppAnswer::Json(json!({"replaced":1}))),
        })
        .unwrap();
    assert_eq!(contenuto(&cliente.ricevi().await)["replaced"], 1);
}

#[tokio::test]
async fn due_chiamate_concorrono_e_le_risposte_si_associano_al_loro_id() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = cliente_pronto(&mut server).await;
    cliente
        .manda_chiamata(10, "write_file", json!({"path":"uno","content":"1"}))
        .await;
    cliente
        .manda_chiamata(11, "write_file", json!({"path":"due","content":"2"}))
        .await;
    let (one, _, _) = prossima_ask(&mut server).await;
    let (two, _, _) = prossima_ask(&mut server).await;
    server
        .posta
        .send(McpCommand::Answer {
            id: two,
            answer: Ok(AppAnswer::Json(json!({"value":2}))),
        })
        .unwrap();
    assert_eq!(cliente.ricevi().await["id"], 11);
    server
        .posta
        .send(McpCommand::Answer {
            id: one,
            answer: Ok(AppAnswer::Json(json!({"value":1}))),
        })
        .unwrap();
    assert_eq!(cliente.ricevi().await["id"], 10);
}

#[tokio::test]
async fn cancelled_ritira_la_domanda_e_non_scrive_risposta_tardiva() {
    let mut server = avvia(Availability::default()).await;
    let mut cliente = cliente_pronto(&mut server).await;
    cliente
        .manda_chiamata(20, "write_file", json!({"path":"a.txt","content":"x"}))
        .await;
    let (id, _, _) = prossima_ask(&mut server).await;
    cliente
        .manda(
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":20}}),
        )
        .await;
    assert!(matches!(prossimo_evento(&mut server).await, McpEvent::AskGone(gone) if gone == id));
    cliente
        .manda(json!({"jsonrpc":"2.0","id":21,"method":"ping"}))
        .await;
    assert_eq!(cliente.ricevi().await["id"], 21);
}

#[tokio::test]
async fn timeout_ritira_la_domanda_in_attesa() {
    let mut server = avvia_con_timeout(Availability::default(), Duration::from_millis(20)).await;
    let mut cliente = cliente_pronto(&mut server).await;
    cliente
        .manda_chiamata(2, "write_file", json!({"path":"a.txt","content":"x"}))
        .await;
    let (id, _, _) = prossima_ask(&mut server).await;
    let response = cliente.ricevi().await;
    assert_eq!(response["result"]["isError"], true);
    assert!(matches!(prossimo_evento(&mut server).await, McpEvent::AskGone(gone) if gone == id));
}

#[tokio::test]
async fn rename_inoltra_posizione_e_nome_validato() {
    let mut server = avvia(tutto_acceso()).await;
    let mut cliente = cliente_pronto(&mut server).await;
    cliente
        .manda_chiamata(
            2,
            "rename",
            json!({"path":"a.txt","line":1,"column":2,"new_name":"nuovo"}),
        )
        .await;
    let (_, _, query) = prossima_ask(&mut server).await;
    assert_eq!(
        query,
        AppQuery::Rename {
            relative: PathBuf::from("a.txt"),
            line: 1,
            column: 2,
            new_name: "nuovo".into()
        }
    );
}
