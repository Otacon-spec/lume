//! Chi **possiede** la sessione: un attore, raggiunto solo per messaggi.
//!
//! Nessuno dei tre posti ovvi reggeva. Non un campo sull'app: i metodi di
//! `features.rs` sono `async fn(&self)` e il future che ne esce prende in
//! prestito `&self`, mentre [`LspSession::next_event`] vuole l'esclusiva
//! proprio nel frattempo. Non dentro un tool: il registro dà solo `&dyn Tool`.
//! Non un worker sincrono come `lume-sql`: qui è async tutto.
//!
//! Quindi la sessione vive dentro **questo** future, e chi la vuole le manda
//! un [`LspCommand`]. In cambio escono [`ActorEvent`], che è tutto quel che
//! l'interfaccia ha bisogno di sapere.
//!
//! ATTENZIONE: **Due punti non sono interrompibili, e non è un dettaglio.**
//! `initialize` sta **prima** del ciclo, e il riavvio dopo un
//! [`SessionEvent::Disconnected`] è un `.await` **fuori** dal `select!`. È la
//! ragione per cui `next_event` ha smesso di riavviare da sé: `select!`
//! droppa il future perdente, e un riavvio abbandonato a metà lascia la
//! sessione ferma a `Initializing` — server vivo, IDE che lo crede morto.
//! Nel frattempo i comandi non si perdono: si accodano nella mailbox, che è
//! esattamente il suo mestiere.
//!
//! La funzione **ritorna** quando ha finito. La regola iced «non finire mai»
//! vive nell'involucro che la ospita, non qui: così un test può aspettarla e
//! guardare cosa ha lasciato dietro.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use lsp_types::{
    CompletionResponse, DocumentSymbolResponse, GotoDefinitionResponse, Hover, Location, Position,
    PrepareRenameResponse, PublishDiagnosticsParams, Uri, WorkspaceEdit,
};
use tokio::sync::Notify;
use tokio::sync::mpsc::error::TryRecvError;

use crate::{
    FeatureClient, InitializeSpec, LspSession, NegotiatedCapabilities, ServerCommand, SessionEvent,
    TransportConfig,
};

/// Quanto si aspetta il commiato prima di lasciar fare al sistema operativo.
/// Oltre, la rete è il Job Object: chiudere lo stream uccide l'albero.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
pub const COMMAND_CAPACITY: usize = 64;
pub const DEFERRED_COMMAND_CAPACITY: usize = 128;
pub const MAILBOX_CAPACITY: usize = COMMAND_CAPACITY + DEFERRED_COMMAND_CAPACITY;
pub const MAILBOX_TEXT_BUDGET_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxError {
    Full,
    Closed,
}

#[derive(Debug)]
struct SharedCommands {
    queue: VecDeque<LspCommand>,
    queued_text_bytes: usize,
    sender_count: usize,
    receiver_open: bool,
}

/// Mailbox FIFO limitata; le modifiche pendenti consecutive dello stesso URI
/// si fondono fino al successivo comando che costituisce una barriera.
#[derive(Debug)]
pub struct LspCommandMailbox {
    shared: Arc<Mutex<SharedCommands>>,
    wake: Arc<Notify>,
    capacity: usize,
    max_text_bytes: usize,
}

#[derive(Debug)]
pub struct LspCommandReceiver {
    shared: Arc<Mutex<SharedCommands>>,
    wake: Arc<Notify>,
}

impl LspCommandReceiver {
    pub async fn recv(&mut self) -> Option<LspCommand> {
        loop {
            let notified = self.wake.notified();
            let (command, closed) = {
                let mut shared = self
                    .shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (rimuovi_primo(&mut shared), shared.sender_count == 0)
            };
            if let Some(command) = command {
                return Some(command);
            }
            if closed {
                return None;
            }
            notified.await;
        }
    }

    pub fn try_recv(&mut self) -> Result<LspCommand, TryRecvError> {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rimuovi_primo(&mut shared).ok_or(if shared.sender_count == 0 {
            TryRecvError::Disconnected
        } else {
            TryRecvError::Empty
        })
    }
}

