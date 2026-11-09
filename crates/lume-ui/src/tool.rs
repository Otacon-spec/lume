//! Separation of concerns: ogni icona della activity bar è un Tool autonomo
//! (icona + titolo + pannello + stato + eventi propri). Le barre e l'app non
//! conoscono i tool concreti: li leggono dal `Registry`. Un plugin futuro è
//! semplicemente un altro `Box<dyn Tool>` registrato.

use std::any::Any;
use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::Element;
use lume_theme::LumeTheme;

use crate::file_icons::FileIconSource;
use crate::icons::Icon;

/// Identità stabile di un tool (stringa: pronta per plugin esterni).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolId(pub &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Dove vive il contenuto del tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Pannello laterale (si apre sul lato della barra che contiene l'icona).
    SidePanel,
    /// Dock in fondo alla colonna centrale (Terminale, Log Git, Problemi):
    /// un solo tool alla volta, l'icona fa toggle (stesso click = chiude).
    Dock,
}

/// Altezza del dock: (min, default, max) — misure del prototipo.
pub const DOCK_HEIGHT_RANGE: (f32, f32, f32) = (180.0, 300.0, 500.0);

/// Evento opaco di un tool: ogni tool definisce il proprio enum e fa downcast.
#[derive(Clone)]
pub struct ToolEvent(Arc<dyn Any + Send + Sync>);

impl ToolEvent {
    pub fn new<T: Any + Send + Sync>(ev: T) -> Self {
        Self(Arc::new(ev))
    }

    pub fn downcast<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for ToolEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ToolEvent(..)")
    }
}

/// Contesto passato alle view dei tool (solo lettura).
pub struct ToolCtx<'a> {
    pub theme: &'a LumeTheme,
    /// File attivo nell'editor, se esiste (per evidenziare la riga dell'albero).
    pub active_path: Option<&'a Path>,
    /// Altezza del corpo per i tool Dock (sotto l'header); None altrove.
    pub body_height: Option<f32>,
    /// Resolver condiviso del tema icone attivo.
    pub file_icons: Option<&'a dyn FileIconSource>,
    /// Diff aperto nell'area centrale: `(path, commit)`, con `commit: None`
    /// per la base «Modifiche locali». Serve al pannello Commit e al Log Git
    /// per evidenziare la riga aperta — il diff lo possiede l'app, i tool lo
    /// leggono e basta.
    pub open_diff: Option<(&'a Path, Option<&'a str>)>,
}

/// Stringa che non si stampa. `ToolRequest` deriva `Debug` e passa per una
/// coda: basterebbe un `{:?}` di diagnostica, oggi o fra un anno, perché una
/// password finisse in un log. Qui non può, per costruzione.
#[derive(Clone, Default)]
pub struct Redacted(zeroize::Zeroizing<String>);

impl Redacted {
    pub fn new(value: String) -> Self {
        Self(zeroize::Zeroizing::new(value))
    }

    /// Il testo, senza consumare il guscio: serve a `text_input`, che vuole
    /// un `&str` a ogni frame. Non salvarlo né stamparlo.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consuma il guscio. Chiamarlo è il punto in cui il segreto torna
    /// leggibile: deve restare raro e visibile in revisione. Il guscio resta
    /// vuoto, quindi il suo `Drop` non ha più nulla da azzerare: da qui in
    /// poi l'azzeramento è affare di chi riceve la `String` (di norma
    /// `lume_sql::Secret::new`).
    pub fn expose(mut self) -> String {
        std::mem::take(&mut *self.0)
    }
}

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("«redatto»")
    }
}

/// Un'azione Jujutsu richiesta dal pannello; `lume-app` la traduce in un
/// comando del crate `lume-jj`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JjAction {
    Describe(String),
    New(Option<String>),
    Push,
}

impl JjAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Describe(_) => "Descrizione",
            Self::New(_) => "Nuovo cambiamento",
            Self::Push => "Push",
        }
    }
}

/// Un'azione Git richiesta dalla UI e tradotta in `lume-app`.
///
/// Il tipo usa solo stringhe e indici: `lume-ui` non dipende da `lume-git`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitAction {
    Fetch,
    Pull,
    Push,
    MergeUpstream,
    MergeContinue(String),
    MergeAbort,
    /// Unisce un branch locale esistente (azione Git del pannello).
    MergeBranch(String),
    SwitchBranch(String),
    SwitchRemoteBranch {
        remote: String,
        name: String,
    },
    StashPush(Option<String>),
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
}

impl GitAction {
    pub fn needs_git_cli(&self) -> bool {
        matches!(
            self,
            Self::Push
                | Self::MergeUpstream
                | Self::MergeContinue(_)
                | Self::MergeAbort
                | Self::MergeBranch(_)
                | Self::SwitchBranch(_)
                | Self::SwitchRemoteBranch { .. }
                | Self::StashPush(_)
                | Self::StashApply(_)
                | Self::StashPop(_)
        )
    }

    pub fn touches_stash(&self) -> bool {
        matches!(
            self,
            Self::StashPush(_) | Self::StashApply(_) | Self::StashPop(_) | Self::StashDrop(_)
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Fetch => "Fetch in corso…",
            Self::Pull => "Pull in corso…",
            Self::Push => "Push in corso…",
            Self::MergeUpstream => "Merge in corso…",
            Self::MergeContinue(_) => "Conclusione del merge…",
            Self::MergeAbort => "Annullamento del merge…",
            Self::MergeBranch(_) => "Merge del branch…",
            Self::SwitchBranch(_) | Self::SwitchRemoteBranch { .. } => "Cambio di branch…",
            Self::StashPush(_) => "Salvataggio dello stash…",
            Self::StashApply(_) => "Applicazione dello stash…",
            Self::StashPop(_) => "Ripristino dello stash…",
            Self::StashDrop(_) => "Eliminazione dello stash…",
        }
    }
}

