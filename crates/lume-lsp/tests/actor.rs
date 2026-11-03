#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! L'attore contro un server finto **vero**: processo separato, stdio vero,
//! framing vero. È il livello che non si può simulare in memoria — la
//! cancel-safety, il riavvio e il commiato esistono solo con un processo
//! dall'altra parte.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lsp_types::{CompletionResponse, GotoDefinitionResponse, HoverContents, Position, Uri};
use lume_lsp::testing::{Script, run_if_fake_server};
use lume_lsp::{
    ActorEvent, InitializeSpec, LspAnswer, LspCommand, LspCommandMailbox, LspRequest, actor,
};
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// Un file di appoggio che si cancella da sé: i test girano in parallelo e
/// due nomi uguali si mangerebbero le righe a vicenda.
struct TempFile(PathBuf);

impl TempFile {
    fn unique(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("orologio dopo l'epoca")
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "lume-actor-{label}-{}-{nonce}.txt",
            std::process::id()
        )))
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.0)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn attendi_righe(record: &TempFile, expected: &[&str]) {
    let timeout = Duration::from_secs(10);
    let wait = async {
        loop {
            let rows = record.lines();
            if expected
                .iter()
                .all(|item| rows.iter().any(|row| row == item))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    if tokio::time::timeout(timeout, wait).await.is_err() {
        panic!("righe attese {expected:?}, annotate {:?}", record.lines());
    }
}

async fn attendi_prefisso(record: &TempFile, prefix: &str) {
    let wait = async {
        loop {
            if record.lines().iter().any(|line| line.starts_with(prefix)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    if tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .is_err()
    {
        panic!("prefisso {prefix:?} non registrato: {:?}", record.lines());
    }
}

async fn le_richieste_del_server_ricevono_risposta() {
    let record = TempFile::unique("richieste");
    let script = Script::ready()
        .requesting_after(
            "initialized",
            "window/workDoneProgress/create",
            json!({"token":"t"}),
        )
        .requesting_after(
            "initialized",
            "workspace/configuration",
            json!({"items":[{"section":"a"},{"section":"b"}]}),
        )
        .requesting_after("initialized", "metodo/che-non-esiste", json!({}))
        .recording_to(record.path());
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    attendi(&mut events, "Initialized", |event| {
        matches!(event, ActorEvent::Initialized(_))
    })
    .await;
    attendi_righe(
        &record,
        &[
            "response 9000 result null",
            "response 9001 result [null,null]",
            "response 9002 error -32601",
        ],
    )
    .await;
    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    actor.await.expect("l'attore finisce");
}

async fn change_incrementale_utf16_attraversiona_attore_e_protocollo() {
    let record = TempFile::unique("change-incrementale");
    let target = "file:///workspace/src/unicode.rs";
    let script = Script::ready().recording_to(record.path());
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    attendi(&mut events, "Initialized", |event| {
        matches!(event, ActorEvent::Initialized(_))
    })
    .await;
    commands
        .send(LspCommand::Open {
            uri: uri(target),
            language: "rust".into(),
            text: "a😀z\n".into(),
        })
        .expect("mailbox viva");
    commands
        .send(LspCommand::Change {
            uri: uri(target),
            text: "a漢z\n".into(),
        })
        .expect("mailbox viva");
    attendi_prefisso(&record, "params textDocument/didChange ").await;
    let riga = record
        .lines()
        .into_iter()
        .find(|line| line.starts_with("params textDocument/didChange "))
        .expect("parametri didChange registrati");
    let params: serde_json::Value = serde_json::from_str(
        riga.strip_prefix("params textDocument/didChange ")
            .expect("prefisso parametri"),
    )
    .expect("JSON didChange valido");
    assert_eq!(params["textDocument"]["version"], 2);
    assert_eq!(
        params["contentChanges"][0]["range"]["start"]["character"],
        1
    );
    assert_eq!(params["contentChanges"][0]["range"]["end"]["character"], 3);
    assert_eq!(params["contentChanges"][0]["rangeLength"], 2);
    assert_eq!(params["contentChanges"][0]["text"], "漢");

    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    actor.await.expect("attore terminato");
}

async fn raffica_durante_initialize_arriva_come_ultima_revisione() {
    let record = TempFile::unique("change-raffica");
    let target = "file:///workspace/src/burst.rs";
    let script = Script::ready()
        .delaying_after("initialize", 100)
        .recording_to(record.path());
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    commands
        .send(LspCommand::Open {
            uri: uri(target),
            language: "rust".into(),
            text: "fn initial() {}".into(),
        })
        .expect("mailbox viva");
    for revision in 0..1_000 {
        commands
            .send(LspCommand::Change {
                uri: uri(target),
                text: format!("fn revision_{revision}() {{}}"),
            })
            .expect("la coda fonde le revisioni consecutive");
    }
    attendi(&mut events, "Initialized", |event| {
        matches!(event, ActorEvent::Initialized(_))
    })
    .await;
    attendi_prefisso(&record, "params textDocument/didChange ").await;

    let rows = record.lines();
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "textDocument/didChange")
            .count(),
        1,
        "le revisioni in attesa si fondono prima dell'attore"
    );
    let row = rows
        .iter()
        .find(|row| row.starts_with("params textDocument/didChange "))
        .expect("parametri didChange registrati");
    let params: serde_json::Value = serde_json::from_str(
        row.strip_prefix("params textDocument/didChange ")
            .expect("prefisso parametri"),
    )
    .expect("JSON didChange valido");
    assert_eq!(params["textDocument"]["version"], 2);
    assert_eq!(params["contentChanges"][0]["text"], "revision_999");

    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    actor.await.expect("attore terminato");
}

async fn server_lento_sui_did_change_riceve_l_ultima_revisione_accodata() {
    let record = TempFile::unique("change-server-lento");
    let target = "file:///workspace/src/slow.rs";
    let text_for = |revision: usize| {
        let offset = u8::try_from(revision % 26).expect("resto entro l'alfabeto");
        format!(
            "r{revision:04}{}",
            char::from(b'a' + offset).to_string().repeat(8 * 1024)
        )
    };
    let script = Script::ready_with(json!({ "textDocumentSync": 1 }))
        .delaying_once_after("textDocument/didChange", 3_000)
        .recording_to(record.path());
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    attendi(&mut events, "Initialized", |event| {
        matches!(event, ActorEvent::Initialized(_))
    })
    .await;
    commands
        .send(LspCommand::Open {
            uri: uri(target),
            language: "rust".into(),
            text: "initiale".into(),
        })
        .expect("mailbox viva");
    commands
        .send(LspCommand::Change {
            uri: uri(target),
            text: text_for(0),
        })
        .expect("mailbox viva");
    attendi_prefisso(&record, "params textDocument/didChange ").await;

    for revision in 1..1_000 {
        commands
            .send(LspCommand::Change {
                uri: uri(target),
                text: text_for(revision),
            })
            .expect("la mailbox mantiene la revisione più recente");
    }

    let final_marker = "\"text\":\"r0999";
    let wait_final = async {
        loop {
            let rows = record.lines();
            if rows.iter().any(|row| row.contains(final_marker)) {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let rows = tokio::time::timeout(Duration::from_secs(10), wait_final)
        .await
        .unwrap_or_else(|_| {
            let rows = record.lines();
            let mut actor_events = Vec::new();
            while let Ok(event) = events.try_recv() {
                actor_events.push(format!("{event:?}"));
            }
            let payloads: Vec<_> = rows
                .iter()
                .filter_map(|row| row.strip_prefix("params textDocument/didChange "))
                .filter_map(|row| serde_json::from_str::<serde_json::Value>(row).ok())
                .filter_map(|params| {
                    params["contentChanges"][0]["text"]
                        .as_str()
                        .map(|text| text.chars().take(16).collect::<String>())
                })
                .collect();
            panic!(
                "il server lento riceve l'ultima revisione: {} righe, actor_finished={}, eventi={:?}, payloads={:?}, ultime {:?}",
                rows.len(),
                actor.is_finished(),
                actor_events,
                payloads,
                rows.iter()
                    .rev()
                    .take(5)
                    .map(|row| &row[..row.len().min(120)])
                    .collect::<Vec<_>>()
            );
        });
    let changes: Vec<_> = rows
        .iter()
        .filter(|row| row.as_str() == "textDocument/didChange")
        .collect();
    assert!(
        changes.len() < 1_000,
        "la contropressione deve fondere le revisioni non ancora inviate"
    );
    let last = rows
        .iter()
        .rev()
        .find(|row| row.starts_with("params textDocument/didChange "))
        .expect("parametri didChange registrati");
    assert!(
        last.contains(final_marker),
        "la coda consegna il delta della revisione più recente"
    );

    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    actor.await.expect("attore terminato");
}

fn uri(value: &str) -> Uri {
    value.parse().expect("URI valida")
}

fn diagnostics_for(target: &str) -> serde_json::Value {
    json!({
        "uri": target,
        "diagnostics": [{
            "range": {
                "start": { "line": 0, "character": 3 },
                "end": { "line": 0, "character": 7 }
            },
            "severity": 1,
            "message": "cannot find function `mian`"
        }]
    })
}

async fn attendi(
    events: &mut UnboundedReceiver<ActorEvent>,
    quale: &str,
    predicato: impl Fn(&ActorEvent) -> bool,
) -> ActorEvent {
    let scadenza = Duration::from_secs(10);
    let attesa = async {
        loop {
            let evento = events
                .recv()
                .await
                .expect("l'attore non ha chiuso il canale");
            if predicato(&evento) {
                return evento;
            }
            if let ActorEvent::Failed(reason) = evento {
                panic!("atteso {quale}, l'attore ha fallito: {reason}");
            }
        }
    };
    tokio::time::timeout(scadenza, attesa)
        .await
        .unwrap_or_else(|_| panic!("nessun {quale} entro {scadenza:?}"))
}

/// Il giro che conta: si apre un documento, il server spinge una diagnostica,
/// l'attore la consegna. È il filo che toglie il mock al dock Problemi.
async fn il_progresso_del_server_arriva_a_chi_ascolta() {
    let token = json!("indexing");
    let script = Script::ready()
        .pushing_after(
            "initialized",
            "$/progress",
            json!({"token":token,"value":{"kind":"begin","title":"Indexing"}}),
        )
        .pushing_after(
            "initialized",
            "$/progress",
            json!({"token":token,"value":{"kind":"report","percentage":50}}),
        )
        .pushing_after(
            "initialized",
            "$/progress",
            json!({"token":token,"value":{"kind":"end"}}),
        );
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    let first = attendi(&mut events, "Progress", |e| {
        matches!(e, ActorEvent::Progress(_) | ActorEvent::Failed(_))
    })
    .await;
    let ActorEvent::Progress(begin) = first else {
        panic!("atteso progress, ricevuto {first:?}");
    };
    assert!(matches!(
        begin.value,
        lsp_types::ProgressParamsValue::WorkDone(lsp_types::WorkDoneProgress::Begin(_))
    ));
    let last = attendi(&mut events, "Progress di fine", |e| matches!(e, ActorEvent::Progress(p) if matches!(p.value, lsp_types::ProgressParamsValue::WorkDone(lsp_types::WorkDoneProgress::End(_))))).await;
    assert!(matches!(last, ActorEvent::Progress(_)));
    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    actor.await.expect("l'attore finisce");
}

async fn una_diagnostica_spinta_arriva_a_chi_ascolta() {
    let record = TempFile::unique("diagnostica");
    let target = "file:///workspace/src/main.rs";
    let script = Script::ready()
        .pushing_after(
            "textDocument/didOpen",
            "textDocument/publishDiagnostics",
            diagnostics_for(target),
        )
        .recording_to(record.path());

    let (comandi, rx_comandi) = LspCommandMailbox::channel();
    let (tx_eventi, mut eventi) = unbounded_channel();
    let attore = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_comandi,
        tx_eventi,
    ));

    attendi(&mut eventi, "Initialized", |evento| {
        matches!(evento, ActorEvent::Initialized(_))
    })
    .await;

    comandi
        .send(LspCommand::Open {
            uri: uri(target),
            language: "rust".into(),
            text: "fn mian() {}".into(),
        })
        .expect("mailbox viva");

    let evento = attendi(&mut eventi, "Diagnostics", |evento| {
        matches!(evento, ActorEvent::Diagnostics(_))
    })
    .await;
    let ActorEvent::Diagnostics(params) = evento else {
        unreachable!()
    };
    assert_eq!(params.uri.as_str(), target);
    assert_eq!(params.diagnostics.len(), 1);
    assert_eq!(params.diagnostics[0].message, "cannot find function `mian`");

    comandi.send(LspCommand::Shutdown).expect("mailbox viva");
    attore.await.expect("l'attore finisce");
    let visti = record.lines();
    assert!(visti.contains(&"shutdown".to_owned()), "visti: {visti:?}");
    assert!(visti.contains(&"exit".to_owned()), "visti: {visti:?}");
}

/// ATTENZIONE: La prova della cancel-safety. Il server cade **sul primo `didChange`**,
/// e nello stesso istante arrivano altri comandi: se il riavvio stesse dentro
/// un `select!` verrebbe abbandonato a metà e la sessione resterebbe ferma a
/// `Initializing` per sempre. Qui invece il riavvio finisce, i `didOpen` si
/// rigiocano, e il documento che arriva **dopo** viene accettato — che è
/// esattamente il modo di accorgersi che la sessione è ancora viva.
async fn un_crollo_non_lascia_la_sessione_a_meta() {
    let record = TempFile::unique("crollo");
    let target = "file:///workspace/src/lib.rs";
    let script = Script::ready()
        .dying_after("textDocument/didChange", 41)
        .recording_to(record.path());

    let (comandi, rx_comandi) = LspCommandMailbox::channel();
    let (tx_eventi, mut eventi) = unbounded_channel();
    let attore = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_comandi,
        tx_eventi,
    ));
    attendi(&mut eventi, "Initialized", |evento| {
        matches!(evento, ActorEvent::Initialized(_))
    })
    .await;

    comandi
        .send(LspCommand::Open {
            uri: uri(target),
            language: "rust".into(),
            text: "pub fn a() {}".into(),
        })
        .expect("mailbox viva");
    comandi
        .send(LspCommand::Change {
            uri: uri(target),
            text: "pub fn b() {}".into(),
        })
        .expect("mailbox viva");
    // la raffica che, col vecchio contratto, avrebbe fatto vincere il comando
    // al `select!` e abbandonato il riavvio dentro `negotiate`
    for _ in 0..8 {
        comandi
            .send(LspCommand::Change {
                uri: uri(target),
                text: "pub fn c() {}".into(),
            })
            .expect("mailbox viva");
    }

    attendi(
        &mut eventi,
        "avviso di riavvio",
        |evento| matches!(evento, ActorEvent::Log(riga) if riga.contains("riavvio")),
    )
    .await;

    // il documento nuovo passa solo se la sessione è tornata `Ready`: se il
    // riavvio si fosse perso, questo darebbe un `Log` di `NotReady`
    let secondo = "file:///workspace/src/altro.rs";
    comandi
        .send(LspCommand::Open {
            uri: uri(secondo),
            language: "rust".into(),
            text: "pub fn d() {}".into(),
        })
        .expect("mailbox viva");
    comandi.send(LspCommand::Shutdown).expect("mailbox viva");
    attore.await.expect("l'attore finisce");

    let visti = record.lines();
    let inizializzazioni = visti.iter().filter(|riga| *riga == "initialize").count();
    assert_eq!(
        inizializzazioni, 2,
        "un riavvio, non zero e non due: {visti:?}"
    );
    let aperture = visti
        .iter()
        .filter(|riga| *riga == "textDocument/didOpen")
        .count();
    assert!(
        aperture >= 3,
        "il rigioco dei didOpen più quello nuovo: {visti:?}"
    );
    assert!(visti.contains(&"exit".to_owned()), "visti: {visti:?}");
}

/// ATTENZIONE: Una domanda deve **attraversare un riavvio**, e questo test esiste per
/// quello. Il cliente delle richieste porta dentro il trasporto del processo
/// corrente: tenerlo in caldo vorrebbe dire, dopo un crollo, mandare la
/// domanda in un tubo chiuso e rispondere «disconnesso» a proposito di un
/// server che nel frattempo è tornato vivo — un «vai alla definizione» che
/// smette di funzionare per sempre dopo il primo crollo, senza che niente lo
/// dica. Qui il server muore sul `didChange`, e la domanda che arriva dopo
/// riceve una risposta vera.
async fn una_domanda_attraversa_un_riavvio() {
    let target = "file:///workspace/src/lib.rs";
    let script = Script::ready_with(json!({ "definitionProvider": true }))
        .with_response(
            "textDocument/definition",
            json!({
                "uri": target,
                "range": {
                    "start": { "line": 3, "character": 4 },
                    "end": { "line": 3, "character": 9 }
                }
            }),
        )
        .dying_after("textDocument/didChange", 41);

    let (comandi, rx_comandi) = LspCommandMailbox::channel();
    let (tx_eventi, mut eventi) = unbounded_channel();
    let attore = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_comandi,
        tx_eventi,
    ));
    attendi(&mut eventi, "Initialized", |evento| {
        matches!(evento, ActorEvent::Initialized(_))
    })
    .await;

    comandi
        .send(LspCommand::Open {
            uri: uri(target),
            language: "rust".into(),
            text: "pub fn a() {}".into(),
        })
        .expect("mailbox viva");
    comandi
        .send(LspCommand::Change {
            uri: uri(target),
            text: "pub fn b() {}".into(),
        })
        .expect("mailbox viva");
    attendi(
        &mut eventi,
        "avviso di riavvio",
        |evento| matches!(evento, ActorEvent::Log(riga) if riga.contains("riavvio")),
    )
    .await;

    comandi
        .send(LspCommand::Ask {
            id: 7,
            request: LspRequest::Definition {
                uri: uri(target),
                position: Position::new(0, 7),
            },
        })
        .expect("mailbox viva");

    let evento = attendi(&mut eventi, "Answer", |evento| {
        matches!(evento, ActorEvent::Answer(..))
    })
    .await;
    let ActorEvent::Answer(id, esito) = evento else {
        unreachable!()
    };
    assert_eq!(id, 7, "la risposta porta l'id di chi ha chiesto");
    match *esito {
        Ok(LspAnswer::Definition(Some(GotoDefinitionResponse::Scalar(location)))) => {
            assert_eq!(location.uri.as_str(), target);
            assert_eq!(location.range.start.line, 3);
        }
        altro => panic!("attesa una definizione, visto {altro:?}"),
    }

    comandi.send(LspCommand::Shutdown).expect("mailbox viva");
    attore.await.expect("l'attore finisce");
}

