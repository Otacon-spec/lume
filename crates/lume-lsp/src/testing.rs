//! Un server di prova **scriptabile**, dietro la feature `testing`.
//!
//! Il banco di prova che c'era — dentro `tests/session.rs` — rispondeva
//! `Null` a tutto tranne `initialize`, e soprattutto non sapeva **spingere**
//! niente: nessun `publishDiagnostics` dopo un `didOpen`, quindi nessun modo
//! di provare che il dock Problemi si accenda davvero. Qui il copione è un
//! dato:
//!
//! - `responses`: metodo → risultato già pronto, per le richieste;
//! - `push_after`: visto **questo** metodo, manda **quella** notifica;
//! - `push_requests`: visto **questo** metodo, manda una richiesta al client;
//! - `exit_after`: visto questo metodo, muori — è il crollo del processo.
//!
//! Non è un linguaggio di scripting, ed è deliberato: i passi 2-5 della Fase
//! 6 aggiungono righe alla tabella, non capacità al server.
//!
//! Il copione viaggia **come JSON dentro una variabile d'ambiente**: nessun
//! file temporaneo da ripulire, e `ServerProcess` eredita l'ambiente e ci
//! sovrascrive sopra (`process.rs::make_environment`), quindi arriva intero.

// Modulo di sole prove, dietro una feature che il binario non accende mai:
// qui un eseguibile irraggiungibile è un guasto dell'ambiente di test, e
// farlo esplodere subito dice più di un `Option` che qualcuno ignorerà.
#![allow(clippy::expect_used)]

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CommandSource, FrameDecoder, RpcMessage, ServerCommand, encode_message};

/// La variabile che porta il copione.
pub const SCRIPT_ENV: &str = "LUME_FAKE_LSP_SCRIPT";
/// L'argomento che dice all'eseguibile «oggi sei il server».
pub const SERVER_ARG: &str = "--lume-fake-lsp";

/// Cosa fa il server finto, e quando.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Script {
    /// metodo → risultato della risposta. Chi non è in tabella riceve `null`.
    pub responses: Vec<(String, Value)>,
    /// visto il metodo, spingi la notifica `(metodo, params)`.
    pub push_after: Vec<(String, String, Value)>,
    /// visto il metodo, manda al client la richiesta (trigger, metodo, parametri).
    #[serde(default)]
    pub push_requests: Vec<(String, String, Value)>,
    /// visto il metodo, esci con questo codice: il processo che crolla.
    pub exit_after: Option<(String, i32)>,
    /// dove annotare i metodi ricevuti, uno per riga. Serve a provare che una
    /// cosa è **arrivata**, non solo che non ha rotto niente.
    pub record: Option<String>,
    /// metodo → millisecondi di pausa prima della risposta/continuazione.
    #[serde(default)]
    pub delay_after: Vec<(String, u64)>,
    /// metodo → pausa soltanto sulla prima occorrenza.
    #[serde(default)]
    pub delay_once_after: Vec<(String, u64)>,
}

impl Script {
    /// Un server che si comporta come rust-analyzer sul minimo indispensabile:
    /// annuncia la sincronizzazione incrementale e risponde a tutto il resto.
    pub fn ready() -> Self {
        Self::ready_with(json!({}))
    }