impl Drop for LspCommandReceiver {
    fn drop(&mut self) {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        shared.receiver_open = false;
        drop(shared);
        self.wake.notify_waiters();
    }
}

impl Clone for LspCommandMailbox {
    fn clone(&self) -> Self {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        shared.sender_count += 1;
        drop(shared);
        Self {
            shared: Arc::clone(&self.shared),
            wake: Arc::clone(&self.wake),
            capacity: self.capacity,
            max_text_bytes: self.max_text_bytes,
        }
    }
}

impl Drop for LspCommandMailbox {
    fn drop(&mut self) {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        shared.sender_count = shared.sender_count.saturating_sub(1);
        drop(shared);
        self.wake.notify_waiters();
    }
}

impl LspCommandMailbox {
    pub fn channel() -> (Self, LspCommandReceiver) {
        Self::with_capacity(MAILBOX_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> (Self, LspCommandReceiver) {
        Self::with_limits(capacity, MAILBOX_TEXT_BUDGET_BYTES)
    }

    pub fn with_limits(capacity: usize, max_text_bytes: usize) -> (Self, LspCommandReceiver) {
        let shared = Arc::new(Mutex::new(SharedCommands {
            queue: VecDeque::new(),
            queued_text_bytes: 0,
            sender_count: 1,
            receiver_open: true,
        }));
        let wake = Arc::default();
        (
            Self {
                shared: Arc::clone(&shared),
                wake: Arc::clone(&wake),
                capacity: capacity.max(1),
                max_text_bytes,
            },
            LspCommandReceiver { shared, wake },
        )
    }

    /// Accetta l'ordine nella coda limitata, oppure restituisce un errore se il
    /// canale è chiuso o anche la coda differita ha raggiunto il proprio limite.
    pub fn send(&self, command: LspCommand) -> Result<(), MailboxError> {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !shared.receiver_open {
            return Err(MailboxError::Closed);
        }
        if let Some(index) = indice_change_coalescibile(&shared.queue, &command) {
            let Some(precedente) = shared.queue.get(index) else {
                return Err(MailboxError::Full);
            };
            let bytes_precedenti = bytes_testo(precedente);
            let bytes_nuovi = bytes_testo(&command);
            let bytes_totali = shared
                .queued_text_bytes
                .saturating_sub(bytes_precedenti)
                .checked_add(bytes_nuovi)
                .ok_or(MailboxError::Full)?;
            if bytes_totali > self.max_text_bytes {
                return Err(MailboxError::Full);
            }
            let Some(slot) = shared.queue.get_mut(index) else {
                return Err(MailboxError::Full);
            };
            *slot = command;
            shared.queued_text_bytes = bytes_totali;
            drop(shared);
            self.wake.notify_one();
            return Ok(());
        }
        if shared.queue.len() >= self.capacity {
            return Err(MailboxError::Full);
        }
        let bytes_totali = shared
            .queued_text_bytes
            .checked_add(bytes_testo(&command))
            .ok_or(MailboxError::Full)?;
        if bytes_totali > self.max_text_bytes {
            return Err(MailboxError::Full);
        }
        shared.queued_text_bytes = bytes_totali;
        shared.queue.push_back(command);
        drop(shared);
        self.wake.notify_one();
        Ok(())
    }
}

fn change_uri(command: &LspCommand) -> Option<&Uri> {
    match command {
        LspCommand::Change { uri, .. } | LspCommand::ChangeSnapshot { uri, .. } => Some(uri),
        _ => None,
    }
}

fn indice_change_coalescibile(queue: &VecDeque<LspCommand>, command: &LspCommand) -> Option<usize> {
    let uri = change_uri(command)?;
    queue
        .iter()
        .enumerate()
        .rev()
        .take_while(|(_, queued)| change_uri(queued).is_some())
        .find_map(|(index, queued)| (change_uri(queued) == Some(uri)).then_some(index))
}

fn bytes_testo(command: &LspCommand) -> usize {
    match command {
        LspCommand::Open { text, .. } | LspCommand::Change { text, .. } => text.len(),
        LspCommand::ChangeSnapshot { text, .. } => text.len_bytes(),
        LspCommand::Save { .. }
        | LspCommand::Close { .. }
        | LspCommand::Ask { .. }
        | LspCommand::Shutdown => 0,
    }
}

fn rimuovi_primo(shared: &mut SharedCommands) -> Option<LspCommand> {
    let command = shared.queue.pop_front()?;
    shared.queued_text_bytes = shared
        .queued_text_bytes
        .saturating_sub(bytes_testo(&command));
    Some(command)
}

/// Cosa si può chiedere all'attore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspCommand {
    Open {
        uri: Uri,
        language: String,
        text: String,
    },
    /// Testo completo posseduto dal chiamante: l'attore lo riduce a un delta
    /// incrementale se il server lo ha negoziato, altrimenti invia il testo.
    Change {
        uri: Uri,
        text: String,
    },
    /// Snapshot condiviso: materializzato dall'attore, senza allocazione
    /// full-text nel thread che ammette il comando nella mailbox.
    ChangeSnapshot {
        uri: Uri,
        text: ropey::Rope,
    },
    Save {
        uri: Uri,
    },
    Close {
        uri: Uri,
    },
    /// Una **domanda**, non un ordine: prima o poi torna un
    /// [`ActorEvent::Answer`] con lo stesso `id`. Chi chiede tiene il conto
    /// di cosa aveva chiesto — l'attore no, e non deve: il testo può essere
    /// cambiato nel frattempo, e sapere se la risposta vale ancora è mestiere
    /// di chi possiede i documenti.
    Ask {
        id: u64,
        request: LspRequest,
    },
    Shutdown,
}

/// Cosa si può chiedere. Cresce di un ramo per ogni richiesta LSP cablata.
///
/// ATTENZIONE: I parametri `lsp_types` li costruisce **l'attore**, non chi chiede:
/// così la forma del protocollo resta di chi parla il protocollo, e l'app
/// dice solo *dove* e *cosa*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspRequest {
    Definition {
        uri: Uri,
        position: Position,
    },
    /// Dove compare il simbolo in questa posizione, **la dichiarazione
    /// compresa**: cercare gli utilizzi e non trovare il posto da cui si è
    /// partiti confonde più di quanto aiuti.
    References {
        uri: Uri,
        position: Position,
    },
    /// Si può rinominare quel che sta qui? È una domanda a parte, e serve:
    /// senza, il riquadro si aprirebbe anche su una parola chiave e il no
    /// arriverebbe **dopo** che l'utente ha digitato il nome nuovo.
    PrepareRename {
        uri: Uri,
        position: Position,
    },
    Formatting {
        uri: Uri,
        tab_size: u32,
        insert_spaces: bool,
    },
    /// Formattazione di un intervallo (la selezione corrente). La risposta
    /// è la stessa di `Formatting`: un elenco di `TextEdit` per il documento.
    RangeFormatting {
        uri: Uri,
        range: lsp_types::Range,
        tab_size: u32,
        insert_spaces: bool,
    },
    /// Azioni rapide disponibili nel punto (o sull'intervallo) corrente.
    CodeAction {
        uri: Uri,
        range: lsp_types::Range,
    },
    Rename {
        uri: Uri,
        position: Position,
        new_name: String,
    },
    /// La struttura di **un** documento. Non c'è una posizione: la risposta
    /// vale per tutto il file, ed è quel che la rende utile due volte — al
    /// pannello Struttura e al breadcrumb.
    DocumentSymbols {
        uri: Uri,
    },
    /// Le voci di completamento nel punto del cursore.
    Completion {
        uri: Uri,
        position: Position,
    },
    /// La documentazione del simbolo sotto la posizione.
    Hover {
        uri: Uri,
        position: Position,
    },
    /// Firma attiva nel punto del cursore.
    SignatureHelp {
        uri: Uri,
        position: Position,
        trigger_character: Option<String>,
    },
}