/// Una capability che il server non dichiara non diventa una richiesta: si
/// risponde di no **senza scrivere niente sul filo**, che è la stessa regola
/// di `MENUS` in `menu.rs` un piano più sotto.
async fn una_capability_non_dichiarata_non_diventa_una_richiesta() {
    let record = TempFile::unique("capability");
    let script = Script::ready().recording_to(record.path());
    let (comandi, rx_comandi) = LspCommandMailbox::channel();
    let (tx_eventi, mut eventi) = unbounded_channel();
    let attore = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_comandi,
        tx_eventi,
    ));
    attendi(&mut eventi, "Initialized", |evento| {
        matches!(evento, ActorEvent::Initialized(_))
    })
    .await;

    comandi
        .send(LspCommand::Ask {
            id: 1,
            request: LspRequest::Definition {
                uri: uri("file:///workspace/src/main.rs"),
                position: Position::new(0, 0),
            },
        })
        .expect("mailbox viva");
    let evento = attendi(&mut eventi, "Answer", |evento| {
        matches!(evento, ActorEvent::Answer(..))
    })
    .await;
    let ActorEvent::Answer(_, esito) = evento else {
        unreachable!()
    };
    assert!(esito.is_err(), "atteso un no, visto {esito:?}");

    comandi.send(LspCommand::Shutdown).expect("mailbox viva");
    attore.await.expect("l'attore finisce");
    let visti = record.lines();
    assert!(
        !visti.contains(&"textDocument/definition".to_owned()),
        "non doveva partire: {visti:?}"
    );
}