/// Identità della scansione che ha prodotto una vista Appunti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotesOrigin(pub u64);

/// Richieste che un tool può fare all'applicazione dopo un proprio evento.
#[derive(Debug, Clone)]
pub enum ToolRequest {
    NotesLoad,
    /// Il pannello Appunti si è chiuso: l'app scarica la cache della vista
    /// (riepiloghi, note caricate) tenendo il minimo per salvare le schede
    /// ancora aperte.
    NotesUnload,
    NotesCreate(NotesOrigin),
    NotesOpen(NotesOrigin, lume_notes::NoteId),
    NotesDelete(NotesOrigin, lume_notes::NoteId),
    NotesChooseDirectory,
    OpenFile(PathBuf),
    OpenFileAt {
        path: PathBuf,
        line: usize,
    },
    /// Apri il menù contestuale della riga su cui si è premuto il tasto
    /// destro. `x`/`y` sono il punto del clic, in coordinate della finestra:
    /// il menù si apre dove si è premuto, e in iced 0.14 l'evento di
    /// pressione non lo porta.
    ContextMenu {
        path: PathBuf,
        is_dir: bool,
        x: f32,
        y: f32,
    },
    /// Chiudi il pannello/dock di questo tool (es. «×» o «—»).
    CloseSelf,
    /// Togli il segnalibro all'indice (la lista è dell'app).
    RemoveBookmark(usize),
    /// Avvia una ricerca nel progetto; i risultati tornano come ToolEvent.
    ///
    /// ATTENZIONE: `area` è l'ambito, e viaggia come **scelta** e non come lista:
    /// quali documenti siano aperti lo sa l'app, non il pannello, ed è anche
    /// l'unica delle due cose che può cambiare fra la richiesta e il giro.
    RunSearch {
        options: lume_core::SearchOptions,
        area: lume_core::SearchArea,
    },
    /// Ferma la ricerca in corso e l'attesa di quiete prima della prossima:
    /// il pannello si è chiuso e i risultati non li guarderebbe nessuno.
    CancelSearch,
    /// Sostituisce le occorrenze **già mostrate**, e nient'altro.
    ///
    /// ATTENZIONE: Viaggiano i risultati, non il testo cercato: ricalcolare il pattern
    /// al momento della sostituzione vorrebbe dire scrivere su file che
    /// l'utente non ha mai visto in elenco — l'albero può essere cambiato fra
    /// la ricerca e il clic, e chi ha confermato «in 12 file» ne intendeva
    /// dodici precisi.
    ReplaceAll {
        matches: Vec<lume_core::FileMatches>,
        replacement: String,
    },
    GitRefresh,
    /// Più path insieme: la casella di gruppo mette in stage tutto il gruppo,
    /// e farlo con una scrittura sola dell'index invece di N è la differenza
    /// fra istantaneo e visibilmente lento.
    GitStage(Vec<PathBuf>),
    GitUnstage(Vec<PathBuf>),
    /// Butta via le modifiche locali dei path. Distruttivo: chi lo manda ha
    /// già chiesto conferma.
    GitDiscard(Vec<PathBuf>),
    GitCommit {
        message: String,
        /// Riscrive HEAD invece di aggiungere un commit.
        amend: bool,
    },
    /// Apri il visualizzatore di diff. `commit: None` è la base «Modifiche
    /// locali»; con l'hash è quel commit contro il suo primo parent.
    ///
    /// Il tipo è `Option<String>` e non un enum di `lume-git` di proposito:
    /// `lume-ui` non dipende da `lume-git` e non è il caso di iniziare ora.
    GitOpenDiff {
        path: PathBuf,
        commit: Option<String>,
    },
    /// Carica branch + log dal riferimento («HEAD», «main», «origin/main»).
    GitLog(String),
    /// File modificati del commit (hash completo).
    GitCommitFiles(String),
    OpenExternal(String),
    /// Sostituisce lo scope del watcher progetto con le sole directory
    /// espanse e visibili comunicate dal tool filesystem.
    SetProjectWatchDirectories(Vec<PathBuf>),
    /// Espandi/comprimi la connessione all'indice: non è solo stato di vista,
    /// espandere richiede di interrogare lo schema.
    SqlToggleConnection(usize),
    /// Espandi/comprimi la tabella (connessione, tabella): carica le colonne.
    SqlToggleTable(usize, usize),
    /// Apri la console `console.sql`; con `table` precompila un SELECT.
    SqlOpenConsole {
        connection: Option<String>,
        table: Option<String>,
    },
    SqlRefreshSchema,
    SqlImportIdea,
    SqlDetectIdea,
    SqlConfirmIdea,
    SqlIdeaUser {
        index: usize,
        user: String,
    },
    SqlCancelIdea,
    /// Password (o client secret) digitata nel campo del pannello Database.
    /// Viaggia una sola volta, alla conferma: non c'è nessun `SecretChanged`
    /// per battuta, così il segreto non attraversa la coda dei messaggi a
    /// ogni tasto.
    SqlSecretSubmitted(Redacted),
    SqlSecretCancelled,
    /// Nuova origine dati confermata nel modulo del pannello Database.
    SqlCreateConnection(SqlConnectionDraft),
    /// Elimina il profilo all'indice. Non chiude nulla di irreversibile: il
    /// segreto non è mai stato salvato, e i dati restano nel database.
    SqlDeleteConnection(usize),
    /// Il pannello Estensioni accende o spegne una funzionalità.
    ///
    /// La chiave è la stessa di [`Tool::feature`]: `lume-ui` non conosce le
    /// funzionalità concrete di `lume-app`, e non deve — stessa scelta già
    /// fatta per `ToolId`.
    SetFeatureEnabled {
        feature: &'static str,
        enabled: bool,
    },
    /// «Ferma» dal pannello Estensioni. Chi lo riceve decide se serve una
    /// conferma: il pannello non la conosce, la **mostra** e basta.
    StopFeature(&'static str),
    /// «Avvia»: il verso opposto, che esiste solo per chi si accende **a
    /// comando** — il terminale e il database si accendono aprendo una
    /// risorsa, e un «Avvia» per loro non saprebbe quale.
    StartFeature(&'static str),
    /// La conferma di stop è stata annullata.
    CancelStopFeature,
    /// Copia negli appunti una configurazione preparata dall'app.
    WriteClipboard(String),
    // ── Blame ────────────────────────────────────────────────────
    /// Apri il pannello blame per il file indicato.
    GitBlame(PathBuf),
    /// Calcola il blame allineato al buffer dell'editor, senza usare il pannello Git.
    GitBlameFile(PathBuf),
    /// Chiudi il pannello blame.
    GitCloseBlame,
    // ── Stash ────────────────────────────────────────────────────
    /// Esegue un'azione Git sul worker condiviso.
    GitAction(GitAction),
    /// Annulla l'azione Git attualmente in corso.
    GitCancel,
    /// Ricarica la lista degli stash (solo lettura).
    GitStashRefresh,
    // ── RAM ─────────────────────────────────────────────────────
    /// Rileggi il picco di working set del processo Lume (`GetProcessMemoryInfo`).
    RefreshPeakMemory,
    /// Carica cambiamenti e stato del workspace jj.
    JjLoad,
    /// Lancia un'azione jj.
    JjAction(JjAction),
    /// Annulla il caricamento o l'azione jj in corso.
    JjCancel,
    // ── Debugger ─────────────────────────────────────────────────
    /// Un gesto del pannello Debug; `lume-app` lo traduce in DAP.
    Debug(DebugAction),
}

/// I gesti del pannello Debug (piano 17). Solo indici, percorsi e testo:
/// `lume-ui` non dipende da `lume-dap`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugAction {
    /// Rilegge `.lume/launch.toml` e cerca di nuovo l'adattatore.
    ReloadConfigs,
    /// Sceglie la configurazione di lancio.
    SelectConfig(usize),
    /// Avvia la configurazione scelta (chiede fiducia la prima volta).
    Start,
    /// Conferma la fiducia chiesta da `Start`.
    TrustAndStart,
    /// Annulla la richiesta di fiducia.
    CancelTrust,
    Continue,
    Pause,
    StepOver,
    StepIn,
    StepOut,
    Stop,
    /// Storia degli arresti: precedente, successivo, ultimo.
    HistoryPrevious,
    HistoryNext,
    HistoryLatest,
    /// Frame della cascata, contato dal più interno (0).
    SelectFrame(usize),
    /// Espande o chiude una variabile strutturata.
    ToggleVariable(i64),
    Evaluate(String),
    OpenSource {
        path: PathBuf,
        line: u32,
    },
    ClearConsole,
    /// Toglie un punto di interruzione dall'elenco.
    RemoveBreakpoint {
        path: PathBuf,
        line: u32,
    },
    /// Pannello Maven: esegue i goal con i profili attivi, nel Terminale.
    MavenEsegui {
        goal: Vec<String>,
        profili: Vec<String>,
    },
}

/// Modulo «nuova origine dati». Solo la parte **non segreta**: la password
/// si chiede alla connessione e non passa mai di qui.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqlConnectionDraft {
    pub label: String,
    /// `false` = Azure SQL, `true` = Oracle. Due motori, un interruttore.
    pub oracle: bool,
    pub host: String,
    pub database: String,
    /// `false` = login SQL, `true` = service principal Entra (solo Azure).
    pub service_principal: bool,
    pub user: String,
    pub tenant_id: String,
    pub client_id: String,
}