impl LspRequest {
    /// ATTENZIONE: Non è una precauzione teorica: la prima richiesta a un
    /// rust-analyzer che sta ancora indicizzando può metterci parecchio, e
    /// senza scadenza la voce resterebbe in sospeso per sempre — un
    /// «caricamento» che non finisce mai è peggio di un no.
    const fn timeout(&self) -> Duration {
        match *self {
            Self::Definition { .. } => Duration::from_secs(10),
            Self::PrepareRename { .. } => Duration::from_secs(5),
            // come `references`, e per la stessa ragione: per rinominare
            // deve prima trovarle tutte
            Self::Rename { .. }
            | Self::Formatting { .. }
            | Self::RangeFormatting { .. }
            | Self::CodeAction { .. } => Duration::from_secs(20),
            // la più lenta delle tre: su un simbolo molto usato il server
            // deve guardare l'indice intero
            Self::References { .. } => Duration::from_secs(20),
            // più corta: è un aggiornamento di sfondo, e se tarda tanto vale
            // la pena richiederlo sul testo nuovo invece di aspettare quello
            // vecchio
            Self::DocumentSymbols { .. } => Duration::from_secs(5),
            // Corti di proposito: dopo cinque secondi chi scrive ha già
            // cambiato contesto e la risposta non serve più.
            Self::Completion { .. } | Self::Hover { .. } | Self::SignatureHelp { .. } => {
                Duration::from_secs(5)
            }
        }
    }
}