/// Chiudere la mailbox è un ordine di spegnimento come un altro: è il modo in
/// cui iced lo dice quando droppa lo stream, e restare vivi vorrebbe dire un
/// processo che nessuno può più fermare.
async fn buttare_via_la_mailbox_spegne_il_server() {
    let record = TempFile::unique("mailbox");
    let script = Script::ready().recording_to(record.path());
    let (comandi, rx_comandi) = LspCommandMailbox::channel();
    let (tx_eventi, mut eventi) = unbounded_channel();
    let attore = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_comandi,
        tx_eventi,
    ));
    attendi(&mut eventi, "Initialized", |evento| {
        matches!(evento, ActorEvent::Initialized(_))
    })
    .await;

    drop(comandi);
    attore.await.expect("l'attore finisce");
    let uscita = attendi(&mut eventi, "ProcessExited", |evento| {
        matches!(evento, ActorEvent::ProcessExited { .. })
    })
    .await;
    assert_eq!(
        uscita,
        ActorEvent::ProcessExited {
            success: true,
            code: Some(0)
        }
    );
    attendi(&mut eventi, "Stopped", |evento| {
        matches!(evento, ActorEvent::Stopped)
    })
    .await;
    assert!(record.lines().contains(&"exit".to_owned()));
}

/// Un eseguibile che non c'è non è un motivo per andare in panico: è una riga
/// spenta con scritto perché.
async fn un_comando_che_non_esiste_diventa_un_fallimento_detto() {
    let mut command = Script::ready().command();
    command.program = PathBuf::from("questo-server-non-esiste-davvero");
    let (_comandi, rx_comandi) = LspCommandMailbox::channel();
    let (tx_eventi, mut eventi) = unbounded_channel();
    actor::run(command, InitializeSpec::default(), rx_comandi, tx_eventi).await;
    let evento = eventi.recv().await.expect("un evento c'è");
    assert!(
        matches!(evento, ActorEvent::Failed(_)),
        "atteso Failed, visto {evento:?}"
    );
}