    /// Come [`Script::ready`], con capability in più.
    ///
    /// ATTENZIONE: Serve dal passo 2 in poi: `FeatureClient` **rifiuta** una richiesta
    /// che il server non ha dichiarato, e la rifiuta *prima* di mandarla. Un
    /// server finto muto sulle capability farebbe fallire la prova con
    /// «capability non disponibile» invece di provare il giro vero.
    pub fn ready_with(mut extra: Value) -> Self {
        let mut capabilities = json!({ "textDocumentSync": 2 });
        if let (Some(base), Some(extra)) = (capabilities.as_object_mut(), extra.as_object_mut()) {
            base.append(extra);
        }
        Self {
            responses: vec![(
                "initialize".to_owned(),
                json!({ "capabilities": capabilities }),
            )],
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_response(mut self, method: impl Into<String>, result: Value) -> Self {
        self.responses.push((method.into(), result));
        self
    }

    /// Dopo `trigger`, manda la notifica `method` con quei parametri.
    #[must_use]
    pub fn pushing_after(
        mut self,
        trigger: impl Into<String>,
        method: impl Into<String>,
        params: Value,
    ) -> Self {
        self.push_after
            .push((trigger.into(), method.into(), params));
        self
    }

    #[must_use]
    pub fn requesting_after(
        mut self,
        trigger: impl Into<String>,
        method: impl Into<String>,
        params: Value,
    ) -> Self {
        self.push_requests
            .push((trigger.into(), method.into(), params));
        self
    }

    #[must_use]
    pub fn dying_after(mut self, trigger: impl Into<String>, code: i32) -> Self {
        self.exit_after = Some((trigger.into(), code));
        self
    }

    #[must_use]
    pub fn recording_to(mut self, path: impl Into<String>) -> Self {
        self.record = Some(path.into());
        self
    }

    #[must_use]
    pub fn delaying_after(mut self, method: impl Into<String>, millis: u64) -> Self {
        self.delay_after.push((method.into(), millis));
        self
    }

    #[must_use]
    pub fn delaying_once_after(mut self, method: impl Into<String>, millis: u64) -> Self {
        self.delay_once_after.push((method.into(), millis));
        self
    }

    /// Il comando che avvia **questo** eseguibile come server finto.
    ///
    /// # Panics
    /// Se l'eseguibile corrente non è raggiungibile: in un test è un guasto
    /// dell'ambiente, non una condizione da gestire.
    pub fn command(&self) -> ServerCommand {
        let copione = serde_json::to_string(self).unwrap_or_default();
        ServerCommand {
            program: std::env::current_exe().expect("eseguibile di prova"),
            args: vec![OsString::from(SERVER_ARG)],
            cwd: None,
            env: vec![(OsString::from(SCRIPT_ENV), OsString::from(copione))],
            source: CommandSource::Bundled,
        }
    }
}

/// Se questo processo è stato avviato come server finto, **fa il server** e
/// restituisce `true`. Va chiamata per prima cosa nel `main` di un test con
/// `harness = false`.
pub fn run_if_fake_server() -> bool {
    let Some(copione) = std::env::var_os(SCRIPT_ENV) else {
        return false;
    };
    if !std::env::args().any(|argomento| argomento == SERVER_ARG) {
        return false;
    }
    let script: Script = copione
        .to_str()
        .and_then(|testo| serde_json::from_str(testo).ok())
        .unwrap_or_default();
    serve(&script);
    true
}

fn annota(script: &Script, metodo: &str) {
    let Some(percorso) = &script.record else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(percorso)
    {
        let _ = writeln!(file, "{metodo}");
    }
}

fn annota_parametri(script: &Script, metodo: &str, params: &Value) {
    if metodo == "textDocument/didChange"
        && let Ok(serializzati) = serde_json::to_string(params)
    {
        annota(script, &format!("params {metodo} {serializzati}"));
    }
}

#[allow(clippy::indexing_slicing)] // read <= buffer.len() è il contratto di `read`
fn serve(script: &Script) {
    let mut decoder = FrameDecoder::default();
    let mut delayed_once = HashSet::new();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut buffer = [0_u8; 4096];
    loop {
        let Ok(read) = input.read(&mut buffer) else {
            return;
        };
        if read == 0 {
            return;
        }
        let Ok(frames) = decoder.push(&buffer[..read]) else {
            return;
        };
        for frame in frames {
            let Ok(message) = serde_json::from_slice::<RpcMessage>(&frame) else {
                continue;
            };
            let (method, response_id, params) = match message {
                RpcMessage::Request(request) => (request.method, Some(request.id), None),
                RpcMessage::Notification(notification) => {
                    (notification.method, None, Some(notification.params))
                }
                RpcMessage::Response(response) => {
                    let line = match &response.payload {
                        crate::RpcResponsePayload::Result(value) => {
                            format!("response {} result {value}", response.id.0)
                        }
                        crate::RpcResponsePayload::Error(error) => {
                            format!("response {} error {}", response.id.0, error.code)
                        }
                    };
                    annota(script, &line);
                    continue;
                }
            };
            annota(script, &method);
            if let Some(params) = &params {
                annota_parametri(script, &method, params);
            }
            if let Some((_, millis)) = script
                .delay_after
                .iter()
                .find(|(trigger, _)| trigger == &method)
            {
                std::thread::sleep(Duration::from_millis(*millis));
            }
            if let Some((_, millis)) = script
                .delay_once_after
                .iter()
                .find(|(trigger, _)| trigger == &method)
                && delayed_once.insert(method.clone())
            {
                std::thread::sleep(Duration::from_millis(*millis));
            }
            if let Some((trigger, code)) = &script.exit_after
                && trigger == &method
            {
                std::process::exit(*code);
            }
            if let Some(id) = response_id {
                let result = script
                    .responses
                    .iter()
                    .find(|(nome, _)| nome == &method)
                    .map_or(Value::Null, |(_, valore)| valore.clone());
                let Ok(bytes) = encode_message(&RpcMessage::success(id, result)) else {
                    return;
                };
                if output.write_all(&bytes).is_err() || output.flush().is_err() {
                    return;
                }
            }
            for (indice, (trigger, requested_method, params)) in
                script.push_requests.iter().enumerate()
            {
                if trigger != &method {
                    continue;
                }
                let id = 9000 + i64::try_from(indice).unwrap_or(0);
                let Ok(bytes) = encode_message(&RpcMessage::request(
                    id,
                    requested_method.clone(),
                    params.clone(),
                )) else {
                    return;
                };
                if output.write_all(&bytes).is_err() || output.flush().is_err() {
                    return;
                }
            }
            for (trigger, notifica, params) in &script.push_after {
                if trigger != &method {
                    continue;
                }
                let Ok(bytes) =
                    encode_message(&RpcMessage::notification(notifica.clone(), params.clone()))
                else {
                    return;
                };
                if output.write_all(&bytes).is_err() || output.flush().is_err() {
                    return;
                }
            }
            if method == "exit" {
                return;
            }
        }
    }
}