/// Com'è andata. Un ramo per ramo di [`LspRequest`].
#[derive(Debug, Clone, PartialEq)]
pub enum LspAnswer {
    /// `None` è una risposta legittima: il server non sa dove sta quel
    /// simbolo. Non è un errore, e non va detto come tale.
    Definition(Option<GotoDefinitionResponse>),
    Symbols(Option<DocumentSymbolResponse>),
    References(Option<Vec<Location>>),
    PrepareRename(Option<PrepareRenameResponse>),
    Rename(Option<WorkspaceEdit>),
    Formatting {
        uri: Uri,
        edits: Option<Vec<lsp_types::TextEdit>>,
    },
    /// `None` è legittimo: nessuna voce da offrire qui.
    Completion(Option<CompletionResponse>),
    /// `None` è legittimo: nessuna documentazione per quel punto.
    Hover(Option<Hover>),
    /// `None` è legittimo: nessuna firma disponibile nel contesto.
    SignatureHelp(Option<lsp_types::SignatureHelp>),
    /// `None` è legittimo: nessuna azione disponibile qui.
    CodeAction(Option<Vec<lsp_types::CodeActionOrCommand>>),
}

/// Pronto per l'uso dopo `initialize`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionReady {
    pub capabilities: NegotiatedCapabilities,
    pub peak_memory_bytes: Option<u64>,
}

/// Cosa ne esce.
#[derive(Debug, Clone, PartialEq)]
pub enum ActorEvent {
    Initialized(Box<SessionReady>),
    Diagnostics(Box<PublishDiagnosticsParams>),
    Progress(Box<lsp_types::ProgressParams>),
    /// Rumore del server, o un comando che non è andato: si mostra, non si
    /// muore.
    Log(String),
    /// La risposta a un [`LspCommand::Ask`], con l'`id` di chi l'aveva
    /// chiesta. Arriva **fuori ordine** rispetto alle altre: le richieste
    /// corrono in parallelo apposta.
    Answer(u64, Box<Result<LspAnswer, String>>),
    /// La sessione non c'è più e non torna. Chi ascolta spegne la riga.
    Failed(String),
    /// Esito del processo dopo un commiato richiesto. `Stopped` segue sempre
    /// soltanto quando il protocollo di shutdown è terminato senza errore.
    ProcessExited {
        success: bool,
        code: Option<i32>,
    },
    Stopped,
}

/// Una richiesta in volo: l'id per ritrovarla, e come è andata.
type InFlight = Pin<Box<dyn Future<Output = (u64, Result<LspAnswer, String>)> + Send>>;