/// Il giro vero del completamento: richiesta sul filo, risposta con voci e
/// dettagli, consegnata con l'id di chi ha chiesto.
async fn un_completamento_attraversa_l_attore() {
    let target = "file:///workspace/src/lib.rs";
    let script = Script::ready_with(json!({
        "completionProvider": { "triggerCharacters": ["."] }
    }))
    .with_response(
        "textDocument/completion",
        json!({
            "isIncomplete": false,
            "items": [
                { "label": "push", "detail": "fn(&mut self, T)", "insertText": "push" },
                { "label": "pop" }
            ]
        }),
    );
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    attendi(&mut events, "Initialized", |event| {
        matches!(event, ActorEvent::Initialized(_))
    })
    .await;

    commands
        .send(LspCommand::Ask {
            id: 11,
            request: LspRequest::Completion {
                uri: uri(target),
                position: Position::new(0, 3),
            },
        })
        .expect("mailbox viva");
    let event = attendi(&mut events, "Answer", |event| {
        matches!(event, ActorEvent::Answer(..))
    })
    .await;
    let ActorEvent::Answer(id, result) = event else {
        unreachable!()
    };
    assert_eq!(id, 11);
    match *result {
        Ok(LspAnswer::Completion(Some(CompletionResponse::List(list)))) => {
            assert_eq!(list.items.len(), 2);
            assert_eq!(list.items[0].label, "push");
            assert_eq!(list.items[0].detail.as_deref(), Some("fn(&mut self, T)"));
        }
        other => panic!("attesa una lista di completamenti, visto {other:?}"),
    }
    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    actor.await.expect("l'attore finisce");
}