pub trait Tool {
    fn id(&self) -> ToolId;
    fn title(&self) -> &'static str;
    fn icon(&self) -> Icon;
    fn kind(&self) -> ToolKind {
        ToolKind::SidePanel
    }
    /// Badge nell'header del dock (es. numero di problemi); None = nessuno.
    fn badge(&self) -> Option<String> {
        None
    }
    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent>;
    fn update(&mut self, _ev: &ToolEvent) -> Vec<ToolRequest> {
        Vec::new()
    }
    /// Blocco di funzionalità a cui il tool appartiene; `None` = nucleo,
    /// sempre disponibile.
    ///
    /// È una stringa e non un enum perché `lume-ui` non deve conoscere le
    /// funzionalità concrete di `lume-app` — stessa scelta già fatta per
    /// `ToolId`. Spegnendo la funzionalità, i suoi tool spariscono dalle
    /// barre ma **restano registrati**: riaccendendola l'icona torna dov'era.
    fn feature(&self) -> Option<&'static str> {
        None
    }

    /// Risorse vive che il tool sta tenendo, per il pannello Estensioni.
    /// `None` = non ne tiene, e non si mostra alcun numero: **chi non sa
    /// contarle non risponde**, non stima. L'unità («sessione»,
    /// «connessione») la sa il chiamante, non il tool.
    fn live_count(&self) -> Option<usize> {
        None
    }

    /// Il tool ha ancora una sorgente asincrona che può produrre eventi senza
    /// che l'utente faccia niente (un lettore PTY, un canale aperto): finché
    /// risponde `true`, e la vista è aperta, l'app tiene vivo il ponte che la
    /// sveglia. È una domanda diversa da `live_count`: un processo già uscito
    /// non è «vivo», ma il suo lettore può avere ancora l'ultimo output da
    /// consegnare. Default `false`: chi non ha sorgenti non costa niente.
    fn needs_wakeups(&self) -> bool {
        false
    }

    /// Il pannello/dock del tool si è aperto (`true`) o chiuso (`false`).
    ///
    /// Contratto del modello di attivazione:
    /// - le **risorse dell'app** (processi, socket, thread, timer) le possiede
    ///   l'app e le rilascia smettendo di produrre la subscription che le
    ///   tiene in vita: un tool non le tocca mai da qui;
    /// - le **cache del tool** (risultati, righe, storia) si rilasciano qui;
    ///   se serve fermare o ricaricare qualcosa dell'app si emette una
    ///   richiesta (`CancelSearch` alla chiusura di Cerca, `NotesUnload` alla
    ///   chiusura di Appunti e `NotesLoad` alla riapertura); il lavoro saltato a vista chiusa, come il
    ///   ricaricamento del log Git, lo recupera l'app stessa alla riapertura;
    /// - eccezione dichiarata: il terminale possiede le PTY e le tiene vive a
    ///   dock chiuso, qui riduce solo lo scrollback;
    /// - distinzione da non perdere: una risorsa dell'app nasce **dentro lo
    ///   stream** di una `Subscription::run_with` e muore quando l'app smette
    ///   di produrla (anche il timer d'inattività di `IdleTable` è fatto
    ///   così), mentre una cache del tool appartiene alla vista e si libera
    ///   qui: le due cose non si sostituiscono a vicenda.
    ///
    /// `update` non viene chiamato alla chiusura: questo è l'unico punto in
    /// cui un tool sa di non essere più visto.
    fn on_visibility_changed(&mut self, _open: bool) -> Vec<ToolRequest> {
        Vec::new()
    }
}