/// Prepara il future di una domanda.
///
/// ATTENZIONE: Il cliente si prende **adesso**, non si tiene in caldo: un
/// `FeatureClient` porta dentro il trasporto del processo corrente, e dopo un
/// riavvio quello punta a un morto. Chiederlo qui vuol dire anche che una
/// domanda arrivata durante un riavvio riceve un `NotReady` onesto invece di
/// scrivere in un tubo chiuso.
fn ask(
    client: Result<FeatureClient, crate::SessionError>,
    id: u64,
    request: LspRequest,
) -> InFlight {
    let timeout = request.timeout();
    Box::pin(async move {
        let esito = match client {
            Ok(client) => match request {
                LspRequest::Completion { uri, position } => client
                    .completion(
                        lsp_types::CompletionParams {
                            text_document_position: lsp_types::TextDocumentPositionParams {
                                text_document: lsp_types::TextDocumentIdentifier { uri },
                                position,
                            },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                            partial_result_params: lsp_types::PartialResultParams::default(),
                            context: None,
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::Completion)
                    .map_err(|error| error.to_string()),
                LspRequest::Hover { uri, position } => client
                    .hover(
                        lsp_types::HoverParams {
                            text_document_position_params: lsp_types::TextDocumentPositionParams {
                                text_document: lsp_types::TextDocumentIdentifier { uri },
                                position,
                            },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::Hover)
                    .map_err(|error| error.to_string()),
                LspRequest::SignatureHelp {
                    uri,
                    position,
                    trigger_character,
                } => client
                    .signature_help(
                        lsp_types::SignatureHelpParams {
                            text_document_position_params: lsp_types::TextDocumentPositionParams {
                                text_document: lsp_types::TextDocumentIdentifier { uri },
                                position,
                            },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                            context: Some(lsp_types::SignatureHelpContext {
                                trigger_kind: if trigger_character.is_some() {
                                    lsp_types::SignatureHelpTriggerKind::TRIGGER_CHARACTER
                                } else {
                                    lsp_types::SignatureHelpTriggerKind::INVOKED
                                },
                                trigger_character,
                                is_retrigger: false,
                                active_signature_help: None,
                            }),
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::SignatureHelp)
                    .map_err(|error| error.to_string()),
                LspRequest::PrepareRename { uri, position } => client
                    .prepare_rename(
                        lsp_types::TextDocumentPositionParams {
                            text_document: lsp_types::TextDocumentIdentifier { uri },
                            position,
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::PrepareRename)
                    .map_err(|error| error.to_string()),
                LspRequest::Rename {
                    uri,
                    position,
                    new_name,
                } => client
                    .rename(
                        lsp_types::RenameParams {
                            text_document_position: lsp_types::TextDocumentPositionParams {
                                text_document: lsp_types::TextDocumentIdentifier { uri },
                                position,
                            },
                            new_name,
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::Rename)
                    .map_err(|error| error.to_string()),
                LspRequest::References { uri, position } => client
                    .references(
                        lsp_types::ReferenceParams {
                            text_document_position: lsp_types::TextDocumentPositionParams {
                                text_document: lsp_types::TextDocumentIdentifier { uri },
                                position,
                            },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                            partial_result_params: lsp_types::PartialResultParams::default(),
                            context: lsp_types::ReferenceContext {
                                include_declaration: true,
                            },
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::References)
                    .map_err(|error| error.to_string()),
                LspRequest::Formatting {
                    uri,
                    tab_size,
                    insert_spaces,
                } => client
                    .format_document(
                        lsp_types::DocumentFormattingParams {
                            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                            options: lsp_types::FormattingOptions {
                                tab_size,
                                insert_spaces,
                                ..lsp_types::FormattingOptions::default()
                            },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                        },
                        timeout,
                    )
                    .await
                    .map(|edits| LspAnswer::Formatting { uri, edits })
                    .map_err(|error| error.to_string()),
                LspRequest::RangeFormatting {
                    uri,
                    range,
                    tab_size,
                    insert_spaces,
                } => client
                    .format_range(
                        lsp_types::DocumentRangeFormattingParams {
                            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                            range,
                            options: lsp_types::FormattingOptions {
                                tab_size,
                                insert_spaces,
                                ..lsp_types::FormattingOptions::default()
                            },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                        },
                        timeout,
                    )
                    .await
                    .map(|edits| LspAnswer::Formatting { uri, edits })
                    .map_err(|error| error.to_string()),
                LspRequest::CodeAction { uri, range } => client
                    .code_actions(
                        lsp_types::CodeActionParams {
                            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                            range,
                            context: lsp_types::CodeActionContext {
                                diagnostics: Vec::new(),
                                only: None,
                                trigger_kind: Some(lsp_types::CodeActionTriggerKind::INVOKED),
                            },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                            partial_result_params: lsp_types::PartialResultParams::default(),
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::CodeAction)
                    .map_err(|error| error.to_string()),
                LspRequest::DocumentSymbols { uri } => client
                    .document_symbols(
                        lsp_types::DocumentSymbolParams {
                            text_document: lsp_types::TextDocumentIdentifier { uri },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                            partial_result_params: lsp_types::PartialResultParams::default(),
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::Symbols)
                    .map_err(|error| error.to_string()),
                LspRequest::Definition { uri, position } => client
                    .definition(
                        lsp_types::GotoDefinitionParams {
                            text_document_position_params: lsp_types::TextDocumentPositionParams {
                                text_document: lsp_types::TextDocumentIdentifier { uri },
                                position,
                            },
                            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                            partial_result_params: lsp_types::PartialResultParams::default(),
                        },
                        timeout,
                    )
                    .await
                    .map(LspAnswer::Definition)
                    .map_err(|error| error.to_string()),
            },
            Err(error) => Err(error.to_string()),
        };
        (id, esito)
    })
}

/// Il ciclo di vita di una sessione, dall'avvio al commiato.
///
/// Finisce quando arriva [`LspCommand::Shutdown`], quando la mailbox viene
/// chiusa da chi la teneva, o quando la sessione muore per davvero.
pub async fn run(
    command: ServerCommand,
    spec: InitializeSpec,
    mut commands: LspCommandReceiver,
    events: tokio::sync::mpsc::UnboundedSender<ActorEvent>,
) {
    let mut session = match LspSession::spawn(command, TransportConfig::default()) {
        Ok(session) => session,
        Err(error) => {
            let _ = events.send(ActorEvent::Failed(error.to_string()));
            return;
        }
    };
    // ATTENZIONE: **fuori** dal ciclo: un `initialize` abbandonato a metà lascia lo
    // stato a `Initializing` e da lì ogni `require_ready` fallisce per sempre
    match session.initialize(spec).await {
        Ok(negotiated) => {
            let peak_memory_bytes = session.peak_memory_bytes();
            let _ = events.send(ActorEvent::Initialized(Box::new(SessionReady {
                capabilities: negotiated,
                peak_memory_bytes,
            })));
        }
        Err(error) => {
            let _ = events.send(ActorEvent::Failed(error.to_string()));
            return;
        }
    }

    // ATTENZIONE: Le domande **non** si aspettano dentro il ciclo. Fermarsi su un
    // `.await` qui vorrebbe dire smettere di svuotare le notifiche del
    // server, e la coda degli eventi del trasporto a quel punto non aspetta:
    // `send_event` fa `try_send` e a coda piena **disconnette**
    // (`Overloaded`, `send_event` in `transport.rs`). Vivono qui accanto, e il ciclo
    // continua a girare.
    let mut in_volo: FuturesUnordered<InFlight> = FuturesUnordered::new();
    let mut throttle = crate::ProgressThrottle::default();

    loop {
        tokio::select! {
            // ATTENZIONE: La guardia serve: `FuturesUnordered` vuoto è **pronto**
            // subito con `None`, e senza questa il `select!` girerebbe a
            // vuoto bruciando un core.
            Some((id, esito)) = in_volo.next(), if !in_volo.is_empty() => {
                let _ = events.send(ActorEvent::Answer(id, Box::new(esito)));
            }
            comando = commands.recv() => {
                // mailbox chiusa: chi la teneva non c'è più, e restare vivi
                // vorrebbe dire un processo che nessuno può più fermare
                let Some(comando) = comando else { break };
                if matches!(comando, LspCommand::Shutdown) {
                    break;
                }
                if let LspCommand::Ask { id, request } = comando {
                    let mut domanda = ask(session.feature_client(), id, request);
                    // FeatureClient arriva senza sospensioni a request_handle:
                    // il primo poll ammette la richiesta nella coda FIFO del
                    // trasporto prima di cedere sulla risposta. Inserire solo
                    // il future permetteva al prossimo Change di scavalcarla.
                    // Non attendiamo la risposta: rimane cancellabile e in volo.
                    match futures::poll!(domanda.as_mut()) {
                        std::task::Poll::Ready((id, esito)) => {
                            let _ = events.send(ActorEvent::Answer(id, Box::new(esito)));
                        }
                        std::task::Poll::Pending => in_volo.push(domanda),
                    }
                    continue;
                }
                if let Err(error) = apply(&mut session, comando).await {
                    // un documento che il server non conosce non è un motivo
                    // per buttare via la sessione: si dice e si va avanti
                    let _ = events.send(ActorEvent::Log(error));
                }
            }
            evento = session.next_event() => {
                match evento {
                    Some(SessionEvent::Diagnostics(params)) => {
                        let _ = events.send(ActorEvent::Diagnostics(Box::new(params)));
                    }
                    Some(SessionEvent::Progress(params)) => {
                        if throttle.admit(&params, std::time::Instant::now()) {
                            let _ = events.send(ActorEvent::Progress(Box::new(params)));
                        }
                    }
                    Some(SessionEvent::Log(line)) => {
                        let _ = events.send(ActorEvent::Log(line));
                    }
                    Some(SessionEvent::Notification(_)) => {}
                    Some(SessionEvent::Exited(error)) => {
                        let _ = events.send(ActorEvent::Failed(error.to_string()));
                        return;
                    }
                    Some(SessionEvent::Disconnected(caduta)) => {
                        let _ = events.send(ActorEvent::Log(format!(
                            "server caduto: {caduta}; riavvio"
                        )));
                        // ATTENZIONE: passo proprio e **non interrompibile**: i comandi
                        // che arrivano nel frattempo restano in coda
                        if let Err(error) = session.restart().await {
                            let _ = events.send(ActorEvent::Failed(error.to_string()));
                            return;
                        }
                    }
                    None => {
                        let _ = events.send(ActorEvent::Failed(
                            "il canale col server si è chiuso".to_owned(),
                        ));
                        return;
                    }
                }
            }
        }
    }

    let status = match session.shutdown(SHUTDOWN_TIMEOUT).await {
        Ok(status) => status,
        Err(error) => {
            let _ = events.send(ActorEvent::Failed(format!(
                "commiato non riuscito: {error}"
            )));
            return;
        }
    };
    let _ = events.send(ActorEvent::ProcessExited {
        success: status.success(),
        code: status.code(),
    });
    let _ = events.send(ActorEvent::Stopped);
}

async fn apply(session: &mut LspSession, comando: LspCommand) -> Result<(), String> {
    match comando {
        LspCommand::Open {
            uri,
            language,
            text,
        } => session.open_document_wait(uri, language, text).await,
        LspCommand::Change { uri, text } => {
            session.change_document_text_owned_wait(&uri, text).await
        }
        LspCommand::ChangeSnapshot { uri, text } => {
            // L'attore possiede questo lavoro e non passa al comando seguente
            // prima di apply. Nessun worker detached o nuova coda non posseduta.
            let text = text.to_string();
            session.change_document_text_owned_wait(&uri, text).await
        }
        LspCommand::Save { uri } => session.save_document(&uri, false),
        LspCommand::Close { uri } => session.close_document(&uri),
        // gestiti da chi chiama: qui non ci arrivano
        LspCommand::Ask { .. } | LspCommand::Shutdown => Ok(()),
    }
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod mailbox_tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[tokio::test]
    async fn coda_limitata_fonde_change_senza_superare_barriere_ask() {
        let (mailbox, mut receiver) = LspCommandMailbox::with_capacity(3);
        let uri: Uri = "file:///workspace/src/main.rs".parse().expect("URI valida");
        assert!(
            mailbox
                .send(LspCommand::Open {
                    uri: uri.clone(),
                    language: "rust".into(),
                    text: "fn main() {}".into(),
                })
                .is_ok()
        );
        assert!(
            mailbox
                .send(LspCommand::Change {
                    uri: uri.clone(),
                    text: "fn first() {}".into(),
                })
                .is_ok()
        );
        for revision in 0..1_000 {
            assert!(
                mailbox
                    .send(LspCommand::Change {
                        uri: uri.clone(),
                        text: format!("fn revision_{revision}() {{}}"),
                    })
                    .is_ok()
            );
        }
        assert!(
            mailbox
                .send(LspCommand::Ask {
                    id: 7,
                    request: LspRequest::Hover {
                        uri: uri.clone(),
                        position: Position::new(0, 4),
                    },
                })
                .is_ok()
        );
        assert_eq!(
            mailbox.send(LspCommand::Change {
                uri: uri.clone(),
                text: "fn after_barrier() {}".into(),
            }),
            Err(MailboxError::Full),
            "una modifica dopo Ask non può sostituire quella prima della richiesta"
        );

        assert!(matches!(
            receiver.recv().await,
            Some(LspCommand::Open { .. })
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(LspCommand::Change { text, .. }) if text == "fn revision_999() {}"
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(LspCommand::Ask { id: 7, .. })
        ));
    }

    #[tokio::test]
    async fn pressione_massima_rifiuta_esplicitamente_l_ordine_in_eccesso() {
        let (mailbox, receiver) = LspCommandMailbox::with_capacity(DEFERRED_COMMAND_CAPACITY + 2);
        let uri: Uri = "file:///workspace/src/main.rs".parse().expect("URI valida");
        assert!(
            mailbox
                .send(LspCommand::Open {
                    uri: uri.clone(),
                    language: "rust".into(),
                    text: String::new(),
                })
                .is_ok()
        );
        assert!(mailbox.send(LspCommand::Save { uri: uri.clone() }).is_ok());
        for _ in 0..DEFERRED_COMMAND_CAPACITY {
            assert!(mailbox.send(LspCommand::Save { uri: uri.clone() }).is_ok());
        }
        assert_eq!(mailbox.send(LspCommand::Shutdown), Err(MailboxError::Full));
        drop(receiver);
    }

    #[tokio::test]
    async fn budget_testo_rifiuta_sostituzione_senza_perdere_l_ultima_revisione_accettata() {
        let (mailbox, mut receiver) = LspCommandMailbox::with_limits(4, 12);
        let uri: Uri = "file:///workspace/src/main.rs".parse().expect("URI valida");
        assert!(
            mailbox
                .send(LspCommand::Open {
                    uri: uri.clone(),
                    language: "rust".into(),
                    text: String::new(),
                })
                .is_ok()
        );
        assert!(
            mailbox
                .send(LspCommand::Change {
                    uri: uri.clone(),
                    text: "prima".into(),
                })
                .is_ok()
        );
        assert!(
            mailbox
                .send(LspCommand::Change {
                    uri: uri.clone(),
                    text: "123456789012".into(),
                })
                .is_ok()
        );

        assert_eq!(
            mailbox.send(LspCommand::Change {
                uri: uri.clone(),
                text: "testo troppo lungo".into(),
            }),
            Err(MailboxError::Full)
        );
        assert!(matches!(
            receiver.recv().await,
            Some(LspCommand::Open { .. })
        ));
        assert!(matches!(receiver.recv().await,
            Some(LspCommand::Change { text, .. }) if text == "123456789012"));

        assert!(
            mailbox
                .send(LspCommand::Open {
                    uri,
                    language: "rust".into(),
                    text: "liberato".into(),
                })
                .is_ok(),
            "il consumo dalla coda deve liberare il budget byte"
        );
    }

    #[tokio::test]
    async fn chiusura_sender_drena_ordini_accettati_e_termina() {
        let (mailbox, mut receiver) = LspCommandMailbox::with_capacity(2);
        let uri: Uri = "file:///workspace/src/main.rs".parse().expect("URI valida");
        assert!(
            mailbox
                .send(LspCommand::Open {
                    uri: uri.clone(),
                    language: "rust".into(),
                    text: String::new(),
                })
                .is_ok()
        );
        assert!(mailbox.send(LspCommand::Save { uri: uri.clone() }).is_ok());
        drop(mailbox);
        assert!(matches!(
            receiver.recv().await,
            Some(LspCommand::Open { .. })
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(LspCommand::Save { .. })
        ));
        assert!(receiver.recv().await.is_none());
    }
}