/// Il giro vero dell'hover, con testo Markdown.
async fn un_hover_attraversa_l_attore() {
    let script = Script::ready_with(json!({ "hoverProvider": true })).with_response(
        "textDocument/hover",
        json!({ "contents": { "kind": "markdown", "value": "```rust\nfn a()\n```" } }),
    );
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    attendi(&mut events, "Initialized", |event| {
        matches!(event, ActorEvent::Initialized(_))
    })
    .await;

    commands
        .send(LspCommand::Ask {
            id: 12,
            request: LspRequest::Hover {
                uri: uri("file:///workspace/src/lib.rs"),
                position: Position::new(0, 4),
            },
        })
        .expect("mailbox viva");
    let event = attendi(&mut events, "Answer", |event| {
        matches!(event, ActorEvent::Answer(..))
    })
    .await;
    let ActorEvent::Answer(id, result) = event else {
        unreachable!()
    };
    assert_eq!(id, 12);
    match *result {
        Ok(LspAnswer::Hover(Some(hover))) => match hover.contents {
            HoverContents::Markup(content) => assert!(content.value.contains("fn a()")),
            other => panic!("atteso Markdown, visto {other:?}"),
        },
        other => panic!("atteso un hover, visto {other:?}"),
    }
    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    actor.await.expect("l'attore finisce");
}