/// Registro dei tool + disposizione icone sulle due barre + stato aperto/chiuso.
pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
    pub left: Vec<ToolId>,
    pub right: Vec<ToolId>,
    pub active_panel: Option<ToolId>,
    /// Tool Dock attualmente mostrato (uno solo, come `bottomTab` nel prototipo).
    pub active_dock: Option<ToolId>,
    /// Altezza corrente del dock (condivisa da tutti i tool Dock).
    pub dock_height: f32,
    /// Funzionalità spente dall'utente. Vuoto per default, quindi il
    /// comportamento di partenza è identico a prima di questo campo.
    disabled_features: BTreeSet<&'static str>,
    /// Tool nascosti per una ragione ambientale, non per scelta dell'utente.
    unavailable: HashSet<ToolId>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
            active_panel: None,
            active_dock: None,
            dock_height: DOCK_HEIGHT_RANGE.1,
            disabled_features: BTreeSet::new(),
            unavailable: HashSet::new(),
        }
    }

    /// Il tool è utilizzabile, cioè la sua funzionalità non è spenta e non è
    /// nascosto per una ragione ambientale.
    pub fn is_available(&self, id: ToolId) -> bool {
        !self.unavailable.contains(&id)
            && self
                .get(id)
                .and_then(Tool::feature)
                .is_none_or(|feature| !self.disabled_features.contains(feature))
    }

    /// Le icone di una barra, **già filtrate** per disponibilità. Chi disegna
    /// una barra passa da qui e non da `self.left`/`self.right`: leggere i
    /// campi grezzi mostrerebbe i tool spenti.
    pub fn bar(&self, side: Side) -> Vec<ToolId> {
        let bar = match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        };
        bar.iter()
            .copied()
            .filter(|&id| self.is_available(id))
            .collect()
    }

    /// Accende o spegne una funzionalità. Spegnendola chiude pannello e dock
    /// che le appartengono **passando dai setter**, non assegnando i campi:
    /// è l'unico modo di far scattare `on_visibility_changed` e liberare
    /// davvero le risorse (vedi il commento a `set_active_panel`).
    pub fn set_feature_enabled(
        &mut self,
        feature: &'static str,
        enabled: bool,
    ) -> Vec<(ToolId, ToolRequest)> {
        let changed = if enabled {
            self.disabled_features.remove(feature)
        } else {
            self.disabled_features.insert(feature)
        };
        if !changed || enabled {
            return Vec::new();
        }
        self.close_unavailable()
    }

    /// Nasconde un tool per una ragione ambientale. Se è aperto, lo chiude
    /// passando dai setter così riceve la notifica di visibilità.
    pub fn set_unavailable(&mut self, id: ToolId, unavailable: bool) -> Vec<(ToolId, ToolRequest)> {
        let changed = if unavailable {
            self.unavailable.insert(id)
        } else {
            self.unavailable.remove(&id)
        };
        if !changed || !unavailable {
            return Vec::new();
        }
        self.close_unavailable()
    }

    fn close_unavailable(&mut self) -> Vec<(ToolId, ToolRequest)> {
        let mut requests = Vec::new();
        if self.active_panel.is_some_and(|id| !self.is_available(id)) {
            requests.extend(self.set_active_panel(None));
        }
        if self.active_dock.is_some_and(|id| !self.is_available(id)) {
            requests.extend(self.set_active_dock(None));
        }
        requests
    }

    /// Le funzionalità spente. Serve alla persistenza e ai menù, che devono
    /// poter chiedere «questa chiave è spenta?» a ogni frame senza allocare.
    pub fn disabled_features(&self) -> &BTreeSet<&'static str> {
        &self.disabled_features
    }

    pub fn register(&mut self, tool: Box<dyn Tool>, side: Side) {
        let id = tool.id();
        match side {
            Side::Left => self.left.push(id),
            Side::Right => self.right.push(id),
        }
        self.tools.push(tool);
    }

    pub fn get(&self, id: ToolId) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.id() == id).map(|t| t.as_ref())
    }

    pub fn get_mut(&mut self, id: ToolId) -> Option<&mut Box<dyn Tool>> {
        self.tools.iter_mut().find(|t| t.id() == id)
    }

    pub fn kind_of(&self, id: ToolId) -> Option<ToolKind> {
        self.get(id).map(|t| t.kind())
    }

    /// Lato della barra che contiene l'icona (determina il lato del pannello).
    pub fn side_of(&self, id: ToolId) -> Side {
        if self.left.contains(&id) {
            Side::Left
        } else {
            Side::Right
        }
    }

    /// Icone di una barra filtrate per tipo, nell'ordine della barra
    /// (gruppo alto = SidePanel, gruppo basso = Dock).
    pub fn bar_of_kind(&self, side: Side, kind: ToolKind) -> Vec<ToolId> {
        let bar = match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        };
        bar.iter()
            .copied()
            .filter(|&id| self.kind_of(id) == Some(kind) && self.is_available(id))
            .collect()
    }

    pub fn is_open(&self, id: ToolId) -> bool {
        match self.kind_of(id) {
            Some(ToolKind::SidePanel) => self.active_panel == Some(id),
            Some(ToolKind::Dock) => self.active_dock == Some(id),
            None => false,
        }
    }

    /// Notifica un cambio di visibilità al tool e ne raccoglie le richieste,
    /// etichettate con l'id di chi le ha emesse.
    pub fn notify_visibility(&mut self, id: ToolId, open: bool) -> Vec<(ToolId, ToolRequest)> {
        self.get_mut(id)
            .map(|tool| tool.on_visibility_changed(open))
            .unwrap_or_default()
            .into_iter()
            .map(|request| (id, request))
            .collect()
    }

    /// Cambia il pannello laterale attivo notificando prima chi si chiude e
    /// poi chi si apre. Passaggio obbligato: assegnare `active_panel` a mano
    /// salterebbe `on_visibility_changed` e lascerebbe vive le risorse del
    /// tool chiuso.
    pub fn set_active_panel(&mut self, id: Option<ToolId>) -> Vec<(ToolId, ToolRequest)> {
        if id.is_some_and(|id| !self.is_available(id)) {
            return Vec::new();
        }
        let previous = self.active_panel;
        if previous == id {
            return Vec::new();
        }
        self.active_panel = id;
        let mut requests = Vec::new();
        if let Some(previous) = previous {
            requests.extend(self.notify_visibility(previous, false));
        }
        if let Some(id) = id {
            requests.extend(self.notify_visibility(id, true));
        }
        requests
    }

    /// Come `set_active_panel` per il dock. Copre anche lo **swap implicito**:
    /// aprire un tool Dock ne chiude un altro senza che nessuno emetta un
    /// messaggio di chiusura.
    pub fn set_active_dock(&mut self, id: Option<ToolId>) -> Vec<(ToolId, ToolRequest)> {
        if id.is_some_and(|id| !self.is_available(id)) {
            return Vec::new();
        }
        let previous = self.active_dock;
        if previous == id {
            return Vec::new();
        }
        self.active_dock = id;
        let mut requests = Vec::new();
        if let Some(previous) = previous {
            requests.extend(self.notify_visibility(previous, false));
        }
        if let Some(id) = id {
            requests.extend(self.notify_visibility(id, true));
        }
        requests
    }

    /// Clic sull'icona: toggle secondo il tipo di tool.
    /// Non serve una guardia sulla disponibilita': i due setter qui sotto
    /// sono il collo di bottiglia di ogni apertura, e la fanno gia' loro.
    pub fn click(&mut self, id: ToolId) -> Vec<(ToolId, ToolRequest)> {
        match self.kind_of(id) {
            Some(ToolKind::SidePanel) => {
                let next = (self.active_panel != Some(id)).then_some(id);
                self.set_active_panel(next)
            }
            Some(ToolKind::Dock) => {
                let next = (self.active_dock != Some(id)).then_some(id);
                self.set_active_dock(next)
            }
            None => Vec::new(),
        }
    }

    pub fn close(&mut self, id: ToolId) -> Vec<(ToolId, ToolRequest)> {
        let mut requests = Vec::new();
        if self.active_panel == Some(id) {
            requests.extend(self.set_active_panel(None));
        }
        if self.active_dock == Some(id) {
            requests.extend(self.set_active_dock(None));
        }
        requests
    }

    /// Altezza del dock entro i clamp del prototipo.
    pub fn set_dock_height(&mut self, height: f32) {
        let (min, _, max) = DOCK_HEIGHT_RANGE;
        self.dock_height = height.clamp(min, max);
    }

    /// Sposta `id` nella barra `to`, prima di `before` (None = in coda).
    /// Usato dal drag-and-drop delle icone.
    pub fn move_icon(&mut self, id: ToolId, to: Side, before: Option<ToolId>) {
        self.left.retain(|&i| i != id);
        self.right.retain(|&i| i != id);
        let bar = match to {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        };
        match before.and_then(|b| bar.iter().position(|&i| i == b)) {
            Some(idx) => bar.insert(idx, id),
            None => bar.push(id),
        }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    struct Fake(ToolId, ToolKind);
    impl Tool for Fake {
        fn id(&self) -> ToolId {
            self.0
        }
        fn title(&self) -> &'static str {
            "fake"
        }
        fn icon(&self) -> Icon {
            Icon::Project
        }
        fn kind(&self) -> ToolKind {
            self.1
        }
        fn view<'a>(&'a self, _ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
            iced::widget::Space::new().into()
        }
    }

    /// Registra ogni transizione di visibilità ricevuta. Il quarto campo è
    /// la funzionalità di appartenenza: `None` = nucleo.
    struct Spy(
        ToolId,
        ToolKind,
        std::rc::Rc<std::cell::RefCell<Vec<(&'static str, bool)>>>,
        Option<&'static str>,
    );
    impl Tool for Spy {
        fn id(&self) -> ToolId {
            self.0
        }
        fn title(&self) -> &'static str {
            "spy"
        }
        fn icon(&self) -> Icon {
            Icon::Project
        }
        fn kind(&self) -> ToolKind {
            self.1
        }
        fn view<'a>(&'a self, _ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
            iced::widget::Space::new().into()
        }
        fn feature(&self) -> Option<&'static str> {
            self.3
        }
        fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
            self.2.borrow_mut().push((self.0.0, open));
            Vec::new()
        }
    }

    /// Spegnere una funzionalità deve **liberare le risorse**, non solo
    /// nascondere l'icona: se si assegnasse `active_panel = None` a mano, il
    /// tool non riceverebbe mai la chiusura e terrebbe vivo quel che ha.
    #[test]
    fn spegnere_una_funzionalita_chiude_il_pannello_notificando_la_visibilita() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.register(
            Box::new(Spy(
                ToolId("db"),
                ToolKind::SidePanel,
                log.clone(),
                Some("database"),
            )),
            Side::Left,
        );
        r.click(ToolId("db"));
        assert_eq!(log.borrow().as_slice(), &[("db", true)]);

        r.set_feature_enabled("database", false);

        assert_eq!(
            log.borrow().as_slice(),
            &[("db", true), ("db", false)],
            "il tool deve ricevere la chiusura, o le sue risorse restano vive"
        );
        assert_eq!(r.active_panel, None);
    }

    /// Un tool spento sparisce dalle barre ma **resta registrato**: è quel che
    /// permette di riaccenderlo ritrovando l'icona al suo posto.
    #[test]
    fn un_tool_di_funzionalita_spenta_sparisce_dalle_barre_ma_non_dal_registro() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        for (id, feature) in [("project", None), ("db", Some("database"))] {
            r.register(
                Box::new(Spy(ToolId(id), ToolKind::SidePanel, log.clone(), feature)),
                Side::Left,
            );
        }

        r.set_feature_enabled("database", false);

        assert_eq!(r.bar(Side::Left), vec![ToolId("project")]);
        assert_eq!(
            r.bar_of_kind(Side::Left, ToolKind::SidePanel),
            vec![ToolId("project")]
        );
        assert!(
            r.get(ToolId("db")).is_some(),
            "resta registrato: spegnere non e' disinstallare"
        );
        assert!(!r.is_available(ToolId("db")));

        r.set_feature_enabled("database", true);
        assert_eq!(
            r.bar(Side::Left),
            vec![ToolId("project"), ToolId("db")],
            "riaccendendo, l'icona torna dov'era"
        );
    }

    /// L'ordine della barra è deciso dall'utente col trascinamento e
    /// persistito: un giro spegni/accendi non deve mescolarlo.
    #[test]
    fn riaccendere_una_funzionalita_rimette_l_icona_dove_stava() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        for (id, feature) in [("a", None), ("db", Some("database")), ("b", None)] {
            r.register(
                Box::new(Spy(ToolId(id), ToolKind::SidePanel, log.clone(), feature)),
                Side::Left,
            );
        }

        r.set_feature_enabled("database", false);
        r.set_feature_enabled("database", true);

        assert_eq!(
            r.bar(Side::Left),
            vec![ToolId("a"), ToolId("db"), ToolId("b")],
            "«db» deve tornare in mezzo, non in coda"
        );
    }

    /// Se un tool non disponibile potesse essere aperto per altre vie
    /// (scorciatoia, voce di menù, ripristino di sessione), comparirebbe un
    /// pannello che la barra non permette di richiudere.
    #[test]
    fn un_tool_non_disponibile_non_si_apre_per_nessuna_via() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.register(
            Box::new(Spy(
                ToolId("db"),
                ToolKind::SidePanel,
                log.clone(),
                Some("database"),
            )),
            Side::Left,
        );
        r.register(
            Box::new(Spy(
                ToolId("log"),
                ToolKind::Dock,
                log.clone(),
                Some("database"),
            )),
            Side::Left,
        );
        r.set_feature_enabled("database", false);

        assert!(r.click(ToolId("db")).is_empty());
        assert!(r.set_active_panel(Some(ToolId("db"))).is_empty());
        assert!(r.set_active_dock(Some(ToolId("log"))).is_empty());

        assert_eq!(r.active_panel, None);
        assert_eq!(r.active_dock, None);
        assert!(
            log.borrow().is_empty(),
            "nessuna apertura, quindi nessuna notifica: {:?}",
            log.borrow()
        );
    }

    #[test]
    fn lo_swap_implicito_del_dock_notifica_prima_la_chiusura_poi_l_apertura() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.register(
            Box::new(Spy(ToolId("a"), ToolKind::Dock, log.clone(), None)),
            Side::Left,
        );
        r.register(
            Box::new(Spy(ToolId("b"), ToolKind::Dock, log.clone(), None)),
            Side::Left,
        );
        r.click(ToolId("a"));
        // aprire «b» chiude «a» senza che nessuno emetta un messaggio: e' il
        // percorso che lasciava vive le risorse del tool sostituito
        r.click(ToolId("b"));
        r.close(ToolId("b"));
        assert_eq!(
            log.borrow().as_slice(),
            &[("a", true), ("a", false), ("b", true), ("b", false)]
        );
    }

    #[test]
    fn il_pannello_laterale_notifica_apertura_e_chiusura() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.register(
            Box::new(Spy(ToolId("p"), ToolKind::SidePanel, log.clone(), None)),
            Side::Left,
        );
        r.click(ToolId("p"));
        r.set_active_panel(None);
        // riassegnare lo stesso valore non e' una transizione
        r.set_active_panel(None);
        assert_eq!(log.borrow().as_slice(), &[("p", true), ("p", false)]);
    }

    #[test]
    fn dock_e_esclusivo_e_lo_stesso_click_lo_chiude() {
        let mut r = Registry::new();
        r.register(Box::new(Fake(ToolId("a"), ToolKind::Dock)), Side::Left);
        r.register(Box::new(Fake(ToolId("b"), ToolKind::Dock)), Side::Left);
        r.click(ToolId("a"));
        assert!(r.is_open(ToolId("a")));
        r.click(ToolId("b"));
        assert!(!r.is_open(ToolId("a")), "un solo dock alla volta");
        assert!(r.is_open(ToolId("b")));
        r.click(ToolId("b"));
        assert_eq!(r.active_dock, None);
    }

    #[test]
    fn altezza_dock_clampata_e_gruppi_per_tipo() {
        let mut r = Registry::new();
        r.register(Box::new(Fake(ToolId("p"), ToolKind::SidePanel)), Side::Left);
        r.register(Box::new(Fake(ToolId("t"), ToolKind::Dock)), Side::Left);
        r.register(Box::new(Fake(ToolId("q"), ToolKind::SidePanel)), Side::Left);
        r.set_dock_height(10.0);
        assert_eq!(r.dock_height, DOCK_HEIGHT_RANGE.0);
        r.set_dock_height(9999.0);
        assert_eq!(r.dock_height, DOCK_HEIGHT_RANGE.2);
        assert_eq!(
            r.bar_of_kind(Side::Left, ToolKind::SidePanel),
            vec![ToolId("p"), ToolId("q")]
        );
        assert_eq!(r.bar_of_kind(Side::Left, ToolKind::Dock), vec![ToolId("t")]);
    }

    #[test]
    fn redacted_non_stampa_il_contenuto_e_lo_restituisce_una_volta() {
        let segreto = Redacted::new("p4ss-segretissima".to_owned());
        assert_eq!(format!("{segreto:?}"), "«redatto»");
        assert_eq!(format!("{:?}", Some(segreto.clone())), "Some(«redatto»)");
        assert_eq!(segreto.as_str(), "p4ss-segretissima");
        assert!(!segreto.is_empty());
        assert_eq!(segreto.expose(), "p4ss-segretissima");
    }

    #[test]
    fn redacted_di_default_e_vuota() {
        let vuota = Redacted::default();
        assert!(vuota.is_empty());
        assert_eq!(vuota.as_str(), "");
    }

    #[test]
    fn la_copia_di_redacted_e_indipendente() {
        let originale = Redacted::new("uno".to_owned());
        let copia = originale.clone();
        assert_eq!(originale.expose(), "uno");
        assert_eq!(
            copia.as_str(),
            "uno",
            "consumare l'originale non tocca la copia"
        );
    }

    #[test]
    #[allow(unsafe_code)]
    fn l_azzeramento_di_redacted_copre_tutta_la_capacita() {
        use zeroize::Zeroize;

        // Vincolo a compile-time: senza `ZeroizeOnDrop` sul contenuto il
        // `Drop` di `Redacted` non azzererebbe niente e il resto del test
        // proverebbe solo `Vec::zeroize`, non il guscio.
        fn azzera_da_solo<T: zeroize::ZeroizeOnDrop>(_: &T) {}

        let mut testo = String::with_capacity(128);
        testo.push_str("segretissimo");
        let mut segreto = Redacted::new(testo);
        azzera_da_solo(&segreto.0);
        // SAFETY: vedi `Vec::zeroize`: dopo lo zeroize tutta la capacità è
        // scritta con zeri, quindi riportare `len` alla capacità espone solo
        // byte inizializzati e `u8` non ha requisiti di validità.
        let byte_vivi = unsafe {
            let vettore = segreto.0.as_mut_vec();
            let capacita = vettore.capacity();
            assert!(capacita >= 128);
            vettore.zeroize();
            vettore.set_len(capacita);
            vettore.iter().all(|byte| *byte == 0)
        };
        assert!(
            byte_vivi,
            "nessun byte della bozza deve restare nella capacità"
        );
        assert!(segreto.0.is_empty() || segreto.0.bytes().all(|byte| byte == 0));
        segreto.0.clear();
    }

    #[test]
    fn indisponibile_chiude_il_pannello_e_notifica_la_visibilita() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.register(
            Box::new(Spy(ToolId("jj"), ToolKind::SidePanel, log.clone(), None)),
            Side::Left,
        );
        r.click(ToolId("jj"));
        r.set_unavailable(ToolId("jj"), true);
        assert_eq!(r.active_panel, None);
        assert_eq!(log.borrow().as_slice(), &[("jj", true), ("jj", false)]);
    }

    #[test]
    fn indisponibile_nasconde_senza_rimuovere_e_ripristina_la_posizione() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        for id in ["a", "jj", "b"] {
            r.register(
                Box::new(Spy(ToolId(id), ToolKind::SidePanel, log.clone(), None)),
                Side::Left,
            );
        }
        r.set_unavailable(ToolId("jj"), true);
        assert_eq!(r.bar(Side::Left), vec![ToolId("a"), ToolId("b")]);
        assert!(r.get(ToolId("jj")).is_some());
        assert!(r.click(ToolId("jj")).is_empty());
        assert!(r.set_active_panel(Some(ToolId("jj"))).is_empty());
        r.set_unavailable(ToolId("jj"), false);
        assert_eq!(
            r.bar(Side::Left),
            vec![ToolId("a"), ToolId("jj"), ToolId("b")]
        );
        assert_eq!(r.active_panel, None);
    }

    #[test]
    fn indisponibilita_idempotente_non_disturba_altri_tool() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        for id in ["a", "jj"] {
            r.register(
                Box::new(Spy(ToolId(id), ToolKind::SidePanel, log.clone(), None)),
                Side::Left,
            );
        }
        r.click(ToolId("a"));
        log.borrow_mut().clear();
        r.set_unavailable(ToolId("jj"), true);
        r.set_unavailable(ToolId("jj"), true);
        r.set_unavailable(ToolId("jj"), false);
        r.set_unavailable(ToolId("jj"), false);
        assert_eq!(r.active_panel, Some(ToolId("a")));
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn indisponibilita_e_feature_spenta_sono_indipendenti() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.register(
            Box::new(Spy(ToolId("jj"), ToolKind::SidePanel, log, Some("jujutsu"))),
            Side::Left,
        );
        r.set_unavailable(ToolId("jj"), true);
        r.set_feature_enabled("jujutsu", false);
        r.set_feature_enabled("jujutsu", true);
        assert!(!r.is_available(ToolId("jj")));
        r.set_unavailable(ToolId("jj"), false);
        assert!(r.is_available(ToolId("jj")));
        r.set_feature_enabled("jujutsu", false);
        assert!(!r.is_available(ToolId("jj")));
    }

    #[test]
    fn indisponibilita_chiude_anche_il_dock() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut r = Registry::new();
        r.register(
            Box::new(Spy(ToolId("jj"), ToolKind::Dock, log.clone(), None)),
            Side::Left,
        );
        r.click(ToolId("jj"));
        r.set_unavailable(ToolId("jj"), true);
        assert_eq!(r.active_dock, None);
        assert_eq!(log.borrow().as_slice(), &[("jj", true), ("jj", false)]);
    }

    #[test]
    fn jj_action_ha_un_nome_per_le_notifiche() {
        assert_eq!(JjAction::Describe("x".to_owned()).label(), "Descrizione");
        assert_eq!(JjAction::New(None).label(), "Nuovo cambiamento");
        assert_eq!(JjAction::Push.label(), "Push");
    }

    #[test]
    fn le_richieste_jj_si_stampano_e_le_azioni_si_confrontano() {
        assert_eq!(
            format!("{:?}", ToolRequest::JjAction(JjAction::Push)),
            "JjAction(Push)"
        );
        assert_eq!(format!("{:?}", ToolRequest::JjLoad), "JjLoad");
        assert_eq!(format!("{:?}", ToolRequest::JjCancel), "JjCancel");
        assert_eq!(
            JjAction::New(Some("a".to_owned())),
            JjAction::New(Some("a".to_owned()))
        );
        assert_ne!(JjAction::New(None), JjAction::Push);
    }

    fn tutte_le_azioni_git() -> Vec<GitAction> {
        vec![
            GitAction::Fetch,
            GitAction::Pull,
            GitAction::Push,
            GitAction::MergeUpstream,
            GitAction::MergeContinue("Merge".to_owned()),
            GitAction::MergeAbort,
            GitAction::MergeBranch("feat".to_owned()),
            GitAction::SwitchBranch("feat".to_owned()),
            GitAction::SwitchRemoteBranch {
                remote: "origin".to_owned(),
                name: "feat".to_owned(),
            },
            GitAction::StashPush(None),
            GitAction::StashApply(0),
            GitAction::StashPop(0),
            GitAction::StashDrop(0),
        ]
    }

    #[test]
    fn solo_le_azioni_che_usano_la_cli_lo_dichiarano() {
        let con_cli: Vec<_> = tutte_le_azioni_git()
            .into_iter()
            .filter(GitAction::needs_git_cli)
            .collect();
        assert_eq!(
            con_cli,
            vec![
                GitAction::Push,
                GitAction::MergeUpstream,
                GitAction::MergeContinue("Merge".to_owned()),
                GitAction::MergeAbort,
                GitAction::MergeBranch("feat".to_owned()),
                GitAction::SwitchBranch("feat".to_owned()),
                GitAction::SwitchRemoteBranch {
                    remote: "origin".to_owned(),
                    name: "feat".to_owned(),
                },
                GitAction::StashPush(None),
                GitAction::StashApply(0),
                GitAction::StashPop(0),
            ]
        );
    }

    #[test]
    fn le_azioni_stash_sono_identificate_e_tutte_hanno_un_etichetta() {
        let azioni = tutte_le_azioni_git();
        let stash: Vec<_> = azioni
            .iter()
            .filter(|azione| azione.touches_stash())
            .cloned()
            .collect();
        assert_eq!(
            stash,
            vec![
                GitAction::StashPush(None),
                GitAction::StashApply(0),
                GitAction::StashPop(0),
                GitAction::StashDrop(0),
            ]
        );
        assert!(
            azioni
                .iter()
                .all(|azione| azione.label().ends_with('…') && azione.label().chars().count() > 3)
        );
    }
}
