//! Il language server **a comando**: lo stato freddo, e il filo che lo lega
//! alla `Subscription` dove vive davvero.
//!
//! Gemello di `sql_mode.rs`. Qui non c'è nessuna sessione: c'è quel che l'app
//! *sa* — quale server serve a questo progetto, a che punto è, come si
//! chiama la sua mailbox — mentre la sessione vera vive dentro l'attore
//! (`lume_lsp::actor`), dentro uno stream posseduto da iced.
//!
//! ATTENZIONE: **La subscription non è un dettaglio di comodo, è la garanzia.** Finché
//! l'app produce quel ramo lo stream vive; smesso di produrlo, iced lo droppa,
//! il future muore, `ServerProcess` cade e il **Job Object** con
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` porta via l'albero intero — `cargo` e
//! `rustc` figli compresi. Il vincolo «nessun processo in background da
//! inattivo» diventa così **strutturale**, non una promessa. Il tasto Ferma
//! resta la via garbata (`shutdown` con la sua scadenza), lo stream è la rete.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lsp_types::{
    DocumentSymbol, DocumentSymbolResponse, GotoDefinitionResponse, Location, Position, Range,
    SymbolKind, Uri,
};
use lume_core::DocId;
use lume_language_servers::{CancelFlag, ResolvedServer, ServerCatalog, ServerSpec};
use lume_lsp::{
    ActorEvent, DiagnosticStore, FeatureAvailability, InitializeSpec, LspCommand, ServerCommand,
    ServerId,
};
use serde_json::Value;

/// Prova di forma, prima di scriverci sopra: la sessione dev'essere `Send`, o
/// l'attore non può vivere in una `Subscription` (`iced::stream::channel` su
/// nativo vuole un future `Send`).
///
/// Se un giorno smettesse di compilare, il ripiego è già scritto in casa — un
/// thread con runtime `current_thread`, come `AzureDriver` — e il corpo
/// dell'attore non cambierebbe di una riga.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<lume_lsp::LspSession>();
};

/// A che punto è il server. **Volatile**: dopo un ripristino di sessione si
/// riparte sempre da `Ferma`, ed è ciò che protegge l'avvio sotto i 100 ms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspState {
    /// Mai avviato in questa sessione, o fermato dall'utente.
    Ferma,
    /// La sonda `--version` è in corso: due secondi, fuori dal thread UI.
    Ricerca,
    /// Processo avviato, `initialize` non ancora tornato.
    Avvio,
    /// Il download del server è in corso: ha uno stato suo perché è l'unico
    /// momento in cui il tasto è «Annulla» e la riga mostra dei byte. Conta
    /// come vivo finché il worker di installazione non termina.
    Scaricamento {
        received: u64,
        total: u64,
        fase: FaseScarico,
    },
    Attivo {
        version: String,
        /// Picco di memoria del Job Object al momento dell'`initialize`, in MiB.
        memory_mb: Option<u64>,
    },
    /// Il commiato è partito e si aspetta il `Stopped`. ATTENZIONE: Conta come vivo:
    /// finché non arriva, il ramo della subscription **deve** restare
    /// prodotto, o iced butterebbe via lo stream a metà `shutdown` e il
    /// commiato garbato non finirebbe mai.
    Arresto,
    /// Il binario non si trova: qui il tasto dice «Scarica», non «Avvia».
    Assente(String),
    Errore(String),
}

impl LspState {
    /// Tiene risorse in questo momento?
    pub const fn is_live(&self) -> bool {
        matches!(
            self,
            Self::Ricerca
                | Self::Avvio
                | Self::Scaricamento { .. }
                | Self::Attivo { .. }
                | Self::Arresto
        )
    }
}

/// Fase dell'installazione gestita: trasferimento, verifica, estrazione.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaseScarico {
    Scarico,
    Verifica,
    Estrazione,
}

/// Progress transitorio inviato dal server, indicizzato per token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Indexing {
    attivita: BTreeMap<String, Attivita>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Attivita {
    titolo: String,
    messaggio: Option<String>,
    percentuale: Option<u32>,
}
impl Indexing {
    pub fn is_empty(&self) -> bool {
        self.attivita.is_empty()
    }
    pub fn clear(&mut self) {
        self.attivita.clear();
    }
    pub fn apply(&mut self, params: &lsp_types::ProgressParams) -> bool {
        use lsp_types::{NumberOrString, ProgressParamsValue, WorkDoneProgress};
        let key = match &params.token {
            NumberOrString::Number(number) => number.to_string(),
            NumberOrString::String(text) => text.clone(),
        };
        let ProgressParamsValue::WorkDone(value) = &params.value;
        match value {
            WorkDoneProgress::Begin(begin) => {
                self.attivita.insert(
                    key,
                    Attivita {
                        titolo: begin.title.clone(),
                        messaggio: begin.message.clone(),
                        percentuale: begin.percentage,
                    },
                );
                true
            }
            WorkDoneProgress::Report(report) => {
                let Some(activity) = self.attivita.get_mut(&key) else {
                    return false;
                };
                if report.message.is_some() {
                    activity.messaggio.clone_from(&report.message);
                }
                if report.percentage.is_some() {
                    activity.percentuale = report.percentage;
                }
                true
            }
            WorkDoneProgress::End(_) => self.attivita.remove(&key).is_some(),
        }
    }
    pub fn label(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let activity = self.attivita.values().next()?;
        let mut label = activity.titolo.clone();
        if let Some(percent) = activity.percentuale {
            label.push_str(&format!(" {percent}%"));
        }
        if let Some(message) = activity.messaggio.as_deref().filter(|m| !m.is_empty()) {
            label.push_str(&format!(" · {message}"));
        }
        Some(label)
    }
}

/// Quel che l'app sa del language server. La sessione non è qui.
pub struct LspWorkspace {
    pub spec: &'static ServerSpec,
    pub root: PathBuf,
    /// Cresce a **ogni** avvio. Serve due volte: gli eventi di una
    /// generazione superata si buttano, e la chiave della subscription cambia,
    /// quindi uno Stop seguito da un Avvia è uno stream nuovo e non il
    /// vecchio risvegliato.
    pub epoch: u64,
    pub state: LspState,
    /// La cassetta della posta dell'attore. Arriva **dopo** l'avvio, col
    /// primo evento dello stream: `run_with` costruisce lo stream da una
    /// `fn` senza catture, quindi il canale lo crea lo stream e ce lo passa.
    pub mailbox: Option<lume_lsp::LspCommandMailbox>,
    /// Il comando risolto, tenuto in caldo: senza, ogni riavvio ripagherebbe
    /// la sonda `--version` da due secondi.
    pub command: Option<ServerCommand>,
    /// La versione del binario, dalla sonda `--version`. Vuota finché non
    /// c'è: una riga di stato senza numero è meglio di un numero inventato.
    pub version: String,
    pub problems: DiagnosticStore,
    pub indexing: Indexing,
    /// I documenti che il server conosce, **per `DocId`**: chiudere una scheda
    /// arriva quando il documento sta per sparire, e l'URI da sola non
    /// basterebbe a ritrovarlo. Senza questa mappa un `didChange` su un file
    /// mai aperto è un errore, e un `didClose` di troppo pure.
    pub opened: HashMap<DocId, Uri>,
    /// L'ultima versione **mandata** per ogni documento: un `didChange` che
    /// rimanda lo stesso testo è banda e lavoro d'indice per niente, e il
    /// debounce da solo non lo evita — muovere il cursore non cambia il testo
    /// ma fa scattare lo stesso il timer.
    sent: HashMap<DocId, u64>,
    /// Cosa il server ha **dichiarato** di saper fare. Fino all'`initialize`
    /// è tutto spento, ed è giusto: chiedere una cosa non annunciata è un
    /// errore che si scopre dopo aver aspettato.
    pub availability: FeatureAvailability,
    /// Le domande in volo. Chi risponde porta l'`id`, e qui si ritrova su
    /// **quale testo** era stata fatta.
    pending: HashMap<u64, InFlight>,
    /// La struttura di ogni documento di cui l'abbiamo chiesta.
    pub outlines: HashMap<DocId, Outline>,
    /// Per quale revisione di ogni documento la struttura è **già stata
    /// chiesta**: senza, ogni frame ne chiederebbe una nuova, e un pannello
    /// aperto diventerebbe un polling.
    asked_outline: HashMap<DocId, u64>,
    next_request: u64,
    started_at: Option<Instant>,
    /// Cronometrato dal comando alla risposta a `initialize`, mai stimato.
    /// **Non si persiste**: mostrarlo dopo un riavvio sarebbe spacciare la
    /// misura di un'altra sessione.
    pub startup_ms: Option<u32>,
    /// Lo spec di `initialize`, costruito alla prima richiesta e riusato per
    /// tutta la generazione: `subscription()` chiede `lsp_spawns()` a ogni
    /// messaggio, e ricostruire URI, nome e opzioni JSON ogni volta è lavoro
    /// per niente. Si azzera in `bump`.
    initialize_spec: std::sync::OnceLock<std::sync::Arc<InitializeSpec>>,
    /// Il worker di download condivide il flag: «Ferma» lo arma davvero.
    pub download_cancel: Option<CancelFlag>,
}

impl std::fmt::Debug for LspWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspWorkspace")
            .field("server", &self.spec.id)
            .field("epoch", &self.epoch)
            .field("state", &self.state)
            .field("aperti", &self.opened.len())
            .finish()
    }
}

impl LspWorkspace {
    pub fn new(spec: &'static ServerSpec, root: PathBuf) -> Self {
        Self {
            spec,
            root,
            epoch: 0,
            state: LspState::Ferma,
            mailbox: None,
            command: None,
            version: String::new(),
            problems: DiagnosticStore::new(),
            indexing: Indexing::default(),
            opened: HashMap::new(),
            sent: HashMap::new(),
            availability: FeatureAvailability::default(),
            pending: HashMap::new(),
            outlines: HashMap::new(),
            asked_outline: HashMap::new(),
            next_request: 0,
            started_at: None,
            startup_ms: None,
            initialize_spec: std::sync::OnceLock::new(),
            download_cancel: None,
        }
    }

    pub fn server_id(&self) -> ServerId {
        ServerId::new(self.spec.id.clone())
    }

    /// Una generazione nuova: quel che arriva dalla vecchia si butta.
    pub fn bump(&mut self) -> u64 {
        self.epoch = self.epoch.wrapping_add(1);
        self.mailbox = None;
        self.opened.clear();
        self.sent.clear();
        self.started_at = Some(Instant::now());
        self.startup_ms = None;
        self.initialize_spec = std::sync::OnceLock::new();
        self.epoch
    }

    /// Cosa dire al server in `initialize`: la radice come cartella di
    /// lavoro e le opzioni del catalogo. Una volta per generazione.
    pub fn initialize_spec(&self) -> std::sync::Arc<InitializeSpec> {
        std::sync::Arc::clone(self.initialize_spec.get_or_init(|| {
            let root_uri = crate::lsp_uri::path_to_uri(&self.root);
            let name = self
                .root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "workspace".to_owned());
            std::sync::Arc::new(InitializeSpec {
                workspace_folders: root_uri
                    .clone()
                    .map(|uri| vec![lsp_types::WorkspaceFolder { uri, name }]),
                root_uri,
                initialization_options: initialization_options(self.spec),
                ..InitializeSpec::default()
            })
        }))
    }

    /// Ferma il cronometro. Il punto di misura è la risposta a `initialize`:
    /// è lì che la sessione diventa `Ready`, cioè **usabile**.
    pub fn mark_ready(&mut self, memory_mb: Option<u64>) {
        if let Some(inizio) = self.started_at.take() {
            self.startup_ms = u32::try_from(inizio.elapsed().as_millis()).ok();
        }
        self.state = LspState::Attivo {
            version: self.version.clone(),
            memory_mb,
        };
    }

    /// Il server non c'è più: si dimentica tutto quel che lo riguardava, i
    /// problemi compresi. Un problema che sopravvive al suo server è una riga
    /// che nessuno può più chiudere.
    ///
    /// ATTENZIONE: **E cambia generazione.** Fermare mentre si cerca il binario o
    /// mentre il server si sta avviando lascia roba in volo che nessuno può
    /// richiamare indietro: la sonda `--version` gira in `spawn_blocking` e
    /// non si annulla, e i primi eventi dell'attore sono già nel canale di
    /// iced. Con l'epoca ferma quella roba **arriva e viene creduta** — il
    /// `LspResolved` rimette `Avvio`, `lsp_spawn` torna a produrre il ramo, e
    /// il server **parte dopo che l'utente ha premuto Ferma**. Non si usa
    /// `bump`, che fa anche ripartire il cronometro.
    pub fn forget(&mut self) {
        self.cancel_download();
        let server = self.server_id();
        self.problems.clear_server(&server);
        self.indexing.clear();
        self.epoch = self.epoch.wrapping_add(1);
        self.mailbox = None;
        self.opened.clear();
        self.sent.clear();
        self.availability = FeatureAvailability::default();
        // Le domande in sospeso muoiono col server che avrebbe dovuto
        // rispondere: lasciarle vorrebbe dire che la prima risposta del
        // server **successivo** con lo stesso numero verrebbe creduta.
        self.pending.clear();
        self.outlines.clear();
        self.asked_outline.clear();
        self.started_at = None;
    }

    /// Annulla il download in corso, se presente, e dimentica il flag.
    pub fn cancel_download(&mut self) {
        if let Some(flag) = self.download_cancel.take() {
            flag.cancel();
        }
    }

    /// Registra una domanda e restituisce il suo numero.
    pub fn ask(&mut self, doc: DocId, version: u64) -> u64 {
        self.register(doc, version, false)
    }

    /// Domanda di sfondo (completion/hover): gli errori non diventano avvisi.
    pub fn ask_quiet(&mut self, doc: DocId, version: u64) -> u64 {
        self.register(doc, version, true)
    }

    fn register(&mut self, doc: DocId, version: u64, quiet: bool) -> u64 {
        self.next_request = self.next_request.wrapping_add(1);
        self.pending.insert(
            self.next_request,
            InFlight {
                doc,
                version,
                quiet,
            },
        );
        self.next_request
    }

    /// Ritira la domanda `id`, se era davvero nostra. `None` vuol dire che
    /// non lo era — o che l'abbiamo già ritirata — e la risposta si scarta.
    pub fn answered(&mut self, id: u64) -> Option<InFlight> {
        self.pending.remove(&id)
    }

    /// Serve chiedere la struttura di questo documento?
    ///
    /// No se ce l'abbiamo già aggiornata, e no se l'abbiamo già chiesta per
    /// questa revisione: `sync_outlines` gira a ogni messaggio, e senza
    /// questa guardia un pannello aperto diventerebbe un polling sul server.
    pub fn wants_outline(&self, id: DocId, version: u64) -> bool {
        if !matches!(self.state, LspState::Attivo { .. }) || !self.availability.document_symbols {
            return false;
        }
        if !self.opened.contains_key(&id) {
            return false;
        }
        self.outlines.get(&id).map(|o| o.version) != Some(version)
            && self.asked_outline.get(&id) != Some(&version)
    }

    pub fn mark_outline_asked(&mut self, id: DocId, version: u64) {
        self.asked_outline.insert(id, version);
    }

    pub fn store_outline(&mut self, id: DocId, outline: Outline) {
        self.outlines.insert(id, outline);
    }

    /// La struttura di questo documento, **se parla della sua revisione
    /// corrente**. Una struttura vecchia indicherebbe righe che non esistono
    /// più, con la stessa faccia di una buona.
    pub fn outline_of(&self, id: DocId, version: u64) -> Option<&Outline> {
        self.outlines
            .get(&id)
            .filter(|outline| outline.version == version)
    }

    /// Il server ha in mano **esattamente** questa revisione di questo
    /// documento?
    ///
    /// ATTENZIONE: È la guardia della rinomina multi-file. Per il documento da cui si
    /// è partiti basta l'epoca della domanda; per gli **altri** file toccati
    /// no — nessuno ne ha fotografato la revisione. Questo lo sa: `sent`
    /// avanza solo quando un `didChange` parte davvero, quindi
    /// `sent == version` vuol dire «il server sta guardando il nostro testo».
    pub fn is_synced(&self, id: DocId, version: u64) -> bool {
        self.sent.get(&id) == Some(&version)
    }

    /// Ha senso rinominare adesso?
    pub const fn can_rename(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.rename
    }

    pub const fn can_format(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.formatting
    }

    pub const fn can_format_range(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.range_formatting
    }

    pub const fn can_code_action(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.code_actions
    }

    pub const fn can_prepare_rename(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.prepare_rename
    }

    /// Ha senso chiedere gli utilizzi adesso?
    pub const fn can_find_usages(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.references
    }

    /// Ha senso chiedere una definizione adesso? Server avviato **e**
    /// capability dichiarata: la prima senza la seconda darebbe un rifiuto
    /// dopo un giro sul filo, e una voce di menù che non fa niente.
    pub const fn can_define(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.definition
    }

    /// Ha senso chiedere completamenti: server attivo e capability dichiarata.
    pub const fn can_complete(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.completion
    }

    /// Ha senso chiedere documentazione: server attivo e capability dichiarata.
    pub const fn can_hover(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.hover
    }

    pub const fn can_signature_help(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.signature_help
    }

    #[cfg(feature = "mcp")]
    pub const fn can_symbols(&self) -> bool {
        matches!(self.state, LspState::Attivo { .. }) && self.availability.document_symbols
    }

    /// Ha senso mandare un `didChange` per questo documento?
    ///
    /// No se il server non lo conosce, e no se gli si manderebbe un testo che
    /// ha già: il debounce collassa la raffica di battute, non i movimenti di
    /// cursore, che scattano lo stesso timer senza cambiare una lettera.
    pub fn wants_change(&self, id: DocId, version: u64) -> Option<Uri> {
        if self.sent.get(&id) == Some(&version) {
            return None;
        }
        self.opened.get(&id).cloned()
    }

    pub fn mark_sent(&mut self, id: DocId, version: u64) {
        self.sent.insert(id, version);
    }

    pub fn mark_opened(&mut self, id: DocId, uri: Uri, version: u64) {
        self.opened.insert(id, uri);
        self.sent.insert(id, version);
    }

    pub fn mark_closed(&mut self, id: DocId) -> Option<Uri> {
        self.sent.remove(&id);
        self.outlines.remove(&id);
        self.asked_outline.remove(&id);
        self.pending.retain(|_, request| request.doc != id);
        self.opened.remove(&id)
    }

    pub fn send(&self, command: LspCommand) -> bool {
        self.mailbox
            .as_ref()
            .is_some_and(|mailbox| mailbox.send(command).is_ok())
    }

    /// I file che questo server sa leggere, per estensione.
    pub fn handles(&self, path: &Path) -> bool {
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|estensione| {
                self.spec.extensions.iter().any(|nota| {
                    nota.trim_start_matches('.')
                        .eq_ignore_ascii_case(estensione)
                })
            })
    }

    pub fn language_id(&self) -> String {
        self.spec
            .languages
            .first()
            .cloned()
            .unwrap_or_else(|| "plaintext".to_owned())
    }
}

/// Quale server serve a questo progetto.
///
/// Prima il file sotto il cursore — è la domanda concreta —, poi i marcatori
/// di progetto (`Cargo.toml` e simili). Nessun ripiego su «il primo della
/// lista»: avviare rust-analyzer su una cartella che non è un progetto Rust
/// vorrebbe dire pagarne l'indice per niente.
pub fn server_for(root: Option<&Path>, active: Option<&Path>) -> Option<&'static ServerSpec> {
    let catalog: &'static ServerCatalog = ServerCatalog::embedded().ok()?;
    if let Some(path) = active
        && let Some(spec) = catalog.for_path(path)
    {
        return Some(spec);
    }
    let root = root?;
    catalog.servers().find(|spec| {
        !spec.workspace_markers.is_empty()
            && spec
                .workspace_markers
                .iter()
                .any(|marker| root.join(marker).exists())
    })
}

/// La riga in barra di stato. Pura, come `watcher_status_text`.
///
/// `None` quando non c'è niente di onesto da dire: nessun server per questo
/// progetto, o funzionalità mai avviata — una riga «fermo» permanente
/// sarebbe rumore, non informazione.
/// Le opzioni con cui si apre la sessione, per server.
///
/// ATTENZIONE: È **una decisione di fiducia**, non una preferenza. Lasciata ai
/// default, rust-analyzer esegue i `build.rs` del progetto, espande le
/// proc-macro e lancia `cargo check` a ogni salvataggio: tre modi di
/// eseguire codice arbitrario di un repository che l'utente ha soltanto
/// **aperto**. In `lume-app` non esiste nessuna decisione di fiducia da cui
/// farli dipendere, quindi sono spenti.
///
/// La conseguenza si dice, non si nasconde: **Problemi mostra la diagnostica
/// nativa di rust-analyzer**, non l'output di `cargo check`. Accenderli è una
/// scelta esplicita, e viene dopo.
pub fn initialization_options(spec: &ServerSpec) -> Option<Value> {
    match spec.id.as_str() {
        "rust-analyzer" => Some(serde_json::json!({
            "cargo": { "buildScripts": { "enable": false } },
            "procMacro": { "enable": false },
            "checkOnSave": false,
        })),
        // tombi (TOML) e rumdl (Markdown) non eseguono niente del progetto:
        // non c'è nulla da spegnere, e inventare opzioni che non conoscono
        // sarebbe rumore nel protocollo.
        _ => None,
    }
}

/// Una domanda in volo: su quale documento, e su quale revisione del testo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InFlight {
    pub doc: DocId,
    pub version: u64,
    /// Le domande di sfondo non mostrano errori transitori all'utente.
    pub quiet: bool,
}

/// Una riga della struttura del documento.
///
/// Porta **due** intervalli e servono tutti e due: `selection` è dove sta il
/// nome — è lì che si atterra cliccando — e `range` è tutto l'elemento, che
/// serve al breadcrumb per chiedere «chi contiene il cursore». Confonderli
/// vuol dire o atterrare sulla graffa di chiusura o un breadcrumb che si
/// accende solo col cursore esattamente sul nome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineRow {
    pub name: String,
    pub detail: Option<String>,
    pub kind: SymbolKind,
    pub depth: usize,
    pub selection: Range,
    pub range: Range,
}

/// La struttura di un documento, con la revisione su cui è stata calcolata.
///
/// Senza la revisione sarebbe una fotografia senza data: dopo tre battute le
/// righe indicherebbero posizioni che non esistono più, e il pannello lo
/// direbbe con la stessa faccia di prima.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outline {
    pub version: u64,
    pub rows: Vec<OutlineRow>,
}

/// Le righe della struttura, da tutte e due le forme della risposta.
///
/// ATTENZIONE: `DocumentSymbolResponse` ha **due** forme e i server scelgono da soli:
/// `Nested` porta l'albero (rust-analyzer), `Flat` una lista piatta con il
/// nome del contenitore accanto (server più vecchi). Gestirne una sola
/// darebbe un pannello **vuoto in silenzio** — un mock, con l'aggravante di
/// sembrare un file senza simboli.
pub fn outline_rows(response: Option<DocumentSymbolResponse>) -> Vec<OutlineRow> {
    let mut righe = Vec::new();
    match response {
        None => {}
        Some(DocumentSymbolResponse::Nested(symbols)) => annida(&symbols, 0, &mut righe),
        Some(DocumentSymbolResponse::Flat(symbols)) => {
            for simbolo in symbols {
                righe.push(OutlineRow {
                    name: simbolo.name,
                    // nella forma piatta l'annidamento **non c'è**: al suo
                    // posto il server dice a chi appartiene il simbolo, e
                    // quella è l'unica gerarchia disponibile
                    detail: simbolo.container_name,
                    kind: simbolo.kind,
                    depth: 0,
                    selection: simbolo.location.range,
                    range: simbolo.location.range,
                });
            }
        }
    }
    righe
}

fn annida(symbols: &[DocumentSymbol], depth: usize, out: &mut Vec<OutlineRow>) {
    for simbolo in symbols {
        out.push(OutlineRow {
            name: simbolo.name.clone(),
            detail: simbolo.detail.clone(),
            kind: simbolo.kind,
            depth,
            selection: simbolo.selection_range,
            range: simbolo.range,
        });
        if let Some(figli) = &simbolo.children {
            annida(figli, depth + 1, out);
        }
    }
}

/// La sigla del genere di simbolo, come la scriverebbe il linguaggio.
///
/// Sta al posto di un'icona per genere: ventisei icone nuove per una colonna
/// larga trenta pixel sarebbero un costo senza contropartita, e la sigla si
/// legge meglio. I generi che non hanno una parola loro cadono su una forma
/// breve invece che su una vuota: una riga senza sigla si allineerebbe male
/// con le sue vicine.
pub const fn kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE | SymbolKind::PACKAGE => "mod",
        SymbolKind::NAMESPACE => "ns",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "fn",
        SymbolKind::PROPERTY => "prop",
        SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "new",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "trait",
        SymbolKind::FUNCTION => "fn",
        SymbolKind::VARIABLE => "let",
        SymbolKind::CONSTANT => "const",
        SymbolKind::STRUCT => "struct",
        SymbolKind::ENUM_MEMBER => "case",
        SymbolKind::TYPE_PARAMETER => "type",
        SymbolKind::OPERATOR => "op",
        _ => "•",
    }
}

/// La catena di simboli che contiene questa posizione, dal più esterno al più
/// interno. Vuota se non ne contiene nessuno.
///
/// Le righe arrivano in ordine di visita, quindi gli antenati precedono
/// sempre i discendenti: basta tenere quelle che contengono la posizione.
pub fn symbol_trail(rows: &[OutlineRow], position: Position) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, riga)| contiene(riga.range, position))
        .map(|(indice, _)| indice)
        .collect()
}

fn contiene(range: Range, position: Position) -> bool {
    let chiave = |p: Position| (p.line, p.character);
    chiave(range.start) <= chiave(position) && chiave(position) <= chiave(range.end)
}

/// Le occorrenze raggruppate per file, in ordine, senza doppioni.
///
/// Tre cose che la lista grezza non ha e che servono tutte e tre:
/// - **raggruppate**: quaranta righe sparse in otto file non si leggono,
///   otto intestazioni con i loro numeri sì;
/// - **in ordine**: i server non lo garantiscono, e un elenco che cambia
///   ordine fra due ricerche uguali sembra rotto;
/// - **senza doppioni**: la stessa posizione può arrivare due volte quando la
///   dichiarazione è anche un utilizzo, e contarla due volte gonfia il badge.
pub fn references_groups(locations: Option<Vec<Location>>, root: Option<&Path>) -> Vec<UsageGroup> {
    /// Quel che si accumula per file mentre si raggruppa: dove sta, e le
    /// posizioni grezze del protocollo.
    type Accumulo = (Option<PathBuf>, Vec<(u32, u32)>);
    let mut per_file: BTreeMap<String, Accumulo> = BTreeMap::new();
    for location in locations.unwrap_or_default() {
        let path = crate::lsp_uri::uri_to_path(&location.uri);
        let etichetta = path
            .as_deref()
            .map(|path| etichetta_file(path, root))
            .unwrap_or_else(|| location.uri.as_str().to_owned());
        let voce = per_file.entry(etichetta).or_insert((path, Vec::new()));
        voce.1
            .push((location.range.start.line, location.range.start.character));
    }
    per_file
        .into_iter()
        .map(|(label, (path, mut hits))| {
            hits.sort_unstable();
            hits.dedup();
            UsageGroup {
                label,
                path,
                // il protocollo conta da 0, la gente da 1
                hits: hits
                    .into_iter()
                    .map(|(riga, colonna)| (riga as usize + 1, colonna as usize + 1))
                    .collect(),
            }
        })
        .collect()
}

/// Un file solo, con le sue occorrenze.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageGroup {
    pub label: String,
    pub path: Option<PathBuf>,
    pub hits: Vec<(usize, usize)>,
}

/// Il nome da mostrare: relativo alla radice quando ci sta sotto, altrimenti
/// il percorso intero. Un utilizzo nella libreria standard **non** si accorcia
/// al solo nome del file: lì l'informazione utile è proprio che sta fuori.
fn etichetta_file(path: &Path, root: Option<&Path>) -> String {
    root.and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Dove porta una risposta a `textDocument/definition`.
///
/// ATTENZIONE: **Tre forme, non una**, ed è lo stesso tranello di
/// `DocumentSymbolResponse`: `Scalar` quando la meta è una,
/// `Array` quando i candidati sono più d'uno, `Link` quando il client ha
/// dichiarato `linkSupport` — una dichiarazione che sta in un altro file e
/// può cambiare senza che questo codice se ne accorga. Gestirne una sola
/// darebbe «nessuna definizione trovata» in silenzio: un mock col vestito
/// buono.
///
/// Dei `Link` si prende il `target_selection_range` — dove sta **il nome** —
/// e non il `target_range`, che è tutto il corpo dell'elemento: atterrare
/// sulla graffa di chiusura di una funzione lunga sarebbe tecnicamente
/// giusto e praticamente inutile.
pub fn definition_targets(response: Option<GotoDefinitionResponse>) -> Vec<(Uri, Range)> {
    match response {
        None => Vec::new(),
        Some(GotoDefinitionResponse::Scalar(location)) => vec![(location.uri, location.range)],
        Some(GotoDefinitionResponse::Array(locations)) => locations
            .into_iter()
            .map(|location| (location.uri, location.range))
            .collect(),
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| (link.target_uri, link.target_selection_range))
            .collect(),
    }
}

/// Picco Job Object in MiB. Sotto un megabyte, se c'è memoria, si dice 1:
/// «0 MB» sarebbe una riga che mente.
pub fn peak_memory_mb(bytes: u64) -> u64 {
    let mb = bytes / (1024 * 1024);
    if bytes > 0 && mb == 0 { 1 } else { mb }
}

pub fn lsp_status_text(workspace: Option<&LspWorkspace>) -> Option<String> {
    let workspace = workspace?;
    let nome = &workspace.spec.name;
    if matches!(workspace.state, LspState::Attivo { .. })
        && let Some(work) = workspace.indexing.label()
    {
        return Some(format!("{nome} · {work}"));
    }
    match &workspace.state {
        LspState::Ferma => None,
        LspState::Ricerca => Some(format!("{nome} · ricerca…")),
        LspState::Avvio => Some(format!("{nome} · avvio…")),
        LspState::Scaricamento {
            received,
            total,
            fase,
        } => Some(format!(
            "{nome} · {}",
            download_text(*received, *total, *fase)
        )),
        LspState::Attivo {
            version,
            memory_mb: Some(mb),
        } if !version.is_empty() => Some(format!("{nome} {version} · {mb} MB")),
        LspState::Attivo {
            version,
            memory_mb: Some(mb),
        } if version.is_empty() => Some(format!("{nome} · {mb} MB")),
        LspState::Attivo { version, .. } if version.is_empty() => Some(nome.clone()),
        LspState::Attivo { version, .. } => Some(format!("{nome} {version}")),
        LspState::Arresto => Some(format!("{nome} · arresto…")),
        LspState::Assente(_) => Some(format!("{nome} · non installato")),
        LspState::Errore(motivo) => Some(format!("{nome} · errore: {motivo}")),
    }
}

/// Cosa fa il tasto d'avvio, viste le condizioni.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspStartAction {
    /// C'è un server e non è avviato: si risolve il binario e si parte.
    Avvia,
    /// Il binario non si trova. **Scaricare è un'azione a parte**: eseguire
    /// codice preso da internet è un consenso esplicito, e vive nel pannello
    /// Estensioni, non qui.
    Scarica,
    /// Già in piedi, o in corso: il tasto non ha niente da fare.
    Niente,
    /// Nessun server per questo progetto.
    NessunServer,
}

pub fn lsp_start_action(workspace: Option<&LspWorkspace>) -> LspStartAction {
    match workspace.map(|workspace| &workspace.state) {
        None => LspStartAction::NessunServer,
        Some(LspState::Ferma) => LspStartAction::Avvia,
        Some(LspState::Assente(_)) => LspStartAction::Scarica,
        Some(LspState::Errore(_)) => LspStartAction::Avvia,
        Some(
            LspState::Ricerca
            | LspState::Avvio
            | LspState::Scaricamento { .. }
            | LspState::Attivo { .. }
            | LspState::Arresto,
        ) => LspStartAction::Niente,
    }
}

/// Cosa serve a `run_with` per costruire lo stream.
///
/// ATTENZIONE: L'identità è **`(server, epoca)` e nient'altro**, ed è scritta a mano:
/// `ServerCommand` non è `Hash` e `InitializeSpec` non può esserlo
/// (`initialization_options` è un `Value`). Ma è anche la cosa giusta —
/// l'identità di uno stream è «quale server, quale accensione», non il
/// contenuto dei suoi parametri: un `PathBuf` diverso nel `cwd` non deve far
/// ripartire un server già vivo.
#[derive(Debug, Clone)]
pub struct LspSpawn {
    pub server_id: String,
    pub epoch: u64,
    pub command: ServerCommand,
    pub spec: std::sync::Arc<InitializeSpec>,
}

impl Hash for LspSpawn {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.server_id.hash(state);
        self.epoch.hash(state);
    }
}

/// Quel che esce dallo stream del language server.
#[derive(Debug, Clone)]
pub enum LspEvent {
    /// Il primo di tutti: la mailbox dell'attore. Prima di questo non c'è
    /// niente a cui parlare, e nessun comando parte.
    Ready(lume_lsp::LspCommandMailbox),
    From(ActorEvent),
}

/// Da un evento dell'attore ai problemi da mostrare.
///
/// Sta qui e non nell'attore perché `publish`/`snapshot` sono **sincroni e
/// testabili**: il negozio delle diagnostiche vive sul lato freddo, dove un
/// test lo può guardare senza far girare niente.
pub fn publish(workspace: &mut LspWorkspace, params: lsp_types::PublishDiagnosticsParams) -> bool {
    let server = workspace.server_id();
    let uri = params.uri.clone();
    let empty = params.diagnostics.is_empty();
    let applied = matches!(
        workspace.problems.publish(server.clone(), params),
        lume_lsp::PublishResult::Applied
    );
    if applied && empty && !workspace.opened.values().any(|opened| opened == &uri) {
        workspace.problems.release(&server, &uri);
    }
    applied
}

/// Quali documenti aperti questo server deve conoscere, e con quale URI.
///
/// Pura, e si prova senza far girare niente: è il punto dove un'estensione
/// dimenticata diventerebbe **un editor muto** — server vivo, nessuna
/// diagnostica, e nessun errore da nessuna parte.
pub fn documents_for<'a>(
    workspace: &LspWorkspace,
    documents: impl IntoIterator<Item = (DocId, &'a Path)>,
) -> Vec<(DocId, Uri)> {
    documents
        .into_iter()
        .filter(|(_, path)| workspace.handles(path))
        .filter_map(|(id, path)| crate::lsp_uri::path_to_uri(path).map(|uri| (id, uri)))
        .collect()
}

/// L'esito della sonda `--version`, sempre fuori dal thread della UI.
/// Dove stanno i server che scarichiamo noi.
///
/// ATTENZIONE: Il qualificatore è **quello dell'app** — lo stesso di `appearance.rs` e
/// di `lume-store`. Una terna diversa qui darebbe una cartella diversa, e il
/// giorno che il pannello Estensioni scaricherà rust-analyzer lo metterebbe
/// dove `resolve` non guarda: installato e invisibile.
pub fn managed_root() -> PathBuf {
    lume_store::application_data_dir()
        .map(|directory| directory.join("language-servers"))
        .unwrap_or_else(|_| PathBuf::from("language-servers"))
}

pub fn install_blocking(
    spec: &'static lume_language_servers::ServerSpec,
    cancel: &CancelFlag,
    on_progress: impl FnMut(lume_language_servers::InstallProgress),
) -> Result<(), String> {
    use lume_language_servers::{Platform, ServerCatalog, ServerInstaller};
    let catalog = ServerCatalog::embedded().map_err(|error| error.to_string())?;
    let server = catalog
        .get(&spec.id)
        .ok_or_else(|| format!("server «{}» non nel catalogo", spec.id))?;
    ServerInstaller::new(managed_root())
        .install(server, &Platform::current(), cancel, on_progress)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// I server che arrivano con lo zip di distribuzione: `Lume\servers\`, accanto
/// a `lume.exe`, con lo stesso schema `id/versione/piattaforma` della cartella
/// gestita (li scrive lo stesso `ServerInstaller`, vedi `lume-server-bundle`).
/// Senza questa radice il locator non li guarderebbe mai e l'utente si
/// sentirebbe proporre «Scarica» per un server che ha già sul disco.
pub fn bundle_root() -> Option<PathBuf> {
    bundle_root_for(&std::env::current_exe().ok()?)
}

fn bundle_root_for(exe: &Path) -> Option<PathBuf> {
    exe.parent().map(|directory| directory.join("servers"))
}

pub fn resolve_blocking(spec: &'static ServerSpec) -> Result<ResolvedServer, String> {
    lume_language_servers::ServerLocator::from_environment(bundle_root(), managed_root())
        .resolve(spec, &lume_language_servers::Platform::current())
        .map_err(|error| motivo_assente(&error))
}

/// Il testo che finisce in `LspState::Assente`, e quindi nel pannello
/// Estensioni: per «nessun candidato utilizzabile», una riga per origine e
/// motivo (`describe_unavailable`). **Mai** il `to_string()` dell'errore:
/// contiene percorsi (il profilo Windows ha il nome utente) e testo del
/// sistema. Il locator restituisce solo `Unavailable`; ogni altra variante
/// è un errore di un singolo candidato e qui vale una frase fissa.
pub fn motivo_assente(error: &lume_language_servers::DiscoveryError) -> String {
    match error {
        lume_language_servers::DiscoveryError::Unavailable { attempts, .. } => {
            lume_language_servers::describe_unavailable(attempts)
        }
        _ => "ricerca del server non riuscita".to_owned(),
    }
}

/// Da dove viene il binario di un server attivo. Quello del `PATH` non ha
/// né hash né ricevuta: lo si dice.
const fn origine_attiva(source: lume_lsp::CommandSource) -> &'static str {
    match source {
        lume_lsp::CommandSource::Bundled => "incluso con Lume",
        lume_lsp::CommandSource::Managed => "scaricato da Lume",
        lume_lsp::CommandSource::SystemPath => "dal PATH, non verificato",
        lume_lsp::CommandSource::Workspace => "dal progetto",
    }
}

pub fn download_text(received: u64, total: u64, fase: FaseScarico) -> String {
    match fase {
        FaseScarico::Verifica => "verifica…".to_owned(),
        FaseScarico::Estrazione => "estrazione…".to_owned(),
        FaseScarico::Scarico if total == 0 => {
            format!("scaricamento… {} MB", mib(received))
        }
        FaseScarico::Scarico => {
            let percento = (u128::from(received) * 100 / u128::from(total)).min(100);
            format!(
                "scaricamento… {} / {} MB ({percento}%)",
                mib(received),
                mib(total)
            )
        }
    }
}

fn mib(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    let decimi = (bytes % MIB) * 10 / MIB;
    format!("{}.{}", bytes / MIB, decimi)
}

/// Limita i messaggi di trasferimento; i cambi di fase e l'ultimo byte passano.
pub struct DownloadThrottle {
    min: Duration,
    last: Option<Instant>,
}

impl DownloadThrottle {
    pub const fn new(min: Duration) -> Self {
        Self { min, last: None }
    }

    pub fn accept(&mut self, now: Instant, event: &lume_language_servers::InstallProgress) -> bool {
        let lume_language_servers::InstallProgress::Downloading { received, total } = event else {
            return true;
        };
        let ultimo = *total > 0 && received >= total;
        let scaduto = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= self.min);
        if ultimo || scaduto {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

pub fn lsp_detail_line(workspace: Option<&LspWorkspace>) -> Option<String> {
    let workspace = workspace?;
    match &workspace.state {
        LspState::Assente(motivo) if motivo.is_empty() => Some("non installato".to_owned()),
        LspState::Assente(motivo) => Some(format!("non installato\n{motivo}")),
        LspState::Ferma => Some("non avviata".to_owned()),
        LspState::Ricerca => Some("ricerca binario…".to_owned()),
        LspState::Avvio => Some("in avvio".to_owned()),
        LspState::Scaricamento {
            received,
            total,
            fase,
        } => Some(download_text(*received, *total, *fase)),
        LspState::Attivo { .. } => Some(match workspace.command.as_ref() {
            Some(command) => format!("attivo · {}", origine_attiva(command.source)),
            None => "attivo".to_owned(),
        }),
        LspState::Arresto => Some("arresto…".to_owned()),
        LspState::Errore(motivo) => Some(format!("errore: {motivo}")),
    }
}

pub fn lsp_start_label(workspace: Option<&LspWorkspace>) -> Option<&'static str> {
    match lsp_start_action(workspace) {
        LspStartAction::Avvia => Some("Avvia"),
        LspStartAction::Scarica => Some("Scarica"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{
        DownloadThrottle, FaseScarico, LspStartAction, LspState, LspWorkspace, Uri,
        bundle_root_for, definition_targets, download_text, initialization_options, kind_label,
        lsp_detail_line, lsp_start_action, lsp_status_text, motivo_assente, outline_rows,
        peak_memory_mb, references_groups, server_for,
    };
    use lume_language_servers::ServerCatalog;

    /// Lo zip di `lume-server-bundle` mette i server in `Lume\servers\`,
    /// accanto a `lume.exe`. Che il locator preferisca il bundle a tutto il
    /// resto lo prova `lume-language-servers/tests/discovery.rs`; qui si prova
    /// che l'app gli passi la cartella giusta.
    #[test]
    fn la_radice_del_bundle_e_servers_accanto_all_exe() {
        let exe = std::path::Path::new("C:/Programmi/Lume/lume.exe");
        assert_eq!(
            bundle_root_for(exe),
            Some(std::path::PathBuf::from("C:/Programmi/Lume/servers"))
        );
    }

    fn rust_analyzer() -> &'static lume_language_servers::ServerSpec {
        ServerCatalog::embedded()
            .unwrap()
            .get("rust-analyzer")
            .unwrap()
    }

    fn tempdir(nome: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lume_{nome}_{nonce}"));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn il_testo_del_download_dice_quanto_manca() {
        let mib = 1024 * 1024;
        assert_eq!(
            download_text(5 * mib, 10 * mib, FaseScarico::Scarico),
            "scaricamento… 5.0 / 10.0 MB (50%)"
        );
        assert_eq!(
            download_text(mib + mib / 2, 30 * mib, FaseScarico::Scarico),
            "scaricamento… 1.5 / 30.0 MB (5%)"
        );
        assert_eq!(
            download_text(3 * mib, 0, FaseScarico::Scarico),
            "scaricamento… 3.0 MB",
            "senza totale non si inventa una percentuale"
        );
        assert_eq!(download_text(0, 0, FaseScarico::Verifica), "verifica…");
        assert_eq!(download_text(0, 0, FaseScarico::Estrazione), "estrazione…");
        assert!(
            download_text(20 * mib, 10 * mib, FaseScarico::Scarico).ends_with("(100%)"),
            "mai oltre il cento"
        );
        assert_eq!(
            download_text(u64::MAX, u64::MAX, FaseScarico::Scarico),
            "scaricamento… 17592186044415.9 / 17592186044415.9 MB (100%)",
            "i contatori massimi non devono saturare prima della percentuale"
        );
    }

    #[test]
    fn il_filtro_del_download_lascia_passare_un_evento_ogni_cento_millisecondi() {
        use lume_language_servers::InstallProgress;
        use std::time::{Duration, Instant};
        let ms = Duration::from_millis;
        let scarico = |received| InstallProgress::Downloading {
            received,
            total: 1000,
        };
        let mut filtro = DownloadThrottle::new(ms(100));
        let t0 = Instant::now();
        assert!(filtro.accept(t0, &scarico(1)));
        assert!(!filtro.accept(t0 + ms(10), &scarico(2)));
        assert!(!filtro.accept(t0 + ms(99), &scarico(3)));
        assert!(filtro.accept(t0 + ms(100), &scarico(4)));
        assert!(
            filtro.accept(t0 + ms(101), &scarico(1000)),
            "l'ultimo byte passa sempre"
        );
        assert!(filtro.accept(t0 + ms(102), &InstallProgress::Verifying));
        assert!(filtro.accept(t0 + ms(102), &InstallProgress::Extracting));
        assert!(filtro.accept(t0 + ms(102), &InstallProgress::Installed));
    }

    #[test]
    fn lo_stato_di_download_e_vivo_non_ha_un_tasto_e_dimenticare_annulla() {
        use lume_language_servers::CancelFlag;
        let mut lsp = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        lsp.state = LspState::Scaricamento {
            received: 1,
            total: 2,
            fase: FaseScarico::Scarico,
        };
        assert!(lsp.state.is_live(), "tiene un thread e un socket");
        assert_eq!(
            lsp_start_action(Some(&lsp)),
            LspStartAction::Niente,
            "premere Avvia due volte non lancia due download"
        );
        assert_eq!(
            lsp_status_text(Some(&lsp)).as_deref(),
            Some("rust-analyzer · scaricamento… 0.0 / 0.0 MB (50%)")
        );
        let flag = CancelFlag::default();
        lsp.download_cancel = Some(flag.clone());
        lsp.forget();
        assert!(
            flag.is_cancelled(),
            "dimenticare il server ferma il download"
        );
        assert!(lsp.download_cancel.is_none(), "e il flag non resta armato");
    }

    /// La barra dice qualcosa **solo quando c'è qualcosa di vero da dire**.
    /// Una riga «fermo» permanente sarebbe rumore, non informazione, e
    /// occuperebbe spazio in una barra alta 26 px.
    #[test]
    fn la_barra_tace_finche_non_c_e_niente_da_dire() {
        assert_eq!(lsp_status_text(None), None, "nessun server: nessuna riga");
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        assert_eq!(lsp_status_text(Some(&workspace)), None, "fermo: muta");

        workspace.state = LspState::Avvio;
        assert_eq!(
            lsp_status_text(Some(&workspace)).as_deref(),
            Some("rust-analyzer · avvio…")
        );
        workspace.version = "1.2.3".to_owned();
        workspace.mark_ready(None);
        assert_eq!(
            lsp_status_text(Some(&workspace)).as_deref(),
            Some("rust-analyzer 1.2.3")
        );
        workspace.mark_ready(Some(48));
        assert_eq!(
            lsp_status_text(Some(&workspace)).as_deref(),
            Some("rust-analyzer 1.2.3 · 48 MB")
        );
        workspace.state = LspState::Assente("non nel PATH".to_owned());
        assert_eq!(
            lsp_status_text(Some(&workspace)).as_deref(),
            Some("rust-analyzer · non installato")
        );
    }

    #[test]
    fn il_picco_di_memoria_si_dice_in_mebibyte() {
        assert_eq!(peak_memory_mb(0), 0);
        assert_eq!(peak_memory_mb(1024), 1);
        assert_eq!(peak_memory_mb(48 * 1024 * 1024), 48);
        assert_eq!(peak_memory_mb(48 * 1024 * 1024 + 1), 48);
    }

    /// Il tasto ha tre destini diversi, e confonderli vorrebbe dire o un
    /// comando inerte o — peggio — un «Avvia» che scarica un binario da
    /// internet senza che nessuno l'abbia chiesto.
    #[test]
    fn il_tasto_sa_quando_avviare_quando_scaricare_e_quando_tacere() {
        assert_eq!(lsp_start_action(None), LspStartAction::NessunServer);
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        assert_eq!(lsp_start_action(Some(&workspace)), LspStartAction::Avvia);
        workspace.state = LspState::Assente("non nel PATH".to_owned());
        assert_eq!(lsp_start_action(Some(&workspace)), LspStartAction::Scarica);
        for stato in [
            LspState::Ricerca,
            LspState::Avvio,
            LspState::Arresto,
            LspState::Attivo {
                version: String::new(),
                memory_mb: None,
            },
        ] {
            workspace.state = stato.clone();
            assert_eq!(
                lsp_start_action(Some(&workspace)),
                LspStartAction::Niente,
                "{stato:?}"
            );
        }
        // dopo un errore si può riprovare: è un guasto, non una condanna
        workspace.state = LspState::Errore("caduto".to_owned());
        assert_eq!(lsp_start_action(Some(&workspace)), LspStartAction::Avvia);
    }

    /// ATTENZIONE: Nessun ripiego su «il primo della lista»: avviare rust-analyzer su
    /// una cartella che non è un progetto Rust vorrebbe dire pagarne l'indice
    /// per niente, che è esattamente quel che questo lavoro serve a evitare.
    #[test]
    fn si_sceglie_il_server_dai_marcatori_o_dal_file_aperto_e_mai_a_caso() {
        let vuota = tempdir("lsp_scelta");
        assert!(
            server_for(Some(&vuota), None).is_none(),
            "cartella qualunque: nessun server"
        );
        // il file sotto il cursore è la domanda concreta, e vince
        assert_eq!(
            server_for(Some(&vuota), Some(&vuota.join("a.rs"))).map(|spec| spec.id.as_str()),
            Some("rust-analyzer")
        );
        std::fs::write(vuota.join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(
            server_for(Some(&vuota), None).map(|spec| spec.id.as_str()),
            Some("rust-analyzer"),
            "il marcatore di progetto basta anche senza file aperti"
        );
        let _ = std::fs::remove_dir_all(&vuota);
    }

    /// ATTENZIONE: È una **decisione di fiducia**, non una preferenza. Lasciato ai
    /// suoi default, rust-analyzer esegue i `build.rs` del progetto, espande
    /// le proc-macro e lancia `cargo check` a ogni salvataggio: tre modi di
    /// eseguire codice arbitrario di un repository che l'utente ha soltanto
    /// **aperto**. Un refuso in una di queste chiavi non darebbe nessun
    /// errore — il server ignorerebbe l'opzione sconosciuta e eseguirebbe.
    #[test]
    fn il_server_rust_non_esegue_niente_del_progetto_che_apre() {
        let opzioni = initialization_options(rust_analyzer()).expect("rust-analyzer le ha");
        for chiave in [
            "/cargo/buildScripts/enable",
            "/procMacro/enable",
            "/checkOnSave",
        ] {
            assert_eq!(
                opzioni.pointer(chiave),
                Some(&serde_json::Value::Bool(false)),
                "{chiave}"
            );
        }
        // e chi non esegue niente del progetto non riceve opzioni inventate
        let tombi = ServerCatalog::embedded().unwrap().get("tombi").unwrap();
        assert!(initialization_options(tombi).is_none());
    }

    /// ATTENZIONE: Tre cose che la lista grezza del server non ha, e servono tutte e
    /// tre: raggruppate (quaranta righe sparse in otto file non si leggono),
    /// **in ordine** (i server non lo garantiscono, e un elenco che cambia
    /// ordine fra due ricerche uguali sembra rotto) e **senza doppioni** (la
    /// stessa posizione arriva due volte quando la dichiarazione è anche un
    /// utilizzo, e contarla due volte gonfia il badge).
    #[test]
    fn gli_utilizzi_si_raggruppano_per_file_in_ordine_e_senza_doppioni() {
        use lsp_types::{Location, Position, Range};
        let root = std::path::Path::new(if cfg!(windows) { "C:/repo" } else { "/repo" });
        let dove = |file: &str, riga: u32, colonna: u32| Location {
            uri: super::super::lsp_uri::path_to_uri(&root.join(file)).unwrap(),
            range: Range::new(
                Position::new(riga, colonna),
                Position::new(riga, colonna + 4),
            ),
        };
        let gruppi = references_groups(
            Some(vec![
                dove("src/z.rs", 9, 2),
                dove("src/a.rs", 5, 1),
                dove("src/a.rs", 1, 0),
                // il doppione: la dichiarazione è anche un utilizzo
                dove("src/a.rs", 1, 0),
            ]),
            Some(root),
        );
        assert_eq!(gruppi.len(), 2);
        assert_eq!(gruppi[0].label, "src/a.rs", "i file in ordine");
        assert_eq!(
            gruppi[0].hits,
            vec![(2, 1), (6, 2)],
            "le occorrenze in ordine, contate da 1, senza doppioni"
        );
        assert_eq!(gruppi[1].label, "src/z.rs");
        assert!(gruppi[0].path.is_some(), "e cliccabili");

        // fuori dalla radice il percorso **non** si accorcia: lì l'informazione
        // utile è proprio che sta fuori
        let fuori = references_groups(
            Some(vec![Location {
                uri: super::super::lsp_uri::path_to_uri(std::path::Path::new(if cfg!(windows) {
                    "C:/rust/library/core/src/option.rs"
                } else {
                    "/rust/library/core/src/option.rs"
                }))
                .unwrap(),
                range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            }]),
            Some(root),
        );
        assert_eq!(fuori.len(), 1);
        assert!(
            fuori[0].label.contains("library/core"),
            "visto: {}",
            fuori[0].label
        );
        assert!(references_groups(None, Some(root)).is_empty());
    }

    /// ATTENZIONE: **Due forme**, e i server scelgono da soli quale mandare:
    /// rust-analyzer manda l'albero, server più vecchi la lista piatta con
    /// il contenitore accanto. Gestirne una sola darebbe un pannello vuoto in
    /// silenzio — peggio di un errore, perché sembrerebbe un file senza
    /// simboli.
    #[test]
    fn la_struttura_si_legge_in_tutte_e_due_le_forme_del_protocollo() {
        use lsp_types::{
            DocumentSymbol, DocumentSymbolResponse, Location, Position, Range, SymbolInformation,
            SymbolKind,
        };
        let riga = |da: u32, a: u32| Range::new(Position::new(da, 0), Position::new(a, 1));

        // ad albero: la profondità viene dall'annidamento
        #[allow(deprecated)]
        let figlio = DocumentSymbol {
            name: "campo".to_owned(),
            detail: Some("u32".to_owned()),
            kind: SymbolKind::FIELD,
            tags: None,
            deprecated: None,
            range: riga(2, 3),
            selection_range: riga(2, 2),
            children: None,
        };
        #[allow(deprecated)]
        let padre = DocumentSymbol {
            name: "Cosa".to_owned(),
            detail: None,
            kind: SymbolKind::STRUCT,
            tags: None,
            deprecated: None,
            range: riga(1, 5),
            selection_range: riga(1, 1),
            children: Some(vec![figlio]),
        };
        let righe = outline_rows(Some(DocumentSymbolResponse::Nested(vec![padre])));
        assert_eq!(righe.len(), 2);
        assert_eq!((righe[0].name.as_str(), righe[0].depth), ("Cosa", 0));
        assert_eq!((righe[1].name.as_str(), righe[1].depth), ("campo", 1));
        assert_eq!(
            righe[0].selection,
            riga(1, 1),
            "cliccando si atterra sul nome, non sulla graffa di chiusura"
        );
        assert_eq!(
            righe[0].range,
            riga(1, 5),
            "e il breadcrumb usa tutto il corpo"
        );
        assert_eq!(kind_label(righe[0].kind), "struct");

        // piatta: nessun annidamento, e il contenitore prende il posto del
        // dettaglio perché è l'unica gerarchia che il server dà
        #[allow(deprecated)]
        let piatta = SymbolInformation {
            name: "campo".to_owned(),
            kind: SymbolKind::FIELD,
            tags: None,
            deprecated: None,
            location: Location {
                uri: "file:///x/main.rs".parse().unwrap(),
                range: riga(2, 3),
            },
            container_name: Some("Cosa".to_owned()),
        };
        let righe = outline_rows(Some(DocumentSymbolResponse::Flat(vec![piatta])));
        assert_eq!(righe.len(), 1);
        assert_eq!(righe[0].depth, 0);
        assert_eq!(righe[0].detail.as_deref(), Some("Cosa"));
        assert!(outline_rows(None).is_empty());
    }

    /// Il breadcrumb chiede «chi mi contiene», dal più esterno al più
    /// interno. ATTENZIONE: Va usato il **corpo** del simbolo e non il suo nome: col
    /// solo nome la catena si accenderebbe solo col cursore esattamente
    /// sopra l'identificatore, cioè quasi mai.
    #[test]
    fn la_catena_dei_simboli_va_dal_piu_esterno_al_piu_interno() {
        use lsp_types::{Position, Range, SymbolKind};
        let riga = |da: u32, a: u32| Range::new(Position::new(da, 0), Position::new(a, 1));
        let fai = |name: &str, depth: usize, range: Range| super::OutlineRow {
            name: name.to_owned(),
            detail: None,
            kind: SymbolKind::FUNCTION,
            depth,
            selection: Range::new(range.start, range.start),
            range,
        };
        let righe = vec![
            fai("Cosa", 0, riga(1, 20)),
            fai("metodo", 1, riga(3, 9)),
            fai("altro", 1, riga(11, 18)),
        ];
        let nomi = |line: u32| -> Vec<&str> {
            super::symbol_trail(&righe, Position::new(line, 2))
                .into_iter()
                .map(|indice| righe[indice].name.as_str())
                .collect()
        };
        assert_eq!(nomi(5), vec!["Cosa", "metodo"]);
        assert_eq!(nomi(12), vec!["Cosa", "altro"]);
        assert_eq!(nomi(2), vec!["Cosa"], "fra i metodi resta il contenitore");
        assert!(nomi(40).is_empty(), "fuori da tutto: nessuna briciola");
    }

    /// ATTENZIONE: Tre forme, e gestirne una sola darebbe «nessuna definizione
    /// trovata» in silenzio — un mock col vestito buono. Quale arriva non
    /// dipende da noi: `Scalar` con una meta sola, `Array` con più
    /// candidati, `Link` se il client ha dichiarato `linkSupport`, che è una
    /// riga in un altro file e può cambiare senza che questo codice lo sappia.
    #[test]
    fn una_definizione_si_legge_in_tutte_e_tre_le_forme_del_protocollo() {
        use lsp_types::{GotoDefinitionResponse, Location, LocationLink, Range};
        let uri: Uri = "file:///x/main.rs".parse().unwrap();
        let dove = Range::new(
            lsp_types::Position::new(3, 4),
            lsp_types::Position::new(3, 9),
        );
        let altrove = Range::new(
            lsp_types::Position::new(0, 0),
            lsp_types::Position::new(9, 1),
        );

        assert_eq!(
            definition_targets(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: dove,
            }))),
            vec![(uri.clone(), dove)]
        );
        assert_eq!(
            definition_targets(Some(GotoDefinitionResponse::Array(vec![
                Location {
                    uri: uri.clone(),
                    range: dove,
                },
                Location {
                    uri: uri.clone(),
                    range: altrove,
                },
            ])))
            .len(),
            2,
            "i candidati non si perdono per strada"
        );
        // ATTENZIONE: del `Link` si prende dove sta **il nome**, non tutto il corpo:
        // atterrare sulla graffa di chiusura di una funzione lunga sarebbe
        // tecnicamente giusto e praticamente inutile
        assert_eq!(
            definition_targets(Some(GotoDefinitionResponse::Link(vec![LocationLink {
                origin_selection_range: None,
                target_uri: uri.clone(),
                target_range: altrove,
                target_selection_range: dove,
            }]))),
            vec![(uri, dove)]
        );
        assert!(
            definition_targets(None).is_empty(),
            "«non lo so» è una risposta, non un guasto"
        );
    }

    /// Il server legge i file che dichiara, e **solo** quelli: mandargli un
    /// `.png` sarebbe lavoro buttato da entrambe le parti.
    #[test]
    fn si_annunciano_solo_i_file_che_il_server_dichiara() {
        let workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        assert!(workspace.handles(std::path::Path::new("/x/main.rs")));
        assert!(workspace.handles(std::path::Path::new("/x/MAIN.RS")));
        assert!(!workspace.handles(std::path::Path::new("/x/logo.png")));
        assert!(!workspace.handles(std::path::Path::new("/x/senza-estensione")));
        assert_eq!(workspace.language_id(), "rust");
    }

    /// Il locator vero, su cartelle vere ma vuote: niente server, e il
    /// motivo che ne esce deve arrivare fino alla riga del pannello.
    #[test]
    fn il_motivo_del_non_installato_arriva_alla_riga_del_pannello() {
        let root = tempdir("lsp_motivo");
        let locator = lume_language_servers::ServerLocator::new(
            None,
            root.join("gestiti"),
            vec![root.clone()],
        );
        let errore = locator
            .resolve(rust_analyzer(), &lume_language_servers::Platform::current())
            .unwrap_err();

        let motivo = motivo_assente(&errore);

        assert_eq!(motivo, "Scaricato da Lume: non trovato\nPATH: non trovato");
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        workspace.state = LspState::Assente(motivo);
        assert_eq!(
            lsp_detail_line(Some(&workspace)).as_deref(),
            Some("non installato\nScaricato da Lume: non trovato\nPATH: non trovato")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Il motivo finisce in un pannello che l'utente può condividere: né i
    /// percorsi dei candidati né il testo grezzo degli errori (il profilo
    /// Windows contiene il nome utente) devono arrivarci, per nessuna variante.
    #[test]
    fn il_motivo_mostrato_non_contiene_percorsi_ne_testo_grezzo() {
        use lume_language_servers::{AttemptFailure, DiscoveryAttempt, DiscoveryError};
        let segreto = std::path::PathBuf::from(r"C:\Users\mario.rossi\AppData\Local\Lume\ra.exe");
        let errore = DiscoveryError::Unavailable {
            server: "rust-analyzer".to_owned(),
            attempts: vec![DiscoveryAttempt {
                source: lume_lsp::CommandSource::Managed,
                path: segreto.clone(),
                error: Some(format!(
                    "I/O error at {}: accesso negato",
                    segreto.display()
                )),
                failure: Some(AttemptFailure::Unreadable),
            }],
        };
        assert_eq!(motivo_assente(&errore), "Scaricato da Lume: non leggibile");

        for errore in [
            DiscoveryError::NotFound(segreto.clone()),
            DiscoveryError::NotFile(segreto.clone()),
            DiscoveryError::InvalidReceipt(segreto.clone()),
            DiscoveryError::HashMismatch(segreto.clone()),
            DiscoveryError::Io(segreto.clone(), "accesso negato".to_owned()),
            DiscoveryError::Probe(segreto.clone(), "riga ostile".to_owned()),
            DiscoveryError::ProbeTimeout(segreto.clone()),
            DiscoveryError::ProbeOutputTooLarge(segreto.clone()),
        ] {
            let motivo = motivo_assente(&errore);
            assert!(!motivo.contains("mario"), "{errore:?} -> {motivo}");
            assert!(!motivo.contains("ostile"), "{errore:?} -> {motivo}");
            assert!(!motivo.contains("negato"), "{errore:?} -> {motivo}");
            assert!(!motivo.is_empty(), "{errore:?}");
        }
    }

    #[test]
    fn un_motivo_vuoto_lascia_la_riga_breve() {
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        workspace.state = LspState::Assente(String::new());
        assert_eq!(
            lsp_detail_line(Some(&workspace)).as_deref(),
            Some("non installato")
        );
    }

    /// La barra di stato (26 px) resta corta: il motivo sta solo nel pannello.
    #[test]
    fn la_barra_non_riceve_il_motivo() {
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        workspace.state = LspState::Assente("Scaricato da Lume: non trovato".to_owned());
        assert_eq!(
            lsp_status_text(Some(&workspace)).as_deref(),
            Some("rust-analyzer · non installato")
        );
    }

    fn progress(value: lsp_types::WorkDoneProgress) -> lsp_types::ProgressParams {
        lsp_types::ProgressParams {
            token: lsp_types::NumberOrString::String("roots".to_owned()),
            value: lsp_types::ProgressParamsValue::WorkDone(value),
        }
    }

    #[test]
    fn l_indicizzazione_si_legge_dalla_barra_e_sparisce_alla_fine() {
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        workspace.state = LspState::Attivo {
            version: String::new(),
            memory_mb: None,
        };
        assert!(workspace.indexing.is_empty());
        assert!(
            workspace
                .indexing
                .apply(&progress(lsp_types::WorkDoneProgress::Begin(
                    lsp_types::WorkDoneProgressBegin {
                        title: "Indexing".to_owned(),
                        percentage: Some(0),
                        ..Default::default()
                    }
                )))
        );
        assert_eq!(
            lsp_status_text(Some(&workspace)).as_deref(),
            Some("rust-analyzer · Indexing 0%")
        );
        assert!(
            workspace
                .indexing
                .apply(&progress(lsp_types::WorkDoneProgress::Report(
                    lsp_types::WorkDoneProgressReport {
                        percentage: Some(45),
                        message: Some("12/40".to_owned()),
                        ..Default::default()
                    }
                )))
        );
        assert_eq!(
            lsp_status_text(Some(&workspace)).as_deref(),
            Some("rust-analyzer · Indexing 45% · 12/40")
        );
        assert!(
            workspace
                .indexing
                .apply(&progress(lsp_types::WorkDoneProgress::End(
                    lsp_types::WorkDoneProgressEnd::default()
                )))
        );
        assert!(workspace.indexing.is_empty());
        assert_eq!(
            lsp_status_text(Some(&workspace)).as_deref(),
            Some("rust-analyzer")
        );
    }

    #[test]
    fn un_passo_di_un_gettone_mai_iniziato_non_inventa_un_indicatore() {
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        assert!(
            !workspace
                .indexing
                .apply(&progress(lsp_types::WorkDoneProgress::Report(
                    lsp_types::WorkDoneProgressReport::default()
                )))
        );
        assert!(workspace.indexing.is_empty());
    }

    #[test]
    fn il_server_attivo_dice_da_dove_viene_il_binario() {
        use lume_lsp::{CommandSource, ServerCommand};
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        workspace.state = LspState::Attivo {
            version: "1.2.3".to_owned(),
            memory_mb: None,
        };
        assert_eq!(
            lsp_detail_line(Some(&workspace)).as_deref(),
            Some("attivo"),
            "senza comando risolto non si inventa un'origine"
        );
        for (source, atteso) in [
            (CommandSource::Bundled, "attivo · incluso con Lume"),
            (CommandSource::Managed, "attivo · scaricato da Lume"),
            (
                CommandSource::SystemPath,
                "attivo · dal PATH, non verificato",
            ),
            (CommandSource::Workspace, "attivo · dal progetto"),
        ] {
            workspace.command = Some(ServerCommand {
                program: std::path::PathBuf::from("rust-analyzer"),
                args: Vec::new(),
                cwd: None,
                env: Vec::new(),
                source,
            });
            assert_eq!(lsp_detail_line(Some(&workspace)).as_deref(), Some(atteso));
        }
    }

    #[test]
    fn una_pubblicazione_vuota_per_un_file_chiuso_rilascia_la_generazione() {
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        let uri: Uri = "file:///c:/p/chiuso.rs".parse().unwrap();
        let con = |diagnostics| lsp_types::PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics,
            version: None,
        };
        let una = vec![lsp_types::Diagnostic {
            message: "boom".to_owned(),
            ..Default::default()
        }];
        assert!(super::publish(&mut workspace, con(una)));
        assert_eq!(workspace.problems.generation_count(), 1);
        assert!(super::publish(&mut workspace, con(Vec::new())));
        assert_eq!(workspace.problems.generation_count(), 0);
    }

    #[test]
    fn una_pubblicazione_vuota_per_un_file_aperto_resta_registrata() {
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        let uri: Uri = "file:///c:/p/aperto.rs".parse().unwrap();
        workspace.mark_opened(lume_core::DocId(1), uri.clone(), 1);
        assert!(super::publish(
            &mut workspace,
            lsp_types::PublishDiagnosticsParams {
                uri,
                diagnostics: Vec::new(),
                version: Some(1)
            }
        ));
        assert_eq!(workspace.problems.generation_count(), 1);
    }

    #[test]
    fn chiudere_un_documento_pota_la_sua_struttura() {
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        workspace.state = LspState::Attivo {
            version: String::new(),
            memory_mb: None,
        };
        workspace.availability.document_symbols = true;
        let id = lume_core::DocId(7);
        let uri: Uri = "file:///c:/p/x.rs".parse().unwrap();
        workspace.mark_opened(id, uri.clone(), 1);
        workspace.mark_outline_asked(id, 1);
        workspace.store_outline(
            id,
            super::Outline {
                version: 1,
                rows: Vec::new(),
            },
        );
        assert!(workspace.outline_of(id, 1).is_some());
        assert!(!workspace.wants_outline(id, 1));
        assert!(workspace.mark_closed(id).is_some());
        assert!(workspace.outline_of(id, 1).is_none());
        workspace.mark_opened(id, uri, 1);
        assert!(workspace.wants_outline(id, 1));
    }

    #[test]
    fn chiudere_un_documento_revoca_le_richieste_sul_suo_uri() {
        let mut workspace = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        let id = lume_core::DocId(7);
        let uri: Uri = "file:///c:/p/x.rs".parse().unwrap();
        workspace.mark_opened(id, uri, 1);
        let request = workspace.ask(id, 1);

        let _ = workspace.mark_closed(id);

        assert!(
            workspace.answered(request).is_none(),
            "la risposta non deve superare la chiusura o il rebind del documento"
        );
    }

    #[test]
    fn completamento_e_hover_chiedono_server_attivo_e_capability() {
        use lsp_types::{
            CompletionOptions, HoverProviderCapability, ServerCapabilities, SignatureHelpOptions,
        };
        let mut lsp = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        assert!(
            !lsp.can_complete() && !lsp.can_hover() && !lsp.can_signature_help(),
            "spento: niente"
        );
        lsp.availability = lume_lsp::FeatureAvailability::from_capabilities(&ServerCapabilities {
            completion_provider: Some(CompletionOptions::default()),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            signature_help_provider: Some(SignatureHelpOptions::default()),
            ..ServerCapabilities::default()
        });
        assert!(
            !lsp.can_complete() && !lsp.can_hover() && !lsp.can_signature_help(),
            "capability dichiarate ma server non attivo: niente"
        );
        lsp.state = LspState::Attivo {
            version: String::new(),
            memory_mb: None,
        };
        assert!(lsp.can_complete() && lsp.can_hover() && lsp.can_signature_help());
        lsp.availability = lume_lsp::FeatureAvailability::default();
        assert!(
            !lsp.can_complete() && !lsp.can_hover() && !lsp.can_signature_help(),
            "capability assente"
        );
    }

    #[test]
    fn una_domanda_silenziosa_si_ricorda_come_tale() {
        use lume_core::DocId;
        let mut lsp = LspWorkspace::new(rust_analyzer(), std::path::PathBuf::from("."));
        let normale = lsp.ask(DocId(1), 3);
        let muta = lsp.ask_quiet(DocId(1), 3);
        assert_ne!(normale, muta);
        assert!(!lsp.answered(normale).unwrap().quiet);
        assert!(lsp.answered(muta).unwrap().quiet);
        assert!(lsp.answered(muta).is_none(), "si ritira una volta sola");
    }
}