/// Senza la capability, né completamento né hover partono: niente sul filo.
async fn completamento_e_hover_senza_capability_non_partono() {
    let record = TempFile::unique("senza-completamento");
    let script = Script::ready().recording_to(record.path());
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    attendi(&mut events, "Initialized", |event| {
        matches!(event, ActorEvent::Initialized(_))
    })
    .await;
    for (id, request) in [
        (
            21,
            LspRequest::Completion {
                uri: uri("file:///workspace/src/main.rs"),
                position: Position::new(0, 0),
            },
        ),
        (
            22,
            LspRequest::Hover {
                uri: uri("file:///workspace/src/main.rs"),
                position: Position::new(0, 0),
            },
        ),
    ] {
        commands
            .send(LspCommand::Ask { id, request })
            .expect("mailbox viva");
        let event = attendi(&mut events, "Answer", |event| {
            matches!(event, ActorEvent::Answer(..))
        })
        .await;
        let ActorEvent::Answer(_, result) = event else {
            unreachable!()
        };
        assert!(result.is_err(), "atteso un no per {id}, visto {result:?}");
    }
    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    actor.await.expect("l'attore finisce");
    let seen = record.lines();
    assert!(
        !seen.iter().any(|method| {
            method == "textDocument/completion" || method == "textDocument/hover"
        }),
        "non dovevano partire: {seen:?}"
    );
}

/// Un'uscita non-zero durante il commiato resta distinguibile da uno stop pulito.
async fn l_uscita_non_zero_del_server_non_e_uno_stop_pulito() {
    let script = Script::ready().dying_after("exit", 17);
    let (commands, rx_commands) = LspCommandMailbox::channel();
    let (tx_events, mut events) = unbounded_channel();
    let actor = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx_commands,
        tx_events,
    ));
    attendi(&mut events, "Initialized", |event| {
        matches!(event, ActorEvent::Initialized(_))
    })
    .await;
    commands.send(LspCommand::Shutdown).expect("mailbox viva");
    let event = attendi(&mut events, "ProcessExited", |event| {
        matches!(event, ActorEvent::ProcessExited { .. })
    })
    .await;
    assert_eq!(
        event,
        ActorEvent::ProcessExited {
            success: false,
            code: Some(17)
        }
    );
    attendi(&mut events, "Stopped", |event| {
        matches!(event, ActorEvent::Stopped)
    })
    .await;
    actor.await.expect("attore terminato");
}

async fn snapshot_ask_snapshot_conservano_ordine_sul_filo() {
    let record = TempFile::unique("snapshot-ask-ordine");
    let script = Script::ready_with(json!({"hoverProvider": true}))
        .with_response("textDocument/hover", json!({"contents":"A"}))
        .delaying_after("initialize", 100)
        .recording_to(record.path());
    let (commands, rx) = LspCommandMailbox::channel();
    let (tx, mut events) = unbounded_channel();
    let target = uri("file:///workspace/src/order.rs");
    commands
        .send(LspCommand::Open {
            uri: target.clone(),
            language: "rust".into(),
            text: "initial".into(),
        })
        .unwrap();
    commands
        .send(LspCommand::ChangeSnapshot {
            uri: target.clone(),
            text: ropey::Rope::from_str("A"),
        })
        .unwrap();
    commands
        .send(LspCommand::Ask {
            id: 901,
            request: LspRequest::Hover {
                uri: target.clone(),
                position: Position::new(0, 0),
            },
        })
        .unwrap();
    commands
        .send(LspCommand::ChangeSnapshot {
            uri: target,
            text: ropey::Rope::from_str("B"),
        })
        .unwrap();
    let owner = tokio::spawn(actor::run(
        script.command(),
        InitializeSpec::default(),
        rx,
        tx,
    ));
    attendi(&mut events, "Answer 901", |e| {
        matches!(e, ActorEvent::Answer(901, _))
    })
    .await;
    commands.send(LspCommand::Shutdown).unwrap();
    owner.await.unwrap();
    let methods: Vec<_> = record
        .lines()
        .into_iter()
        .filter(|s| s == "textDocument/didChange" || s == "textDocument/hover")
        .collect();
    assert_eq!(
        methods,
        [
            "textDocument/didChange",
            "textDocument/hover",
            "textDocument/didChange"
        ],
        "Ask deve partire prima dello snapshot B"
    );
}

#[tokio::main]
async fn main() {
    if run_if_fake_server() {
        return;
    }
    snapshot_ask_snapshot_conservano_ordine_sul_filo().await;
    eprintln!("actor::snapshot_ask_ordine ... ok");
    le_richieste_del_server_ricevono_risposta().await;
    eprintln!("actor::risposte_al_server ... ok");
    change_incrementale_utf16_attraversiona_attore_e_protocollo().await;
    eprintln!("actor::change_incrementale_utf16 ... ok");
    raffica_durante_initialize_arriva_come_ultima_revisione().await;
    eprintln!("actor::raffica_durante_initialize ... ok");
    server_lento_sui_did_change_riceve_l_ultima_revisione_accodata().await;
    eprintln!("actor::server_lento_did_change ... ok");
    il_progresso_del_server_arriva_a_chi_ascolta().await;
    eprintln!("actor::progresso ... ok");
    una_diagnostica_spinta_arriva_a_chi_ascolta().await;
    eprintln!("actor::diagnostica_spinta ... ok");
    un_crollo_non_lascia_la_sessione_a_meta().await;
    eprintln!("actor::riavvio_non_interrotto ... ok");
    una_domanda_attraversa_un_riavvio().await;
    eprintln!("actor::domanda_dopo_riavvio ... ok");
    una_capability_non_dichiarata_non_diventa_una_richiesta().await;
    eprintln!("actor::capability_assente ... ok");
    buttare_via_la_mailbox_spegne_il_server().await;
    eprintln!("actor::mailbox_chiusa_spegne ... ok");
    un_comando_che_non_esiste_diventa_un_fallimento_detto().await;
    eprintln!("actor::eseguibile_assente ... ok");
    un_completamento_attraversa_l_attore().await;
    eprintln!("actor::completamento ... ok");
    un_hover_attraversa_l_attore().await;
    eprintln!("actor::hover ... ok");
    completamento_e_hover_senza_capability_non_partono().await;
    eprintln!("actor::completamento_hover_senza_capability ... ok");
    l_uscita_non_zero_del_server_non_e_uno_stop_pulito().await;
    eprintln!("actor::uscita_non_zero ... ok");
}
