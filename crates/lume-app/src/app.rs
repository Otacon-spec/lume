//! Stato applicativo, Message, update e view radice (stack a layer).

#[derive(Debug, Clone)]
struct SaveOrigin {
    epoch: u64,
    path: Option<PathBuf>,
    note: Option<(u64, lume_notes::NoteId)>,
}

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use iced::widget::{Space, button, container, hover, mouse_area, row, text};
use iced::{
    Alignment, Element, Event, Length, Padding, Point, Size, Subscription, Task, event, mouse,
};
use lume_core::{AppliedEdit, DocId, Range, Workspace, movement};
#[cfg(test)]
use lume_core::{ChangeSet, Transaction};
use lume_notes::{DeleteDisposition, LoadedNote, NoteId, NoteStore, NoteSummary};
use lume_store::{
    AppearancePreferences, EnvironmentProfile, SessionSnapshot, StoreError, StoreHandle,
};
use lume_syntax::{InputEditRaw, LanguageId};

/// Metadati UI: parser e alberi appartengono esclusivamente ai worker.
#[derive(Clone, Copy, Debug)]
pub struct SyntaxLanguage(LanguageId);

impl SyntaxLanguage {
    fn lang(&self) -> LanguageId {
        self.0
    }
}
use lume_theme::LumeTheme;
use lume_ui::editor::{EditorCommand, EditorOverlay, EditorView, Motion, PageDirection};
use lume_ui::styles;
use lume_ui::tool::{Side, ToolCtx, ToolEvent, ToolId, ToolKind, ToolRequest};

use crate::activity::{ActivityBar, ActivityBarIntent, ActivityBarMessage, IconDrag};
use crate::appearance::IconThemeState;
use crate::dock::{Dock, DockMessage};
use crate::editor_tab_bar::{EditorTabBar, EditorTabBarIntent, EditorTabBarMessage};
use crate::environment_picker::{
    EnvironmentPicker, EnvironmentPickerIntent, EnvironmentPickerMessage,
};
use crate::environments;
use crate::layout::{HandleKind, LayoutState};
use crate::menu::{MenuAct, MenuBar, MenuBarMessage, MenuChecks, MenuIntent};
use crate::metrics::*;
use crate::overlay_manager::{Ingresso, Modale, OverlayId};
use crate::search::{GlobalSearchIntent, GlobalSearchMessage, GlobalSearchOverlay, SearchTab};
use crate::settings_panel::{SettingsIntent, SettingsMessage, SettingsPanel};
#[cfg(feature = "sql")]
use crate::sql_mode;
use crate::statusbar::{Statusbar, StatusbarIntent, StatusbarMessage};
use crate::titlebar::{Titlebar, TitlebarInfo, TitlebarIntent, TitlebarMessage};
use crate::{editor_empty, jj_actions, search, tools};
pub(crate) use riquadri::AppShell;

pub struct ThemeState {
    pub list: Vec<LumeTheme>,
    pub sel: usize,
    pub iced: iced::Theme,
}

/// Tema di riserva se nessun TOML embedded si carica: l'IDE parte comunque.
static THEME_FALLBACK: std::sync::LazyLock<LumeTheme> =
    std::sync::LazyLock::new(LumeTheme::default);

impl ThemeState {
    fn new() -> Self {
        let list = LumeTheme::builtin();
        let iced = list
            .first()
            .map(LumeTheme::iced_theme)
            .unwrap_or_else(|| THEME_FALLBACK.iced_theme());
        Self { list, sel: 0, iced }
    }

    pub fn current(&self) -> &LumeTheme {
        self.list
            .get(self.sel)
            .or_else(|| self.list.first())
            .unwrap_or(&THEME_FALLBACK)
    }
}

/// Notifica in status bar. Di regola sparisce dopo 5 s; quelle persistenti
/// restano finché l'utente non le chiude.
pub struct Notice {
    pub text: String,
    pub kind: NoticeKind,
    /// Resta finché l'utente non preme «✕»: per «Informazioni su Lume», che
    /// chi apre una segnalazione deve poter ricopiare con calma (#11).
    pub persistente: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Error,
    Info,
}

/// Il worker tiene una copia del flag, l'app ne possiede la durata.
/// Anche un cambio progetto o l'uscita chiedono l'annullamento al Drop.
struct FetchCancellation(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl FetchCancellation {
    fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for FetchCancellation {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct Lume {
    pub(crate) forge_config: Option<crate::forge::RunConfig>,
    pub theme_state: ThemeState,
    pub registry: lume_ui::Registry,
    pub layout: LayoutState,
    // ---- editor reale
    pub workspace: Workspace,
    editor_focus_epoch: u64,
    multi_occurrence: Option<OccurrenceTicket>,
    multi_occurrence_sequence: u64,
    pub syntax: HashMap<DocId, SyntaxLanguage>,
    syntax_pool: lume_syntax::SyntaxPool,
    syntax_documents: HashMap<lume_syntax::SyntaxLane, DocId>,
    syntax_expected:
        HashMap<lume_syntax::SyntaxLane, lume_ui::editor::syntax_delivery::SyntaxRequestGuard>,
    syntax_snapshots:
        HashMap<lume_syntax::SyntaxLane, (DocId, u64, lume_syntax::HighlightSnapshot)>,
    /// File ripristinati ma non ancora materializzati: nessuna lettura o
    /// apertura LSP finché la scheda resta inattiva.
    pending_restored_tabs: crate::session_restore::PendingRestoredTabs,
    /// Letture pending avviate per DocId. Il ticket rende innocue le risposte
    /// arrivate dopo close, rebind o una richiesta più recente.
    restored_tab_reads: HashMap<DocId, u64>,
    restored_tab_delivery: HashMap<DocId, crate::reader_jobs::DeliveryStatus>,
    /// Placeholder di note della cartella precedente: non vanno riaperti
    /// come Markdown ordinario dopo il cambio di libreria.
    restored_notes_revoked: HashMap<DocId, PathBuf>,
    /// Cartella precedente da verificare sul worker per i placeholder la cui
    /// appartenenza non è decidibile dal solo percorso (anche tramite alias).
    restored_notes_candidates: HashMap<DocId, Vec<PathBuf>>,
    readers: crate::reader_jobs::Readers,
    reader_collections: Vec<crate::reader_jobs::CollectionOwner>,
    restored_tab_commands: HashMap<DocId, VecDeque<EditorCommand>>,
    restored_tab_actions: HashMap<DocId, VecDeque<DeferredRestoredAction>>,
    restored_tab_request: u64,
    /// Le schede, divise in uno o due gruppi affiancati.
    ///
    /// Ha preso il posto di `tabs` + `active_doc` + `active_file`: i primi due
    /// perché ogni «apri qui» deve dire **in quale gruppo**, il terzo perché
    /// era ridondante — tutti i punti che scrivevano `active_doc` gli
    /// scrivevano accanto `active_file`, e derivarlo fa uscire gratis il caso
    /// «console SQL ⇒ nessun file attivo», che quel documento non ha path.
    pub groups: crate::editor_groups::EditorGroups,
    pub editor_tab_bar: EditorTabBar,
    pub project_root: Option<PathBuf>,
    #[cfg(feature = "sql")]
    idea_request: u64,
    #[cfg(feature = "sql")]
    idea_pending: Option<(PathBuf, crate::idea_import::Report)>,
    git_available: bool,
    editor_blame_available: bool,
    editor_blame_enabled: bool,
    editor_blame_cache: HashMap<DocId, Vec<Option<lume_ui::editor::BlameAuthorLine>>>,
    editor_blame_generation: HashMap<DocId, u64>,
    editor_blame_next_generation: u64,
    /// Directory del progetto effettivamente visibili nell'albero. Il watcher
    /// nativo usa questo insieme, unito ai parent dei documenti aperti.
    project_watch_directories: BTreeSet<PathBuf>,
    /// Generazione corrente della ricerca nei file (cancellazione).
    search_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Token dell'ultima richiesta di ricerca nei file (debounce 200 ms).
    search_debounce: u64,
    search_jobs: crate::search_events::Jobs,
    search_exit: Option<crate::search_events::Exit>,
    /// La transizione di uscita vieta nuove viste modali fino all'esito.
    uscita_in_corso: bool,
    /// Generazione della ricerca file nel Cerca ovunque. Separata dal grep
    /// del pannello Cerca per cancellare solo il worker pertinente.
    global_search_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    global_file_search_worker: Option<search::GlobalFileSearchWorker>,
    global_file_search_worker_error: Option<String>,
    /// Impronta dei byte su disco all'ultima lettura/scrittura accettata.
    disk_fingerprints: HashMap<DocId, [u8; 32]>,
    /// Documenti cambiati dal watcher durante un save: rileggerli al termine
    /// del worker, quando il risultato sul disco è stabile.
    pending_fs_rechecks: HashSet<DocId>,
    /// Un evento filesystem ha confermato che il disco diverge dal buffer.
    external_conflicts: HashSet<DocId>,
    /// Una sola scrittura in volo; la coda trattiene soltanto ID documento.
    saves: crate::save::Autenticazione,
    save_lease: Option<crate::scritture::Lease>,
    /// Intento del Job posseduto, distinto dal suo ruolo di autosave.
    save_causa: Option<(crate::save::Ticket, crate::scritture::Causa)>,
    save_active_origin: Option<(crate::scritture::Lease, SaveOrigin)>,
    save_job: Option<crate::save::Job>,
    /// Origine dei soli metadati accodati; nessun payload aggiuntivo.
    save_origins: HashMap<DocId, SaveOrigin>,
    save_job_root_epoch: Option<u64>,
    save_as_picker: Option<crate::save::Picker>,
    save_as_sequence: u64,
    save_as_flight: Option<crate::save::Rebind>,
    /// Lease UI fino al confine CommitStarted; non contiene file/preparati.
    save_as_permission: Option<crate::save::Ready>,
    replacement: Option<crate::replace_batch::State>,
    replace_versions: HashMap<DocId, u64>,
    replace_job: Option<crate::replace_batch::Job>,
    replace_permission: Option<crate::replace_batch::Ready>,
    replace_root_epoch: u64,
    replacement_sequence: u64,
    note_delete: Option<NoteDeletion>,
    note_delete_sequence: u64,
    /// Mutatore Git posseduto fino alla risposta, indipendente dalle
    /// generazioni dei pannelli/progetto. Condivide il confine writer.
    git_write: Option<crate::scritture::Lease>,
    git_operazione: Option<GitWriteLease>,
    git_seguito_legacy: bool,
    git_write_sequence: u64,
    git_root_epoch: u64,
    /// Decide quali mutazioni di file possono avvenire adesso (piano 14).
    scritture: crate::scritture::Coordinatore,
    /// Chiusura autorizzata che aspetta il worker; bool = salvare prima.
    close_after_save: Option<(u64, DaChiudere, bool)>,
    close_epoch: u64,
    pub environment_picker: EnvironmentPicker,
    /// Rilevazione delle distro WSL: parte alla prima apertura del selettore.
    pub wsl: environments::RilevazioneWsl,
    pub settings_panel: SettingsPanel,
    pub statusbar: Statusbar,
    pub menu_bar: MenuBar,
    pub global_search: GlobalSearchOverlay,
    pub activity_bar: ActivityBar,
    pub titlebar: Titlebar,
    pub shell: AppShell,
    pub dock: Dock,
    pub icon_themes: IconThemeState,
    store: Option<StoreHandle>,
    /// Assente finche Appunti non viene aperto: nessuna scansione, cache o
    /// risorsa specifica viene costruita all'avvio o quando la feature e spenta.
    notes: Option<NotesState>,
    /// Identità delle scansioni, conservata anche quando NotesState è distrutto.
    notes_scan_sequence: u64,
    /// Identità della libreria, indipendente dalle singole riscansioni.
    notes_library_sequence: u64,
    /// Cartella configurata degli Appunti, usata anche per riconoscere le
    /// schede nota durante il ripristino di sessione a pannello chiuso.
    notes_directory: PathBuf,
    /// Una scelta esplicita resta autorevole anche mentre il DB la salva e
    /// se lo stato della vista viene distrutto e ricreato.
    notes_directory_scelta: Option<PathBuf>,
    pub persistence_error: Option<String>,
    pub watcher_error: Option<(fs_bridge::WatchScope, String)>,
    watcher_scope: std::cell::RefCell<Option<fs_bridge::WatchScope>>,
    /// Notifica transiente in status bar (auto-clear dopo 5 s).
    pub notice: Option<(u64, Notice)>,
    /// Riferimento corrente del dock Log Git («HEAD» o un branch).
    git_log_reference: String,
    /// Segnalibri (persistiti nello snapshot); il tool ne mostra una copia.
    bookmarks: Vec<lume_store::BookmarkRecord>,
    /// Cronologia (più recente per prima; lo snapshot salva le prime 100).
    history: Vec<lume_store::HistoryRecord>,
    /// Messaggio dell'ultimo commit richiesto, per la voce di cronologia.
    pending_commit_message: Option<String>,
    /// Branch git del progetto (da `GitLoaded`); None senza repo.
    pub branch: Option<String>,
    notice_seq: u64,
    /// Log a rotazione degli errori mostrati; attivo solo nell'avvio reale.
    error_log: Option<crate::error_log::ErrorLog>,
    /// Generazione delle letture git (cancellazione), come `search_gen`:
    /// senza di essa due refresh sovrapposti atterrano fuori ordine e lo
    /// snapshot piu' vecchio sovrascrive il piu' recente.
    git_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    git_log_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    jj_workspace: Option<PathBuf>,
    jj_gen: u64,
    jj_cancel: jj_actions::CancelOnDrop,
    jj_loading: bool,
    jj_pending_load: Option<crate::jj_actions::load_admission::Pending>,
    jj_busy: bool,
    jj_pending_action: Option<crate::jj_actions::admission::Pending>,
    jj_action_generation: Option<u64>,
    jj_load_pending: bool,
    /// Worker JJ ammessi conservati dall'app: l'uscita li attende fuori UI
    /// anche quando Pending, producer e collector della subscription sono
    /// già scomparsi.
    jj_final: std::sync::Arc<crate::jj_actions::final_collection::FinalCollection>,
    /// Generazione della lettura del solo ramo (pill della titlebar).
    git_branch_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Fine dell'ultima lettura git avviata da noi. Sopprime l'eco del
    /// watcher su `.git/`: gix puo' riscrivere l'index durante `status()`
    /// (refresh stat racy-git) e le nostre stage/unstage/commit scrivono
    /// index e refs per definizione.
    last_git_operation: Option<std::time::Instant>,
    /// Un evento del filesystem e' arrivato mentre il pannello Git era
    /// chiuso: il ricalcolo si recupera alla riapertura invece di pagarlo
    /// per una vista che nessuno guarda.
    git_stale: bool,
    /// Come `git_stale` per il dock Log Git, che e' molto piu' caro
    /// (`branches()` + `log_from(reference, 200)`).
    git_log_stale: bool,
    /// Richieste emesse da `Tool::on_visibility_changed` in punti che non
    /// possono eseguirle sul momento (click sulla activity bar, ripristino
    /// della sessione). Svuotata da `drain_tool_requests`.
    pending_tool_requests: Vec<(ToolId, ToolRequest)>,
    // ---- modalità SQL
    /// Profili di connessione: leggeri, esistono anche da freddo, altrimenti
    /// il pannello Database sarebbe vuoto al primo avvio.
    #[cfg(feature = "sql")]
    sql_profiles: Vec<lume_sql::ConnectionProfile>,
    /// Nodi espansi dell'albero Database: chiavi `conn` e `conn:tabella`.
    /// Sta qui e non nel tool perché espandere non è solo stato di vista,
    /// richiede di interrogare lo schema.
    #[cfg(feature = "sql")]
    sql_expanded: BTreeSet<String>,
    /// Diff aperto nell'area centrale, con la richiesta che l'ha prodotto.
    /// Vive sull'app e non nel tool perche' e' l'area centrale a disegnarlo,
    /// e perche' due pannelli diversi (Commit e Log Git) lo aprono.
    diff: Option<Box<lume_git::FileDiff>>,
    /// Path e **base** del diff aperto: la chiave con cui `DiffLoaded` scarta
    /// le risposte vecchie. La base ci sta dentro perché due viste dello
    /// stesso file sono due richieste distinte.
    diff_request: Option<(PathBuf, lume_git::DiffBase)>,
    /// Tab `console.sql`: un documento **senza path**, così prende gratis
    /// barra tab, rope, undo ed evidenziazione SQL.
    #[cfg(feature = "sql")]
    sql_console: Option<DocId>,
    /// Dati di sessione conservati senza istanziare il backend SQL.
    #[cfg(not(feature = "sql"))]
    preserved_sql_console: Option<lume_store::SqlConsoleSnapshot>,
    /// Numeri stabili delle schede nuove: si rimuovono solo alla chiusura.
    senza_nome: HashMap<DocId, u32>,
    /// Ultimo numero assegnato: crescente, mai riusato, senza wrap.
    senza_nome_seq: u32,
    /// Risorse vive della modalità SQL. `None` = freddo: nessun socket,
    /// nessun thread, nessuna subscription.
    #[cfg(feature = "sql")]
    sql: Option<sql_mode::SqlWorkspace>,
    /// Lo stato **freddo** del language server: la sessione vive nell'attore,
    /// dentro la subscription. Qui c'è solo quel che l'app sa.
    pub lsp: crate::lsp_set::LspSet,
    /// Il server MCP, se acceso. `None` = nessuna pipe o subscription.
    #[cfg(feature = "mcp")]
    mcp: Option<crate::mcp_mode::McpWorkspace>,
    /// Sopravvive allo Stop per distinguere gli eventi delle vecchie pipe.
    #[cfg(feature = "mcp")]
    mcp_epoch: u64,
    /// La radice canonica per cui si sta chiedendo il consenso.
    #[cfg(feature = "mcp")]
    mcp_consenso: Option<PathBuf>,
    /// Segni diagnostici già convertiti e limitati al testo dei documenti.
    doc_marks: HashMap<DocId, Vec<lume_ui::editor::DiagnosticMark>>,
    completion: Option<crate::completion_ui::CompletionUi>,
    completion_wait: Option<crate::completion_ui::CompletionWait>,
    /// La definizione cambia documento; va ritirata se cambia il focus prima
    /// che il server risponda.
    definition_wait: Option<(String, u64, DocId)>,
    /// Anche il reader successivo all'Answer può ancora spostare il focus.
    /// None è terminale: esaurimento del contatore, mai wrap o riabilitazione.
    definition_epoch: Option<u64>,
    usages_wait: Option<(String, u64, DocId)>,
    hover_gen: u64,
    hover_wait: Option<crate::hover::HoverWait>,
    hover: Option<crate::hover::HoverShown>,
    signature_help_wait: Option<crate::signature_help::SignatureHelpWait>,
    signature_help: Option<crate::signature_help::SignatureHelpShown>,
    /// L'ultima ricerca di utilizzi. Vive qui e non nel tool per la solita
    /// ragione: il `Registry` dà solo `&dyn Tool`, e questa è la fotografia
    /// su cui si asserisce.
    pub(crate) usages: tools::usages::Snapshot,
    /// Il riquadro della rinomina, quando è aperto.
    pub(crate) rename: Option<crate::rename_box::RenameBox>,
    /// Dove è stato premuto il tasto destro nell'editor, quando il menù
    /// contestuale è aperto.
    pub(crate) context_menu: Option<crate::context_menu::ContextMenu>,
    /// Le ultime cinquanta posizioni visitate. Vive in memoria e muore con la
    /// finestra: «torna dov'ero» non ha senso il giorno dopo.
    pub(crate) navigazione: crate::navigazione::CronologiaNavigazione,
    /// Il testo scritto nel riquadro «Vai a riga», quando è aperto.
    pub(crate) goto_line: Option<String>,
    /// Il riquadro «Nuovo file»/«Nuova cartella» dell'albero, quando è aperto.
    pub(crate) albero: Option<albero::Richiesta>,
    /// Le azioni rapide offerte dal server per il documento corrente.
    pub(crate) code_actions: Vec<lsp_types::CodeActionOrCommand>,
    /// I blocchi collassati del documento, per punto d'inizio (riga).
    /// Stato di presentazione, posseduto dall'app per documento aperto:
    /// non si persiste tra i riavvii, e si libera chiudendo la scheda.
    pub(crate) folded: std::collections::HashMap<DocId, std::collections::HashSet<usize>>,
    /// Proiezione compatta dei blocchi collassati per documento attivo.
    pub(crate) fold_projections: std::collections::HashMap<DocId, lume_ui::editor::FoldProjection>,
    /// Versione del documento a cui si riferiscono i blocchi collassati: se
    /// il testo cambia, le righe non corrispondono più e i blocchi si
    /// espandono (lo spostamento dei blocchi con le modifiche non c'è ancora).
    pub(crate) fold_versione: std::collections::HashMap<DocId, u64>,
    /// La conferma di eliminazione di un file dell'albero, quando è aperta.
    pub(crate) albero_elimina: Option<albero_elimina::Richiesta>,
    /// Il riquadro di rinomina di un file dell'albero, quando è aperto.
    pub(crate) albero_rinomina: Option<albero_rinomina::Richiesta>,
    /// Il riquadro «Trova»/«Sostituisci», quando è aperto.
    pub(crate) find: Option<crate::find_replace::FindState>,
    /// La riga scelta nell'elenco dei recenti.
    pub(crate) recents_selected: usize,
    /// Quali funzionalità sono Dormienti (risorsa viva, nessuna vista in uso)
    /// e con che epoca: da qui nasce ogni timer d'inattività. Per il
    /// Database significa console **e** pannello chiusi entrambi.
    idle: crate::idle::IdleTable,
    /// Come si apre un motore. È un campo e non una costante perché i test
    /// vi sostituiscono un driver finto: senza, provare la macchina delle
    /// risposte richiederebbe un server vero.
    #[cfg(feature = "sql")]
    sql_factory: lume_sql::DriverFactory,
    /// Profilo che sta chiedendo la password. Il segreto **non** si salva:
    /// vive nel campo qui sotto finché non parte la connessione, poi si
    /// sposta nel thread del worker e muore con lui.
    #[cfg(feature = "sql")]
    sql_secret_for: Option<String>,
    /// La cassaforte del sistema. `Arc<dyn …>` e non un puntatore a funzione
    /// come `sql_factory` perché deve entrare in un `Task`, e perché i test
    /// vi sostituiscono un doppio in memoria.
    #[cfg(feature = "sql")]
    sql_vault: std::sync::Arc<dyn lume_sql::SecretStore>,
    /// Segreto appena digitato, tenuto **finché la connessione non riesce**.
    /// Salvarlo prima metterebbe in cassaforte anche i segreti buoni respinti
    /// da un host giù, e i cattivi accettati da nessuno.
    #[cfg(feature = "sql")]
    sql_pending_secret: Option<(String, lume_sql::Secret)>,
    /// Una cassaforte che non c'è si dice **una volta**: ripeterlo a ogni
    /// espansione sarebbe rumore su una condizione che non cambia.
    #[cfg(feature = "sql")]
    sql_vault_warned: bool,
    /// `Notify` condiviso col terminale: la subscription PTY si sveglia solo
    /// su output/uscita, non con un poll a 33 ms.
    terminal_wakeup: std::sync::Arc<tokio::sync::Notify>,
    /// Il **comando** in attesa di conferma, perché la funzionalità sta
    /// tenendo processi vivi. Uno solo per volta: la conferma è in linea.
    ///
    /// Il comando e non la sola funzionalità: i tre «Ferma» del chrome e del
    /// pannello mandano tutti `Stop(f)`, ma la conferma può essere stata
    /// aperta da un `SetEnabled(f, false)` — e fermare senza spegnere
    /// rimetterebbe su l'interruttore da solo. Vedi `features::confirm_replays`.
    feature_stop_pending: Option<crate::features::FeatureCommand>,
    /// La domanda «Salvare le modifiche?» in attesa di risposta. Vive solo
    /// finché il suo overlay è quello attivo: un altro overlay la sostituisce
    /// e l'Esc la annulla, come tutto il resto.
    chiusura: Option<ChiusuraInSospeso>,
    // Generazione per le letture RAM (ignora risposte stantie).
    ram_gen: u64,
    // Picco del solo processo Lume; None = non misurabile o non ancora letto.
    peak_memory_bytes: Option<u64>,
    // Generazione per la lista stash (lettura).
    git_stash_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    // Generazione per blame.
    blame_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    git_action_cancel: Option<FetchCancellation>,
    git_action_busy: bool,
    git_action_seq: u64,
}

struct NotesState {
    generation: u64,
    /// Generazione delle sole scansioni (cambia solo con la cartella): creare,
    /// aprire o eliminare una nota non deve far scartare una riscansione in volo.
    scan_generation: u64,
    /// Ultima richiesta di scansione della libreria corrente.
    scan_request: u64,
    directory: Option<PathBuf>,
    store: Option<NoteStore>,
    summaries: Vec<NoteSummary>,
    link_index: lume_notes::LinkIndex,
    marcatori: HashMap<DocId, Vec<std::ops::Range<usize>>>,
    loaded: HashMap<NoteId, LoadedNote>,
    tabs: HashMap<DocId, NoteId>,
    titles: HashMap<NoteId, String>,
    /// Stato di autosalvataggio per ogni scheda Appunti aperta.
    salvataggi: HashMap<DocId, crate::autosave::StatoSalvataggio>,
    link_saves: HashMap<DocId, String>,
    create_after_load: bool,
    /// Il minimo per salvare le schede ancora aperte a pannello chiuso.
    open_saves: HashMap<NoteId, OpenNoteSaveState>,
    /// Il pannello è chiuso e la cache della vista è stata scaricata: né
    /// riepiloghi né note caricate devono rientrare finché non si riapre.
    view_unloaded: bool,
}

/// Copia compatta di ciò che serve a `NoteStore::save` per una nota ancora
/// aperta in una scheda: intestazione, percorso, identità del file letto e
/// stato di sola lettura. **Senza** corpo markdown (vive nell'editor) e senza
/// `original_bytes`: sono le parti pesanti di `LoadedNote`.
///
/// L'identità è quella del file **come l'abbiamo letto**: è lei che fa
/// rifiutare a `NoteStore::save` una modifica esterna, quindi non va mai
/// rimpiazzata con quella di una rilettura fatta altrove.
struct OpenNoteSaveState {
    header: lume_notes::NoteHeader,
    path: PathBuf,
    identity: lume_notes::FileIdentity,
    read_only_error: Option<lume_notes::NoteError>,
}

impl OpenNoteSaveState {
    fn from_loaded(loaded: &LoadedNote) -> Self {
        Self {
            header: loaded.note.header.clone(),
            path: loaded.path().to_path_buf(),
            identity: loaded.identity.clone(),
            read_only_error: loaded.read_only_error.clone(),
        }
    }

    /// Ricostruisce la nota da salvare: il corpo arriva dall'editor.
    fn to_loaded(&self, markdown: String) -> LoadedNote {
        LoadedNote {
            note: lume_notes::Note {
                header: self.header.clone(),
                markdown,
                path: Some(self.path.clone()),
            },
            identity: self.identity.clone(),
            original_bytes: Vec::new(),
            read_only_error: self.read_only_error.clone(),
        }
    }
}

impl NotesState {
    fn loading(scan_generation: u64) -> Self {
        Self {
            generation: 1,
            scan_generation,
            scan_request: scan_generation,
            directory: None,
            store: None,
            summaries: Vec::new(),
            link_index: lume_notes::LinkIndex::default(),
            marcatori: HashMap::new(),
            loaded: HashMap::new(),
            tabs: HashMap::new(),
            titles: HashMap::new(),
            salvataggi: HashMap::new(),
            link_saves: HashMap::new(),
            create_after_load: false,
            open_saves: HashMap::new(),
            view_unloaded: false,
        }
    }
}

fn default_notes_directory() -> PathBuf {
    lume_store::default_notes_directory().unwrap_or_else(|_| PathBuf::from("notes"))
}

async fn notes_scan_blocking(directory: PathBuf) -> Result<(NoteStore, Vec<NoteSummary>), String> {
    tokio::task::spawn_blocking(move || {
        let store = NoteStore::open(directory).map_err(|error| error.to_string())?;
        let summaries = store.scan().map_err(|error| error.to_string())?;
        Ok((store, summaries))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// La nota creata e, se c'è, l'avviso della cartella non sincronizzata.
type NotaCreata = (LoadedNote, Option<String>);

async fn notes_create_blocking(store: NoteStore) -> Result<NotaCreata, String> {
    tokio::task::spawn_blocking(move || {
        store
            .create_reporting("Senza titolo")
            .map(|(loaded, sync_error)| {
                let avviso = sync_error.map(|error| {
                    crate::note_links::avviso_creazione(&loaded.note.header.title, &error)
                });
                (loaded, avviso)
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn notes_create_linked_blocking(
    store: NoteStore,
    link: lume_notes::CodeLink,
) -> Result<NotaCreata, String> {
    tokio::task::spawn_blocking(move || crate::note_links::create_linked(&store, link))
        .await
        .map_err(|error| error.to_string())?
}

async fn notes_open_blocking(store: NoteStore, path: PathBuf) -> Result<LoadedNote, String> {
    tokio::task::spawn_blocking(move || store.load(path).map_err(|e| e.to_string()))
        .await
        .map_err(|error| error.to_string())?
}

struct NoteDeletion {
    request: u64,
    scope: u64,
    root_epoch: u64,
    id: NoteId,
    lease: crate::scritture::Lease,
    store: NoteStore,
    path: PathBuf,
    delete: fn(&NoteStore, &Path) -> Result<DeleteDisposition, String>,
    /// Fotografia dei binding e delle revisioni prima del preflight.
    documenti: Vec<NoteDeleteDocument>,
    /// La fase distruttiva è partita una sola volta; i documenti restano protetti.
    correlati: Option<Vec<DocId>>,
}

#[derive(Debug, Clone)]
struct NoteDeleteDocument {
    id: DocId,
    path: Option<PathBuf>,
    nota: Option<NoteId>,
    versione: u64,
    sporco: bool,
}

/// Identità e alias accertati sul worker, prima della cancellazione.
#[derive(Debug, Clone)]
pub struct NoteDeletePrepared {
    target: PathBuf,
    identity: lume_notes::FileIdentity,
    file_key: (u64, u64),
    alias: Vec<DocId>,
}

#[cfg(all(feature = "mcp", not(test)))]
type SharedRenamePrepared = std::sync::Arc<crate::rename::PreparedEdit>;
#[cfg(all(feature = "mcp", not(test)))]
type McpRenamePreparedResult = Result<(SharedRenamePrepared, Vec<(String, usize)>), String>;

#[cfg(feature = "mcp")]
async fn mcp_prepare_workspace_edit_async(
    root_path: PathBuf,
    edit: lsp_types::WorkspaceEdit,
    sources: std::collections::BTreeMap<String, crate::rename::RenameSource>,
) -> Result<(crate::rename::PreparedEdit, Vec<(String, usize)>), String> {
    tokio::task::spawn_blocking(move || {
        let root = lume_mcp::confine::Root::new(&root_path)
            .map_err(|_| "radice del progetto non leggibile".to_owned())?;
        let edits = crate::rename::rename_edits(&edit)
            .map_err(|_| "proposta di rinomina non valida; nessun file modificato".to_owned())?;
        for (uri, _) in &edits {
            let path = crate::lsp_uri::uri_to_path(uri)
                .ok_or_else(|| "rinomina rifiutata: destinazione non è un file".to_owned())?;
            let relative = crate::mcp_mode::relative_to_root(&root_path, &path).ok_or_else(|| {
                "la rinomina tocca un file fuori dalla radice del progetto: rifiutata, nessun file modificato".to_owned()
            })?;
            let slash = lume_mcp::confine::to_slash(&relative);
            root.resolve_for_write(&slash).map_err(|error| {
                format!("rinomina rifiutata, nessun file modificato: {slash} — {error}")
            })?;
        }

        let prepared = crate::rename::prepare_workspace_edit_blocking(edit, sources).map_err(|error| {
            error
                .to_string()
                .replace(root_path.to_string_lossy().as_ref(), "[radice]")
        })?;
        let mut files = Vec::with_capacity(prepared.files.len());
        for file in &prepared.files {
            let relative = crate::mcp_mode::relative_to_root(&root_path, &file.path).ok_or_else(|| {
                "la rinomina tocca un file fuori dalla radice del progetto: rifiutata, nessun file modificato".to_owned()
            })?;
            let slash = lume_mcp::confine::to_slash(&relative);
            root.resolve_for_write(&slash).map_err(|error| {
                format!("rinomina rifiutata, nessun file modificato: {slash} — {error}")
            })?;
            files.push((slash, file.occurrences));
        }
        Ok((prepared, files))
    })
    .await
    .map_err(|error| format!("worker di rinomina MCP non disponibile: {error}"))?
}

fn delete_note_from_store(store: &NoteStore, path: &Path) -> Result<DeleteDisposition, String> {
    store.delete(path).map_err(|error| error.to_string())
}

fn note_delete_alias(
    file_key: (u64, u64),
    opened: &[NoteDeleteDocument],
) -> Result<Vec<DocId>, String> {
    let mut alias = Vec::new();
    for doc in opened {
        if let Some(path) = &doc.path {
            let key = crate::save::identity(path).map_err(|error| {
                format!(
                    "Identità dell'alias {} non verificabile: {error}",
                    path.display()
                )
            })?;
            if key == Some(file_key) {
                alias.push(doc.id);
            }
        }
    }
    Ok(alias)
}

async fn notes_delete_prepare_blocking(
    store: NoteStore,
    path: PathBuf,
    opened: Vec<NoteDeleteDocument>,
) -> Result<NoteDeletePrepared, String> {
    tokio::task::spawn_blocking(move || {
        let target = lume_core::canonical_path(&path).map_err(|e| e.to_string())?;
        let identity = store.load(&path).map_err(|e| e.to_string())?.identity;
        let file_key = crate::save::identity(&target)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Identità della nota non disponibile".to_owned())?;
        let alias = note_delete_alias(file_key, &opened)?;
        Ok(NoteDeletePrepared {
            target,
            identity,
            file_key,
            alias,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn notes_delete_blocking(
    store: NoteStore,
    path: PathBuf,
    opened: Vec<NoteDeleteDocument>,
    prepared: NoteDeletePrepared,
    delete: fn(&NoteStore, &Path) -> Result<DeleteDisposition, String>,
) -> Result<DeleteDisposition, String> {
    tokio::task::spawn_blocking(move || {
        let target = lume_core::canonical_path(&path).map_err(|e| e.to_string())?;
        let identity = store.load(&path).map_err(|e| e.to_string())?.identity;
        let file_key = crate::save::identity(&target).map_err(|e| e.to_string())?;
        if target != prepared.target
            || identity != prepared.identity
            || file_key != Some(prepared.file_key)
            || note_delete_alias(prepared.file_key, &opened)? != prepared.alias
        {
            return Err(
                "La nota o i suoi alias sono cambiati: eliminazione annullata, riprova".into(),
            );
        }
        delete(&store, &path)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Quiete di digitazione prima di far partire la ricerca nei file.
pub const SEARCH_DEBOUNCE_MS: u64 = 200;
const LSP_CHANGE_RETRY_MS: u64 = 50;
const LSP_CHANGE_RETRY_LIMIT: u8 = 20;
/// Attesa breve perché il puntatore di passaggio non chieda documentazione.
pub const HOVER_DELAY_MS: u64 = 400;

/// Nome della tab della console SQL, come nel prototipo.
#[cfg(feature = "sql")]
pub const SQL_CONSOLE_NAME: &str = "console.sql";

/// Traduce un profilo salvato in uno usabile. Restituisce `None` per un
/// record incoerente (motore sconosciuto, service principal senza tenant):
/// meglio un profilo in meno che uno che fallirà alla connessione senza
/// spiegare perché.
#[cfg(feature = "sql")]
fn sql_profile_from_record(
    record: &lume_store::SqlConnectionRecord,
) -> Option<lume_sql::ConnectionProfile> {
    let engine = match record.engine.as_str() {
        "azure-sql" => lume_sql::Engine::AzureSql,
        "oracle" => lume_sql::Engine::Oracle,
        _ => return None,
    };
    let auth = match record.auth.as_str() {
        "sql-login" => lume_sql::AuthKind::SqlLogin {
            user: record.username.clone()?,
        },
        "service-principal" => lume_sql::AuthKind::ServicePrincipal {
            tenant_id: record.tenant_id.clone()?,
            client_id: record.client_id.clone()?,
        },
        _ => return None,
    };
    Some(lume_sql::ConnectionProfile {
        id: record.id.clone(),
        label: record.label.clone(),
        engine,
        host: record.host.clone(),
        database: record.database.clone(),
        auth,
    })
}

/// Id stabile e leggibile ricavato dall'etichetta, con un suffisso se è già
/// preso. È anche la chiave del vault: deve restare uguale nel tempo, quindi
/// non si usa un contatore né un timestamp.
#[cfg(feature = "sql")]
fn sql_profile_id(label: &str, existing: &[lume_sql::ConnectionProfile]) -> String {
    let mut base = String::new();
    for character in label.trim().to_lowercase().chars() {
        if character.is_alphanumeric() {
            base.push(character);
        } else if !base.ends_with('-') {
            // un solo trattino per ogni gruppo di separatori: «azure · prod»
            // deve dare «azure-prod», non «azure---prod»
            base.push('-');
        }
    }
    let base = base.trim_matches('-').to_owned();
    let base = if base.is_empty() {
        "origine".to_owned()
    } else {
        base
    };
    let taken = |candidate: &str| existing.iter().any(|profile| profile.id == candidate);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or(base)
}

#[cfg(feature = "sql")]
fn sql_record_from_profile(
    profile: &lume_sql::ConnectionProfile,
) -> lume_store::SqlConnectionRecord {
    let (auth, username, tenant_id, client_id) = match &profile.auth {
        lume_sql::AuthKind::SqlLogin { user } => ("sql-login", Some(user.clone()), None, None),
        lume_sql::AuthKind::ServicePrincipal {
            tenant_id,
            client_id,
        } => (
            "service-principal",
            None,
            Some(tenant_id.clone()),
            Some(client_id.clone()),
        ),
    };
    lume_store::SqlConnectionRecord {
        id: profile.id.clone(),
        label: profile.label.clone(),
        engine: match profile.engine {
            lume_sql::Engine::AzureSql => "azure-sql".to_owned(),
            lume_sql::Engine::Oracle => "oracle".to_owned(),
        },
        host: profile.host.clone(),
        database: profile.database.clone(),
        auth: auth.to_owned(),
        username,
        tenant_id,
        client_id,
    }
}

/// Che fare della cassaforte quando una connessione ha risposto. Vedi
/// `Lume::sql_vault_action`.
#[cfg(feature = "sql")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VaultAction {
    /// La connessione è viva: il segreto in sospeso si può salvare.
    Remember,
    /// Le credenziali sono state respinte: quel che c'è in cassaforte non
    /// vale più, va tolto, e il campo torna a chiedere.
    Forget,
    /// Un fallimento di rete non dice niente sul segreto.
    Nothing,
}

/// Che fare del diff aperto quando cambia lo stato git. Vedi
/// `Lume::diff_to_refresh`.
#[derive(Clone, Debug, Eq, PartialEq)]
enum DiffRefresh {
    /// La riga c'è ancora: si ricalcola con la base **risolta**, che può non
    /// essere quella di prima se il lato che si guardava si è svuotato.
    Refresh(PathBuf, lume_git::DiffBase),
    /// La riga non c'è più: non c'è niente da guardare.
    Close,
    /// Non è un diff che invecchia, o non si sa: si lascia stare.
    Keep,
}

/// Cosa si sta per chiudere mentre il riquadro «Salvare le modifiche?» aspetta
/// una risposta.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChiusuraInSospeso {
    oggetto: DaChiudere,
    /// L'ultimo salvataggio fallito, mostrato **dentro** il riquadro: la
    /// notifica in barra di stato si spegne da sola dopo pochi secondi.
    errore: Option<String>,
    /// Numero dell'ultima notifica quando il riquadro si è aperto: un errore
    /// arrivato dopo non va coperto da una chiusura automatica.
    notifiche_dal: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaChiudere {
    Scheda(DocId),
    /// `None` quando la richiesta viene dalla X della barra del titolo, che
    /// non conosce l'id: la finestra è una sola e si risolve con `latest()`.
    Finestra(Option<iced::window::Id>),
}

/// Esito di una richiesta di chiusura della finestra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Uscita {
    /// Niente da perdere: si esce.
    Esci,
    /// C'è almeno un file sporco: si è aperto il riquadro e si aspetta.
    Chiedi,
}

/// Chi occupa l'area centrale, in ordine di precedenza come nel prototipo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CentralView {
    Diff,
    #[cfg(feature = "sql")]
    Sql,
    Editor,
}

#[derive(Debug, Clone)]
enum GitOperation {
    Refresh,
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    Discard(Vec<PathBuf>),
    Commit {
        message: String,
        amend: bool,
    },
    /// Mette in stage il singolo hunk di un file del worktree. L'hunk deve
    /// venire da `diff_worktree_file`: il suo `old_range` e' un offset nel
    /// blob dell'index, che e' il contenuto su cui viene applicato.
    StageHunk {
        path: PathBuf,
        hunk: lume_git::DiffHunk,
    },
    /// Toglie dallo stage il singolo hunk. Speculare: l'hunk deve venire da
    /// `diff_index_file`, dove il lato «nuovo» **e'** l'index.
    UnstageHunk {
        path: PathBuf,
        hunk: lume_git::DiffHunk,
    },
    // Blame, stash e fetch **non** stanno qui: hanno handler dedicati che
    // chiamano lume-git per conto loro, con la loro generazione e il loro
    // Message. Le varianti c'erano ma nessuno le costruiva, e il loro braccio
    // in `run_git` era irraggiungibile -- due strade dichiarate per una sola
    // percorsa. Se un giorno servisse riportarle qui, il criterio e' questo:
    // ci sta l'operazione che finisce con lo snapshot del pannello Commit.
}

#[derive(Clone, Copy)]
struct GitWriteLease {
    worktree: bool,
    ticket: u64,
    root_epoch: u64,
}

#[derive(Debug, Clone)]
pub enum GitWriteResult {
    Snapshot(Result<tools::git_native::Snapshot, String>, bool),
    Action(lume_ui::tool::GitAction, crate::git_actions::ActionReport),
}

/// Esito di una rilettura dal disco pronta per l'adozione nell'editor.
/// Per le schede Appunti porta anche la nota riletta dal `NoteStore`:
/// con lei seguono intestazione e identità della scheda.
#[derive(Debug, Clone)]
pub(crate) struct Ricaricato {
    pub testo: crate::file_read::Testo,
    pub nota: Option<lume_notes::LoadedNote>,
}

#[derive(Debug, Clone)]
pub enum Message {
    SyntaxViewport(syntax::ViewportRequest),
    Syntax(lume_ui::editor::syntax_subscription::SyntaxEvent),
    Forge(crate::forge_panel::ForgeMessage),
    Tool(ToolId, ToolEvent),
    Dock(DockMessage),
    ClosePanel,
    MenuBar(MenuBarMessage),
    CloseAll,
    CopyAbout,
    GlobalSearch(GlobalSearchMessage),
    Statusbar(StatusbarMessage),
    Settings(SettingsMessage),
    ActivityBar(ActivityBarMessage),
    Titlebar(TitlebarMessage),
    FolderPicked(Option<PathBuf>),
    #[cfg(feature = "sql")]
    IdeaPicked(u64, Option<PathBuf>),
    #[cfg(feature = "sql")]
    IdeaLoaded(u64, Result<(PathBuf, crate::idea_import::Report), String>),
    #[cfg(test)]
    SaveAsPicked(u64, Option<PathBuf>),
    #[cfg(test)]
    SaveDocFixture(DocId),
    #[cfg(test)]
    SaveAllFixture,
    SavePickerCollected(crate::save::PickerEvent),
    NotesDirectoryPicked(Option<PathBuf>),
    NotesPreferenceLoaded(u64, Result<Option<PathBuf>, String>),
    NotesScanned(u64, PathBuf, Result<(NoteStore, Vec<NoteSummary>), String>),
    /// Una nota nuova, con l'eventuale avviso della cartella non
    /// sincronizzata.
    NotesOpened(u64, Result<NotaCreata, String>),
    NotesOpenLinked(NoteId),
    NotesOpenCode {
        doc: DocId,
        index: usize,
    },
    NotesRemoveLink {
        doc: DocId,
        index: usize,
    },
    NotesReadFinished(u64, u64, Result<LoadedNote, String>),
    NotesDeletePrepared(
        u64,
        NoteId,
        u64,
        crate::scritture::Lease,
        Result<NoteDeletePrepared, String>,
    ),
    NotesDeleted(
        u64,
        NoteId,
        u64,
        crate::scritture::Lease,
        Result<DeleteDisposition, String>,
    ),
    NotesAutosaveDue(DocId, u64),
    NotesResolveConflict {
        doc: DocId,
        copia: bool,
    },
    NotesDirectorySaved(Result<(), String>),
    NewNote,
    EditorTabBar(EditorTabBarMessage),
    /// La sonda `--version` è tornata: due secondi, quindi mai in linea.
    LspResolved(
        String,
        u64,
        Result<lume_language_servers::ResolvedServer, String>,
    ),
    /// Il download dal catalogo è finito (consenso esplicito dal pannello).
    LspInstalled(String, u64, Result<(), String>),
    /// Progresso del download, associato al server e alla sua generazione.
    LspInstallProgress(String, u64, lume_language_servers::InstallProgress),
    /// Qualcosa dallo stream del language server, con la sua generazione.
    Lsp(String, u64, crate::lsp_mode::LspEvent),
    /// Evento dallo stream MCP e dalla sua specifica accensione.
    #[cfg(feature = "mcp")]
    Mcp(u64, lume_mcp::McpEvent),
    #[cfg(feature = "mcp")]
    ConsensoMcp(crate::consenso_mcp::ConsensoMessage),
    #[cfg(feature = "mcp")]
    McpCard(crate::mcp_card::CardMessage),
    #[cfg(feature = "mcp")]
    McpSetMode(lume_mcp::Mode),
    #[cfg(feature = "mcp")]
    McpModeLoaded(u64, Result<Option<String>, String>),
    #[cfg(feature = "mcp")]
    McpModeSaved(Result<(), String>),
    #[cfg(all(feature = "mcp", not(test)))]
    McpRenamePrepared {
        epoch: u64,
        request_id: u64,
        job_generation: u64,
        conn: u64,
        new_name: String,
        result: McpRenamePreparedResult,
    },
    /// Lettura asincrona richiesta da `open_in_editor`, con risposta rinviata.
    #[cfg(feature = "mcp")]
    McpOpenFinished {
        epoch: u64,
        request_id: u64,
        path: PathBuf,
        line: Option<usize>,
        result: Result<crate::file_read::Testo, String>,
    },
    /// Ricarica successiva a una scrittura MCP. Il testo deve arrivare nel
    /// buffer prima di notificare il language server.
    #[cfg(feature = "mcp")]
    McpDocReloaded(DocId, PathBuf, u64, Result<crate::file_read::Testo, String>),
    /// Il `didChange` differito: si manda solo se nel frattempo il documento
    /// non è cambiato ancora.
    LspChangeDebounced(DocId, u64),
    /// Riprova un `didChange` rifiutato perché la mailbox era satura.
    LspChangeRetry(DocId, u8),
    /// Scaduto il conto di un hover: gettone di generazione e indice del char.
    HoverDue(DocId, u64, usize),
    Rename(crate::rename_box::RenameMessage),
    #[cfg(not(test))]
    WorkspaceEditPrepared {
        root_epoch: u64,
        feedback: Option<String>,
        result: Result<std::sync::Arc<crate::rename::PreparedEdit>, crate::rename::RenameError>,
    },
    GotoLine(crate::prompt::PromptMessage),
    /// Il riquadro «Nuovo file»/«Nuova cartella» dell'albero.
    Albero(crate::prompt::PromptMessage),
    /// Comprime il blocco che contiene il cursore.
    #[allow(dead_code)]
    FoldToggle,
    /// Espande tutti i blocchi del documento attivo.
    #[allow(dead_code)]
    UnfoldAll,
    /// Una voce scelta nell'elenco delle azioni rapide.
    CodeActionChosen(usize),
    /// La conferma di eliminazione di un file dell'albero.
    AlberoElimina(crate::conferma_elimina::ConfermaMessage),
    /// Il riquadro di rinomina di un file dell'albero.
    AlberoRinomina(crate::prompt::PromptMessage),
    /// Il file dell'albero è stato spostato nel Cestino, o non è stato.
    ///
    /// La richiesta viaggia con il messaggio: se nel frattempo l'utente ha
    /// premuto Esc, l'esito non si perde lo stesso.
    AlberoEliminato(
        crate::app::albero_elimina::Richiesta,
        Result<lume_core::TrashOutcome, lume_core::TrashError>,
    ),
    OccurrenceSelected(
        OccurrenceTicket,
        Result<lume_core::SelectionSet, lume_core::occurrence::SelectionActionError>,
    ),
    RestoredTabAction(DocId, DeferredRestoredAction),
    Recents(crate::recents::RecentsMessage),
    /// Il riquadro «Trova»/«Sostituisci» (Ctrl+F / Ctrl+R).
    Find(crate::find_replace::FindMessage),
    /// Comando dell'editor **di un gruppo**: prima porta lì il fuoco.
    ///
    /// Porta il gruppo e non lo lascia implicito perché con due colonne
    /// «l'editor attivo» non è più una cosa sola, e battere in una colonna
    /// deve poterci spostare il fuoco senza un gesto apposta.
    EditorIn(crate::editor_groups::GroupId, EditorCommand),
    SaveActive,
    #[cfg(test)]
    SaveFinished(Box<crate::save::Completion>),
    SaveCollected(crate::save::Receipt),
    SaveReady(crate::save::Ready),
    ReplaceReady(crate::replace_batch::Ready),
    ReplaceCollected(crate::replace_batch::Receipt),
    /// Scorciatoia globale equivalente a una voce di menù.
    MenuAction(MenuAct),
    /// F11 / menù Naviga: segnalibro sulla riga corrente del documento attivo.
    ToggleBookmark,
    /// Ctrl+Maiusc+X: apre (o chiude) il pannello Estensioni.
    ShowExtensions,
    /// Risposta a `iced::clipboard::read()` per Incolla da menù: `None` se
    /// la clipboard è vuota o non testuale.
    ClipboardPasted(Option<String>),
    SearchUpdate(u64, crate::search_events::Delivery),
    SearchCollected(u64, Result<(), String>),
    SearchExitReady(u64, bool),
    /// Ricerca nei file a digitazione ferma (token della richiesta, query).
    /// Ricerca nei file a digitazione ferma: opzioni e ambito viaggiano
    /// interi, perché il pannello può averli cambiati fra una battuta e
    /// l'altra e il giro deve partire con quelli di adesso.
    SearchDebounced(u64, Box<lume_core::SearchOptions>, lume_core::SearchArea),
    NoticeExpired(u64),
    /// Risposta del controllo aggiornamenti (Aiuto -> Controlla
    /// aggiornamenti...). Sempre un `Result`: un controllo che fallisce
    /// in silenzio e' peggio di uno che non c'e'.
    UpdateChecked(Result<crate::updates::Esito, String>),
    /// La lettura di un file chiesto dall'utente è tornata dal thread di
    /// lavoro (il testo non finisce mai in un log: vedi `Testo`).
    FileReadFinished(u64, Option<(DocId, u64)>, crate::file_read::LetturaFile),
    RestoredTabReadFinished(DocId, PathBuf, u64, u64, u64, RestoredTabReadOutcome),
    #[cfg(test)]
    FileLetto(crate::file_read::LetturaFile),
    /// La lettura per il ricaricamento dopo una modifica esterna è tornata.
    /// Porta la versione del documento alla richiesta: una lettura più vecchia
    /// del documento non lo sovrascrive.
    DocRicaricato(DocId, PathBuf, u64, Result<Ricaricato, String>),
    GlobalFileSearch(u64, Result<Vec<search::Entry>, String>),
    GitLoaded(u64, Result<tools::git_native::Snapshot, String>, bool),
    GitWriteFinished(u64, Box<GitWriteResult>),
    GitLogLoaded(u64, Result<tools::gitlog::LogSnapshot, String>),
    JjLoaded(u64, Result<lume_jj::JjSnapshot, String>),
    JjLoadCollected(crate::jj_actions::load_admission::Collector),
    JjActionDone(u64, lume_ui::tool::JjAction, Result<String, String>),
    JjActionCollected(crate::jj_actions::admission::Collector),
    /// Solo il ramo corrente, letto a viste Git chiuse per la titlebar.
    GitBranchLoaded(u64, Option<String>),
    /// Il timeout d'inattività di una funzionalità è scaduto. Porta l'epoca:
    /// una riapertura avvenuta nel frattempo lo rende obsoleto.
    IdleElapsed(crate::features::Feature, u64),
    /// Risposta del worker di una connessione. `generation` è il token: chi
    /// la riceve la scarta se nel frattempo ne è partita un'altra.
    /// Esito della lettura in cassaforte per una connessione che si sta
    /// aprendo. `secret` assente = non c'era niente, si chiede.
    #[cfg(feature = "sql")]
    SqlSecretRecalled {
        connection: String,
        secret: Option<lume_ui::tool::Redacted>,
        error: Option<String>,
    },
    /// Esito di una scrittura o cancellazione in cassaforte. Non blocca
    /// niente — la connessione è già andata a buon fine o è già caduta — ma
    /// un fallimento va detto: spiega perché il segreto torna a chiedersi.
    #[cfg(feature = "sql")]
    SqlVaultWritten(Result<(), String>),
    /// Avvia, ferma o (dis)abilita un blocco di funzionalità.
    Feature(crate::features::FeatureCommand),
    /// «Ferma» su una funzionalità con processi vivi: conferma o annulla.
    FeatureStopConfirmed(crate::features::Feature),
    FeatureStopCancelled,
    #[cfg(feature = "sql")]
    SqlResponse {
        connection: String,
        /// Riquadro che ha chiesto la query; `None` per schema e tabelle,
        /// che non appartengono a un riquadro.
        pane: Option<usize>,
        generation: u64,
        result: Result<lume_sql::worker::Response, String>,
    },
    /// Un solo evento per stream in attesa di applicazione/ack UI.
    #[cfg(feature = "sql")]
    SqlQueryEvent {
        connection: String,
        pane: usize,
        generation: u64,
        delivery: crate::sql_events::Delivery,
    },
    #[cfg(feature = "sql")]
    Sql(crate::sql_console::SqlMessage),
    GitCommitFilesLoaded(String, Result<Vec<lume_git::ChangedFile>, String>),
    /// Diff calcolato fuori dal thread UI. `request` e' il `(path, base)`
    /// chiesto: se nel frattempo se n'e' aperto un altro — altro file **o**
    /// altra base sullo stesso file — questo si scarta.
    DiffLoaded {
        request: (PathBuf, lume_git::DiffBase),
        result: Result<Box<lume_git::FileDiff>, String>,
        /// `true` se e' un ricalcolo automatico e non un gesto dell'utente:
        /// cambia cosa fare quando non resta niente da mostrare.
        refresh: bool,
    },
    CloseDiff,
    /// Mette in stage il singolo hunk: prodotto dal pulsante «≡+» nel diff.
    StageHunk {
        path: PathBuf,
        hunk: lume_git::DiffHunk,
    },
    /// Toglie dallo stage il singolo hunk: pulsante «≡−», vista «In stage».
    UnstageHunk {
        path: PathBuf,
        hunk: lume_git::DiffHunk,
    },
    /// L'utente commuta fra «Locali» e «In stage» sul diff aperto.
    SwitchDiffBase(lume_git::DiffBase),
    FsChanged(Vec<PathBuf>),
    FsRescan(Vec<PathBuf>),
    FsWatchFailed(fs_bridge::WatchScope, String),
    /// Distro WSL rilevate dopo il primo frame.
    EnvironmentsDetected(Vec<EnvironmentProfile>),
    WindowResizeEdge(iced::window::Direction),
    DragStart(HandleKind),
    DragMoved(Point),
    DragEnd,
    WindowResized(Size),
    SessionSaved(Result<(), String>),
    // Blame
    GitBlameLoaded(u64, PathBuf, Result<Vec<lume_git::BlameLine>, String>, bool),
    EditorBlameLoaded(
        DocId,
        u64,
        PathBuf,
        Result<Vec<Option<lume_git::BlameLine>>, String>,
    ),
    // Stash
    GitStashLoaded(u64, Result<Vec<lume_git::StashEntry>, String>),
    // RAM
    PeakMemoryRead(Option<u64>),
    /// Risposta al riquadro «Salvare le modifiche?».
    ConfermaChiusura(crate::conferma_chiusura::ConfermaMessage),
    /// Il sistema chiede di chiudere la finestra (X di sistema, Alt+F4):
    /// con `exit_on_close_request: false` non chiude più da solo.
    WindowCloseRequested(iced::window::Id),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeferredRestoredAction {
    OpenGotoLine(crate::editor_groups::GroupId),
    GotoLineIndex(usize),
    GotoLspPosition(lsp_types::Position),
    OpenFind(bool),
    ToggleBookmark,
    CopySelection,
    CutSelection,
    SaveDocument,
    SaveAs,
    GotoDefinition,
    FindUsages,
    Rename,
    Completion,
    QuickDoc,
    SignatureHelp,
    SelectNextOccurrence,
    RiprendiApertura {
        path: PathBuf,
        dopo: crate::file_read::DopoApertura,
    },
    ReplaceAll {
        matches: Vec<lume_core::FileMatches>,
        replacement: String,
    },
}

struct RestoredTabPayload {
    text: crate::file_read::Testo,
    syntax: Option<SyntaxLanguage>,
    note: Option<(PathBuf, NoteStore, LoadedNote)>,
    notes_selection: Option<PathBuf>,
    note_matches_selection: bool,
}

/// `Message` è Clone+Debug; il payload contiene il Parser tree-sitter, che
/// non si clona né si stampa. Un solo handler consuma il risultato condiviso.
#[derive(Clone)]
pub(crate) struct RestoredTabPayloadMessage(
    std::sync::Arc<std::sync::Mutex<Option<RestoredTabPayload>>>,
);

impl std::fmt::Debug for RestoredTabPayloadMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RestoredTabPayloadMessage")
            .field(&"contenuto redatto")
            .finish()
    }
}

impl RestoredTabPayloadMessage {
    fn new(payload: RestoredTabPayload) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(payload))))
    }

    fn take(&self) -> Option<RestoredTabPayload> {
        self.0.lock().ok()?.take()
    }
}

/// Il guard resta vivo fino alla consegna all'handler o all'ultimo clone Drop.
#[derive(Clone, Debug)]
pub(crate) struct RestoredTabReadOutcome {
    result: Result<RestoredTabPayloadMessage, String>,
    delivery: Option<std::sync::Arc<crate::reader_jobs::Delivery>>,
    notes_context: Option<(u64, Option<PathBuf>)>,
}
impl RestoredTabReadOutcome {
    fn new(
        result: Result<RestoredTabPayloadMessage, String>,
        delivery: crate::reader_jobs::Delivery,
        notes_context: (u64, Option<PathBuf>),
    ) -> Self {
        Self {
            result,
            delivery: Some(std::sync::Arc::new(delivery)),
            notes_context: Some(notes_context),
        }
    }
    fn delivered(&self) {
        if let Some(delivery) = &self.delivery {
            delivery.delivered();
        }
    }
    #[cfg(test)]
    fn is_ok(&self) -> bool {
        self.result.is_ok()
    }
}
#[cfg(test)]
impl From<Result<RestoredTabPayloadMessage, String>> for RestoredTabReadOutcome {
    fn from(result: Result<RestoredTabPayloadMessage, String>) -> Self {
        Self {
            result,
            delivery: None,
            notes_context: None,
        }
    }
}

fn can_defer_restored_command(command: &EditorCommand) -> bool {
    matches!(
        command,
        EditorCommand::Insert(_)
            | EditorCommand::Paste(_)
            | EditorCommand::Backspace
            | EditorCommand::Delete
            | EditorCommand::Enter
            | EditorCommand::Tab
            | EditorCommand::Move(..)
            | EditorCommand::MovePage { .. }
            | EditorCommand::SetCursor { .. }
            | EditorCommand::ContextMenu { .. }
            | EditorCommand::Undo
            | EditorCommand::Redo
            | EditorCommand::SelectAll
    )
}

fn editor_command_text_bytes(command: &EditorCommand) -> usize {
    match command {
        EditorCommand::Insert(text) | EditorCommand::Paste(text) => text.len(),
        _ => 0,
    }
}

/// Il ramo corrente di `root`: `discover` + `branch_name`, niente status né
/// log. `None` fuori da un repository o con HEAD staccato.
fn leggi_ramo(root: &Path) -> Option<String> {
    lume_git::GitRepository::discover(root)
        .and_then(|repository| repository.branch_name())
        .ok()
        .flatten()
}

mod albero;
mod albero_elimina;
mod albero_rinomina;
mod editor;
mod editor_multi;
use editor_multi::{OccurrenceTicket, set_primary};
mod fs_bridge;
mod funzionalita;
mod git;
#[cfg(feature = "sql")]
mod idea;
mod ingresso;
mod lsp;
mod notes;
mod ricerca;
mod richieste_tool;
mod riquadri;
mod sessione;
mod sql;
mod syntax;
mod viste;
#[cfg(test)]
mod visual_regressions;

fn ignora_evento_globale_catturato(evento: &Event, status: event::Status) -> bool {
    if status != event::Status::Captured {
        return false;
    }

    let e_tastiera_o_metodo_input = matches!(evento, Event::Keyboard(_) | Event::InputMethod(_));
    let e_escape = matches!(
        evento,
        Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            ..
        })
    );

    e_tastiera_o_metodo_input && !e_escape
}

impl Lume {
    pub fn new() -> (Self, Task<Message>) {
        crate::startup::traccia("Lume::new() ingresso");
        let initial_file = crate::startup::file_argument(std::env::args_os());
        let loaded = futures::executor::block_on(async {
            let store = StoreHandle::open_default().await?;
            crate::startup::traccia("store aperto");
            let (snapshot, environments) = store.load_session().await?;
            crate::startup::traccia("sessione letta");
            // I record SQL restano nel profilo di sessione; soltanto il build
            // con feature `sql` carica e materializza i profili del driver.
            #[cfg(feature = "sql")]
            let sql_records = store.load_sql_connections().await?;
            #[cfg(not(feature = "sql"))]
            let sql_records: Vec<lume_store::SqlConnectionRecord> = Vec::new();
            let notes_directory = store.load_notes_directory().await.ok().flatten();
            Ok::<_, StoreError>((store, snapshot, environments, sql_records, notes_directory))
        });
        crate::startup::traccia("block_on concluso");
        let (mut app, task) = Self::new_without_store();
        let mut restore_task = Task::none();
        crate::startup::traccia("stato applicazione costruito");
        app.error_log = crate::error_log::ErrorLog::predefinito();
        match loaded {
            Ok((store, snapshot, environments, _sql_records, notes_directory)) => {
                app.store = Some(store);
                if let Some(directory) = notes_directory {
                    app.notes_directory = directory;
                }
                app.environment_picker
                    .set_profiles(environments::merge_profiles(environments, Vec::new()));
                #[cfg(feature = "sql")]
                {
                    app.sql_profiles = _sql_records
                        .iter()
                        .filter_map(sql_profile_from_record)
                        .collect();
                }
                restore_task = app.restore_snapshot_async(snapshot);
                crate::startup::traccia("sessione ripristinata (schede riaperte)");
                #[cfg(feature = "sql")]
                app.sync_db_tool();
            }
            Err(error) => {
                app.registra_errore(&format!("Persistenza non disponibile: {error}"));
                app.persistence_error = Some(error.to_string());
            }
        }
        let initial_file_task = initial_file
            .as_deref()
            .map(|path| app.open_startup_file(path))
            .unwrap_or_else(Task::none);
        app.notify_initial_visibility();
        let visibility_task = app.drain_tool_requests();
        let git_task = app.git_dopo_cambio_radice();
        // nessuna rilevazione WSL all'avvio: parte alla prima apertura del selettore
        crate::startup::traccia("Lume::new() uscita");
        (
            app,
            Task::batch([
                task,
                restore_task,
                initial_file_task,
                visibility_task,
                git_task,
            ]),
        )
    }

    /// Rileva le distro WSL fuori dal thread UI. Il processo parte quando il
    /// runtime esegue il `Task`, non quando lo si costruisce.
    fn detect_environments() -> Task<Message> {
        if !environments::WSL_SUPPORTED {
            return Task::done(Message::EnvironmentsDetected(Vec::new()));
        }
        Task::perform(
            async {
                tokio::task::spawn_blocking(environments::detect_wsl_distros)
                    .await
                    .unwrap_or_default()
            },
            Message::EnvironmentsDetected,
        )
    }

    /// Nome della cartella progetto (title bar); None senza progetto.
    pub fn project_name(&self) -> Option<&str> {
        self.project_root
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
    }

    pub fn titlebar_info(&self) -> TitlebarInfo<'_> {
        TitlebarInfo {
            project: self.project_name(),
            branch: self.branch.as_deref(),
            environment: self.environment_picker.label(),
        }
    }

    /// Allinea la shell dei nuovi terminali all'ambiente selezionato.
    fn sync_terminal_shell(&mut self) {
        let profile = self.environment_picker.selected_profile();
        let event = tools::terminal::Event::SetShell {
            shell: environments::shell_for(profile),
            label: profile.label.clone(),
        };
        if let Some(tool) = self.registry.get_mut(tools::terminal::ID) {
            tool.update(&ToolEvent::new(event));
        }
    }

    fn new_without_store() -> (Self, Task<Message>) {
        // Avvio neutro: nessun progetto o dato di dimostrazione implicito.
        let (registry, terminal_wakeup) = tools::default_registry();
        let mut registry = registry;
        // Le funzionalità senza backend cablato partono spente: i loro
        // pannelli non compaiono. Non è una preferenza dell'utente — vedi
        // `FeatureSpec::wired` — quindi non passa dalla persistenza.
        for spec in crate::features::FEATURES {
            if !spec.wired {
                let _ = registry.set_feature_enabled(spec.key, false);
            }
        }
        registry.active_panel = Some(tools::project::ID);
        // prototipo: il dock parte aperto sul terminale (backend reale)
        registry.active_dock = Some(tools::terminal::ID);
        let app = Self {
            forge_config: None,
            theme_state: ThemeState::new(),
            registry,
            layout: LayoutState::new(),
            workspace: Workspace::new(),
            syntax: HashMap::new(),
            syntax_pool: lume_syntax::SyntaxPool::default(),
            syntax_documents: HashMap::new(),
            syntax_expected: HashMap::new(),
            syntax_snapshots: HashMap::new(),
            pending_restored_tabs: crate::session_restore::PendingRestoredTabs::default(),
            restored_tab_reads: HashMap::new(),
            restored_tab_delivery: HashMap::new(),
            restored_notes_revoked: HashMap::new(),
            restored_notes_candidates: HashMap::new(),
            readers: crate::reader_jobs::Readers::default(),
            reader_collections: Vec::new(),
            restored_tab_commands: HashMap::new(),
            restored_tab_actions: HashMap::new(),
            restored_tab_request: 0,
            groups: crate::editor_groups::EditorGroups::new(),
            editor_tab_bar: EditorTabBar::default(),
            project_root: None,
            #[cfg(feature = "sql")]
            idea_request: 0,
            #[cfg(feature = "sql")]
            idea_pending: None,
            git_available: lume_git::GitCli::locate().is_some(),
            editor_blame_available: false,
            editor_blame_enabled: false,
            editor_blame_cache: HashMap::new(),
            editor_blame_generation: HashMap::new(),
            editor_blame_next_generation: 0,
            project_watch_directories: BTreeSet::new(),
            search_debounce: 0,
            search_jobs: crate::search_events::Jobs::default(),
            search_exit: None,
            uscita_in_corso: false,
            search_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            global_search_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            global_file_search_worker: None,
            global_file_search_worker_error: None,
            disk_fingerprints: HashMap::new(),
            pending_fs_rechecks: HashSet::new(),
            external_conflicts: HashSet::new(),
            environment_picker: EnvironmentPicker::default(),
            wsl: environments::RilevazioneWsl::default(),
            settings_panel: SettingsPanel::default(),
            statusbar: Statusbar,
            menu_bar: MenuBar::default(),
            global_search: GlobalSearchOverlay::default(),
            activity_bar: ActivityBar::default(),
            titlebar: Titlebar::default(),
            shell: AppShell::default(),
            dock: Dock,
            icon_themes: IconThemeState::default(),
            store: None,
            notes: None,
            notes_scan_sequence: 0,
            notes_library_sequence: 0,
            notes_directory: default_notes_directory(),
            notes_directory_scelta: None,
            persistence_error: None,
            watcher_error: None,
            watcher_scope: std::cell::RefCell::new(None),
            notice: None,
            notice_seq: 0,
            error_log: None,
            git_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            git_log_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            jj_workspace: None,
            jj_gen: 0,
            jj_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)).into(),
            jj_loading: false,
            jj_pending_load: None,
            jj_busy: false,
            jj_pending_action: None,
            jj_action_generation: None,
            jj_load_pending: false,
            jj_final: crate::jj_actions::final_collection::FinalCollection::new(),
            git_branch_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_git_operation: None,
            git_stale: false,
            git_log_stale: false,
            pending_tool_requests: Vec::new(),
            #[cfg(feature = "sql")]
            sql_profiles: Vec::new(),
            #[cfg(feature = "sql")]
            sql_expanded: BTreeSet::new(),
            diff: None,
            diff_request: None,
            #[cfg(feature = "sql")]
            sql_console: None,
            #[cfg(not(feature = "sql"))]
            preserved_sql_console: None,
            senza_nome: HashMap::new(),
            senza_nome_seq: 0,
            #[cfg(feature = "sql")]
            sql: None,
            lsp: crate::lsp_set::LspSet::default(),
            #[cfg(feature = "mcp")]
            mcp: None,
            #[cfg(feature = "mcp")]
            mcp_epoch: 0,
            #[cfg(feature = "mcp")]
            mcp_consenso: None,
            doc_marks: HashMap::new(),
            completion: None,
            editor_focus_epoch: 0,
            multi_occurrence: None,
            multi_occurrence_sequence: 0,
            completion_wait: None,
            definition_wait: None,
            definition_epoch: Some(0),
            usages_wait: None,
            hover_gen: 0,
            hover_wait: None,
            hover: None,
            signature_help_wait: None,
            signature_help: None,
            usages: tools::usages::Snapshot::default(),
            rename: None,
            context_menu: None,
            navigazione: crate::navigazione::CronologiaNavigazione::new(),
            goto_line: None,
            albero: None,
            code_actions: Vec::new(),
            folded: std::collections::HashMap::new(),
            fold_projections: std::collections::HashMap::new(),
            fold_versione: std::collections::HashMap::new(),
            albero_elimina: None,
            albero_rinomina: None,
            find: None,
            recents_selected: 0,
            idle: crate::idle::IdleTable::default(),
            #[cfg(feature = "sql")]
            sql_factory: lume_sql::open_driver,
            #[cfg(feature = "sql")]
            sql_secret_for: None,
            #[cfg(feature = "sql")]
            sql_vault: std::sync::Arc::new(lume_sql::OsVault::new()),
            #[cfg(feature = "sql")]
            sql_pending_secret: None,
            #[cfg(feature = "sql")]
            sql_vault_warned: false,
            terminal_wakeup,
            feature_stop_pending: None,
            chiusura: None,
            saves: crate::save::Autenticazione::default(),
            save_lease: None,
            save_causa: None,
            save_active_origin: None,
            save_job: None,
            save_origins: HashMap::new(),
            save_job_root_epoch: None,
            save_as_picker: None,
            save_as_sequence: 0,
            save_as_flight: None,
            save_as_permission: None,
            replacement: None,
            replace_versions: HashMap::new(),
            replace_job: None,
            replace_permission: None,
            replace_root_epoch: 0,
            replacement_sequence: 0,
            note_delete: None,
            note_delete_sequence: 0,
            git_write: None,
            git_operazione: None,
            git_seguito_legacy: false,
            git_write_sequence: 0,
            git_root_epoch: 0,
            scritture: crate::scritture::Coordinatore::default(),
            close_after_save: None,
            close_epoch: 0,
            git_log_reference: tools::gitlog::HEAD.to_string(),
            bookmarks: Vec::new(),
            history: Vec::new(),
            pending_commit_message: None,
            branch: None,
            ram_gen: 0,
            peak_memory_bytes: None,
            git_stash_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            blame_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            git_action_cancel: None,
            git_action_busy: false,
            git_action_seq: 0,
        };
        (app, Task::none())
    }

    /// Mostra una notifica in status bar; l'auto-clear arriva dopo 5 s
    /// (ignorato se nel frattempo è comparsa una notifica più recente).
    /// Un errore lascia traccia anche nel log a rotazione.
    fn notify(&mut self, kind: NoticeKind, text: String) -> Task<Message> {
        if matches!(kind, NoticeKind::Error) {
            self.registra_errore(&text);
        }
        self.notice_seq += 1;
        let id = self.notice_seq;
        self.notice = Some((
            id,
            Notice {
                text,
                kind,
                persistente: false,
            },
        ));
        Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                id
            },
            Message::NoticeExpired,
        )
    }

    /// Come `notify`, ma senza scadenza e senza timer: resta finché non la si
    /// chiude, o finché un'altra notifica non prende il suo posto. Un errore
    /// lascia traccia anche nel log a rotazione, come in `notify`.
    fn notify_persistente(&mut self, kind: NoticeKind, text: String) {
        if matches!(kind, NoticeKind::Error) {
            self.registra_errore(&text);
        }
        self.notice_seq += 1;
        self.notice = Some((
            self.notice_seq,
            Notice {
                text,
                kind,
                persistente: true,
            },
        ));
    }

    /// Mostra un errore: identico a `notify(NoticeKind::Error, ..)`, che ne
    /// scrive anche la traccia nel log a rotazione.
    fn notify_error(&mut self, text: String) -> Task<Message> {
        self.notify(NoticeKind::Error, text)
    }

    /// Registra l'errore mostrato, ignorando eventuali problemi del log.
    fn registra_errore(&mut self, testo: &str) {
        if let Some(log) = self.error_log.as_mut() {
            let _ = log.registra_ora(testo);
        }
    }

    /// Chiede a GitHub l'ultima release e lo dice in status bar.
    ///
    /// Parte **solo** da «Aiuto → Controlla aggiornamenti…»: nessun timer, per
    /// la regola «niente lavoro da inattivo» del budget. La chiamata e'
    /// sincrona e va quindi su `spawn_blocking`, come gix e il vault; il
    /// «Controllo aggiornamenti…» immediato serve perche' una rete lenta non
    /// sembri un click perso.
    fn check_updates(&mut self) -> Task<Message> {
        let attesa = self.notify(NoticeKind::Info, "Controllo aggiornamenti…".to_owned());
        let richiesta = Task::perform(
            async {
                tokio::task::spawn_blocking(crate::updates::controlla)
                    .await
                    .unwrap_or_else(|errore| Err(errore.to_string()))
            },
            Message::UpdateChecked,
        );
        Task::batch([attesa, richiesta])
    }

    /// Token del tema corrente (per le view).
    pub fn theme(&self) -> &LumeTheme {
        self.theme_state.current()
    }

    /// Il documento attivo del gruppo col fuoco: quello su cui agiscono
    /// Ctrl+S, i segnalibri e la barra di stato.
    pub fn active_doc(&self) -> Option<DocId> {
        self.groups.active()
    }

    /// Il path del documento attivo. **Derivato**, non memorizzato: un
    /// documento senza path — file nuovo o console SQL — non ha file attivo; prima
    /// quel caso andava ricordato a mano in cinque punti diversi.
    pub fn active_path(&self) -> Option<&Path> {
        self.active_doc()
            .and_then(|id| self.workspace.get(id))
            .and_then(lume_core::Document::path)
    }

    /// Ogni messaggio passa di qui: dopo l'aggiornamento si rimette in
    /// coerenza il folding (vedi [`Self::coerenza_folding`]). Senza blocchi
    /// compressi il controllo non fa nulla.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.update_messaggio(message);
        let avviso = self.coerenza_folding();
        let seguito = self.applica_seguito();
        Task::batch([task, avviso, seguito])
    }

    /// L'unico punto in cui il seguito del coordinatore delle scritture
    /// diventa `Task` e avvisi (piano 14). Alla quiete ricontrolla la chiusura
    /// in sospeso, che può finalmente procedere.
    fn applica_seguito(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        if std::mem::take(&mut self.git_seguito_legacy) {
            self.scritture.riprendi_esterno();
        }
        loop {
            let seguito = self.scritture.seguito();
            if seguito.avvia.is_empty() && !seguito.quiete {
                break;
            }
            for (lease, op) in seguito.avvia {
                let rimandato = matches!(&op, crate::scritture::Op::Salva { doc, .. } if seguito.rimandati.contains(doc));
                tasks.push(self.avvia_save_autorizzato(lease, op, rimandato));
            }
            if seguito.quiete && self.scritture.guarda(None).in_volo.is_none() {
                let rechecks: Vec<_> = self.pending_fs_rechecks.iter().copied().collect();
                for id in rechecks {
                    tasks.push(self.reload_pending_fs_recheck(id));
                }
                tasks.push(self.finish_close_after_saves());
            }
        }
        Task::batch(tasks)
    }

    fn rifiuta_azioni_sulla_chiusura_durante_cestino(&mut self) -> bool {
        if self.albero_eliminazione_in_corso() && self.chiusura_attiva().is_some() {
            self.notify_persistente(
                NoticeKind::Info,
                "Chiusura in attesa del Cestino: annulla la chiusura per continuare".into(),
            );
            return true;
        }
        false
    }

    fn update_messaggio(&mut self, message: Message) -> Task<Message> {
        #[cfg(feature = "performance-diagnostics")]
        let _phase = lume_ui::performance_diagnostics::Span::new("app.update");
        // Serializza le mutazioni cooperative con il confine del commit, non
        // con l'I/O. Revoche e wake avvengono senza tenere lock in update.
        let save_pause = self.save_job.as_ref().and_then(crate::save::Job::pause);
        let replace_pause = self
            .replace_job
            .as_ref()
            .map(crate::replace_batch::Job::pause);
        self.reconcile_jj_cold_load();
        self.reconcile_jj_cold_action();
        let recovered = self.reconcile_jj_completed_action();
        let task = self.update_inner(message);
        self.mcp_sync_card();
        if let Some(pause) = save_pause {
            let valid = if self.save_as_flight.is_some() {
                self.save_as_model_current()
                    && self
                        .save_as_permission
                        .as_ref()
                        .is_none_or(|ready| self.save_ready_current(ready))
            } else {
                self.save_job_model_current()
                    && self
                        .save_as_permission
                        .as_ref()
                        .is_none_or(|ready| self.save_ready_current(ready))
            };
            pause.finish(valid);
        }
        if let Some(pause) = replace_pause {
            let valid = self.replacement_valid()
                && self
                    .replacement
                    .as_ref()
                    .is_some_and(|state| !state.cancel.load(std::sync::atomic::Ordering::Acquire));
            if !valid && let Some(job) = &self.replace_job {
                job.revoke();
            }
            pause.finish(valid);
        }
        Task::batch([recovered, task])
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        static PRIMO_UPDATE: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if crate::startup::prima_volta(&PRIMO_UPDATE) {
            crate::startup::traccia("primo update (il ciclo eventi consegna messaggi)");
        }
        let politica = message.politica();
        if let Err(rifiuto) = self.shell.ammetti(politica.ingresso) {
            return match rifiuto.avviso() {
                Some(testo) => self.notify(NoticeKind::Info, testo),
                None => Task::none(),
            };
        }
        let mut tasks = Vec::new();
        if matches!(
            &message,
            Message::SaveActive
                | Message::MenuAction(MenuAct::Save | MenuAct::SaveAll)
                | Message::EditorIn(_, EditorCommand::Save)
                | Message::ConfermaChiusura(crate::conferma_chiusura::ConfermaMessage::Salva)
        ) && let Some(testo) = self.rifiuto_salvataggio()
        {
            return self.notify(NoticeKind::Info, testo);
        }
        match message {
            Message::Syntax(event) => return self.syntax_event(event),
            Message::SyntaxViewport(request) => return self.syntax_viewport(request),
            Message::Tool(id, ev) => {
                if politica.ingresso == Ingresso::Gesto
                    && self.rifiuta_azioni_sulla_chiusura_durante_cestino()
                {
                    return Task::none();
                }
                let requests = self
                    .registry
                    .get_mut(id)
                    .map(|tool| tool.update(&ev))
                    .unwrap_or_default();
                return self.apply_tool_requests(requests.into_iter().map(|r| (id, r)).collect());
            }
            Message::ClosePanel => {
                let requests = self.registry.set_active_panel(None);
                self.pending_tool_requests.extend(requests);
            }
            #[cfg(feature = "sql")]
            Message::IdeaPicked(request, path) => {
                if request == self.idea_request
                    && let Some(path) = path
                {
                    return self.read_idea(request, path);
                }
            }
            #[cfg(feature = "sql")]
            Message::IdeaLoaded(request, result) => return self.idea_loaded(request, result),
            Message::FolderPicked(root) => {
                if root.is_some() && self.albero_eliminazione_in_corso() {
                    return self.notify_error(
                        "Spostamento nel Cestino in corso: attendi prima di cambiare progetto"
                            .into(),
                    );
                }
                if self.save_as_picker.is_some() || self.save_as_flight.is_some() {
                    return self.notify_error(
                        "Salva con nome è in corso: attendi la raccolta prima di cambiare progetto"
                            .into(),
                    );
                }
                if let Some(root) = root {
                    self.set_project_root(root);
                    return Task::batch([
                        self.git_dopo_cambio_radice(),
                        self.persist_session(),
                        self.drain_tool_requests(),
                    ]);
                }
            }
            #[cfg(test)]
            Message::SaveAsPicked(ticket, path) => return self.save_as_picked(ticket, path),
            Message::SavePickerCollected(event) => return self.save_picker_collected(event),
            Message::NotesDirectoryPicked(directory) => {
                if directory.is_some() && self.albero_eliminazione_in_corso() {
                    return self.notify_error("Spostamento nel Cestino in corso: attendi prima di cambiare la cartella Appunti".into());
                }
                if let Some(directory) = directory {
                    return self.switch_notes_directory(directory);
                }
            }
            Message::NotesPreferenceLoaded(generation, result) => {
                if self.notes.as_ref().is_none_or(|notes| {
                    notes.scan_generation != generation || notes.scan_request != generation
                }) {
                    return Task::none();
                }
                match result {
                    Ok(directory) => {
                        let directory = directory.unwrap_or_else(default_notes_directory);
                        self.notes_directory = directory.clone();
                        return self.scan_notes_directory(generation, directory);
                    }
                    Err(error) => self.notes_scan_error(generation, error),
                }
            }
            Message::NotesScanned(generation, directory, result) => match result {
                Ok((store, summaries)) => {
                    if !self.finish_notes_scan(generation, directory, store, summaries) {
                        return Task::none();
                    }
                    if !self.registry.is_open(tools::notes::ID) {
                        // Caricamento fatto per un'azione a pannello chiuso
                        // (Nuovo appunto da scorciatoia): serve lo store, non
                        // la lista dei riepiloghi.
                        self.unload_notes_view_cache();
                    }
                    if self
                        .notes
                        .as_mut()
                        .is_some_and(|notes| std::mem::take(&mut notes.create_after_load))
                    {
                        return self.create_note();
                    }
                }
                Err(error) => self.notes_scan_error(generation, error),
            },
            Message::NotesOpened(generation, result) => {
                // L'avviso prima dell'esito: un errore successivo resta in vista.
                let (result, avviso) = match result {
                    Ok((loaded, avviso)) => (Ok(loaded), avviso),
                    Err(error) => (Err(error), None),
                };
                let avviso = avviso
                    .map(|avviso| self.notify(NoticeKind::Error, avviso))
                    .unwrap_or_else(Task::none);
                return Task::batch([avviso, self.finish_notes_opened(generation, result)]);
            }
            Message::NotesOpenLinked(id) => return self.open_note(id),
            Message::NotesOpenCode { doc, index } => return self.open_note_link(doc, index),
            Message::NotesRemoveLink { doc, index } => {
                return self.modifica_collegamenti(doc, move |nota| {
                    crate::note_links::rimuovi_collegamento(nota, index)
                });
            }
            Message::NotesReadFinished(generation, epoch, result) => {
                if epoch != self.scritture.guarda(None).epoca {
                    return self.notify_error(
                        "Lettura della nota superata da un'operazione sui file: ripeti l'apertura"
                            .into(),
                    );
                }
                return self.finish_notes_opened(generation, result);
            }
            Message::NotesDeletePrepared(request, id, scope, lease, result) => {
                return self.note_delete_prepared(request, id, scope, lease, result);
            }
            Message::NotesDeleted(request, id, scope, lease, result) => {
                return self.note_deleted(request, id, scope, lease, result);
            }
            Message::NotesAutosaveDue(doc, versione) => {
                return self.notes_autosave_due(doc, versione);
            }
            Message::NotesResolveConflict { doc, copia } => {
                if self.albero_eliminazione_in_corso() {
                    return self.notify_error(
                        "Spostamento nel Cestino in corso: attendi prima di risolvere il conflitto"
                            .into(),
                    );
                }
                return self.notes_resolve_conflict(doc, copia);
            }
            Message::NotesDirectorySaved(result) => {
                if let Err(error) = result {
                    self.persistence_error = Some(error);
                }
            }
            Message::NewNote => {
                if self
                    .notes
                    .as_ref()
                    .is_some_and(|notes| notes.store.is_some())
                {
                    return self.create_note();
                }
                let task = self.load_notes();
                if let Some(notes) = self.notes.as_mut() {
                    notes.create_after_load = true;
                }
                return task;
            }
            Message::ToggleBookmark => return self.toggle_bookmark(),
            Message::SearchDebounced(token, options, area) => {
                if token == self.search_debounce {
                    return self.run_search(*options, area);
                }
            }
            Message::SearchUpdate(generation, delivery) => {
                let accepted = delivery.is_current()
                    && delivery
                        .take()
                        .is_some_and(|update| self.deliver_search_update(generation, update));
                delivery.acknowledge(accepted);
            }
            Message::SearchCollected(ticket, result) => {
                let current = self
                    .search_jobs
                    .current()
                    .filter(|job| job.id() == ticket && job.is_current());
                if self.search_jobs.remove(ticket, &result)
                    && let (Some(job), Err(error)) = (current, result)
                {
                    self.deliver_search_update(
                        job.generation(),
                        lume_core::SearchUpdate::Error(error),
                    );
                }
            }
            Message::SearchExitReady(epoch, success) => {
                if self.search_exit.as_ref().is_some_and(|exit| {
                    exit.epoch == epoch
                        && exit.completion.result().is_some()
                        && exit.jobs.0.iter().all(crate::search_events::Job::collected)
                        && exit.jj_action.as_ref().is_none_or(|job| job.collected())
                        && exit.jj_load.as_ref().is_none_or(|job| job.collected())
                        && exit
                            .syntax
                            .as_ref()
                            .is_none_or(|join| join.peek().is_some())
                }) {
                    let Some(exit) = self.search_exit.take() else {
                        return Task::none();
                    };
                    let ids: Vec<_> = self.groups.all().collect();
                    let expected: Vec<_> = exit.documents.iter().map(|(id, _)| *id).collect();
                    if !success
                        || exit.completion.result() != Some(true)
                        || !exit.jobs.0.iter().all(crate::search_events::Job::succeeded)
                        || self.shell.active_overlay() != exit.overlay
                        || self.close_epoch != epoch
                        || self.scritture.guarda(None).epoca != exit.file_epoch
                        || (self.scritture.guarda(None).in_volo.is_some()
                            || self.git_operazione.is_some())
                        || self.project_root != exit.root
                        || ids != expected
                        || exit.documents.iter().any(|(id, snapshot)| {
                            !self
                                .workspace
                                .get(*id)
                                .is_some_and(|doc| doc.matches_snapshot(snapshot))
                        })
                    {
                        self.uscita_in_corso = false;
                        return self.notify_error("Chiusura non completata: stato cambiato o raccolta Cerca/Jujutsu fallita. Controlla e ripeti l'uscita".into());
                    }
                    return self.close_window_task(exit.window);
                }
                // Una receipt duplicata o superata non deve drenare richieste
                // rimaste in coda e avviare nuovo I/O dopo la chiusura.
                return Task::none();
            }
            Message::UpdateChecked(esito) => {
                return match esito {
                    Ok(esito) => self.notify(NoticeKind::Info, esito.messaggio()),
                    Err(errore) => self.notify_error(errore),
                };
            }
            // Niente `return` secco: la coda di fine `update` (outline, pannello
            // Struttura, Git stantio) deve girare anche dopo una lettura tornata.
            Message::FileReadFinished(epoch, document_version, reading) => {
                if epoch != self.scritture.guarda(None).epoca {
                    return self.notify_error(
                        "Lettura superata da un'operazione sui file: ripeti l'apertura".into(),
                    );
                }
                if self.file_read_superseded(&reading.path, document_version) {
                    return self.notify_error(
                        "Lettura superata da un aggiornamento del documento: ripeti l'apertura"
                            .into(),
                    );
                }
                if let Some(id) = self.workspace.get_by_path(&reading.path)
                    && (self.restored_notes_revoked.contains_key(&id)
                        || self.restored_notes_candidates.contains_key(&id))
                {
                    // Il reader dedicato riconosce NoteStore e alias; il
                    // seguito dell'apertura resta legato a questo placeholder.
                    if reading.esito.is_ok() {
                        return self.defer_restored_action(
                            id,
                            DeferredRestoredAction::RiprendiApertura {
                                path: reading.path,
                                dopo: reading.dopo,
                            },
                        );
                    }
                }
                let task = self.file_letto(reading);
                return Task::batch([task, self.drain_tool_requests()]);
            }
            Message::RestoredTabReadFinished(id, path, request, epoch, version, result) => {
                return self.restored_tab_read_finished(id, &path, request, epoch, version, result);
            }
            #[cfg(test)]
            Message::FileLetto(lettura) => {
                let task = self.file_letto(lettura);
                return Task::batch([task, self.drain_tool_requests()]);
            }
            Message::DocRicaricato(id, path, versione, esito) => {
                let task = self.doc_ricaricato(id, &path, versione, esito);
                return Task::batch([task, self.drain_tool_requests()]);
            }
            #[cfg(feature = "mcp")]
            Message::McpDocReloaded(id, path, version, result) => {
                return self.mcp_doc_ricaricato(id, &path, version, result);
            }
            Message::NoticeExpired(id) => {
                // Una notifica persistente non arma timer; la guardia tiene
                // fuori anche un timer vecchio con lo stesso id, che oggi non
                // può esistere ma non costa niente escludere.
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|(current, notice)| *current == id && !notice.persistente)
                {
                    self.notice = None;
                }
            }
            Message::GlobalFileSearch(generation, results) => {
                if generation
                    == self
                        .global_search_gen
                        .load(std::sync::atomic::Ordering::SeqCst)
                {
                    self.global_search.finish_file_search(generation, results);
                }
            }
            Message::GitWriteFinished(ticket, result) => {
                return self.finish_git_write(ticket, *result);
            }
            Message::GitLoaded(generation, result, committed) => {
                self.last_git_operation = Some(std::time::Instant::now());
                let committed_ok = committed && result.is_ok();
                let mut tasks = Vec::new();
                // scarta gli snapshot di generazioni superate
                if generation == self.git_gen.load(std::sync::atomic::Ordering::SeqCst) {
                    // Un diff con base «Modifiche locali» è una fotografia del
                    // worktree: dopo un Ripristina, un Commit o una modifica
                    // da fuori mostrerebbe righe che non esistono più. Si
                    // ricalcola insieme allo stato — dentro il controllo di
                    // generazione, o uno snapshot superato lo richiederebbe
                    // una seconda volta per niente. Le basi su commit sono
                    // immutabili: non si toccano.
                    match self.diff_to_refresh(result.as_ref().ok().map(|s| s.files.as_slice())) {
                        DiffRefresh::Refresh(path, base) => {
                            tasks.push(self.open_diff(path, base, true));
                        }
                        DiffRefresh::Close => self.close_diff(),
                        DiffRefresh::Keep => {}
                    }
                    self.branch = result.as_ref().ok().and_then(|s| s.branch.clone());
                    let panel_open = self.registry.is_open(tools::git_native::ID);
                    if let Some(tool) = self.registry.get_mut(tools::git_native::ID) {
                        if panel_open || committed {
                            tool.update(&ToolEvent::new(tools::git_native::Event::Loaded(
                                result, committed,
                            )));
                        }
                        if !panel_open {
                            let _ = tool.on_visibility_changed(false);
                        }
                    }
                }
                if committed_ok && let Some(message) = self.pending_commit_message.take() {
                    let subject = message.lines().next().unwrap_or_default().to_string();
                    self.push_history(
                        lume_store::HistoryAction::Committed,
                        format!("«{subject}»"),
                        None,
                    );
                    tasks.push(self.run_git_log_se_aperto());
                    tasks.push(self.persist_session());
                } else if committed {
                    tasks.push(self.run_git_log_se_aperto());
                }
                return Task::batch(tasks);
            }
            Message::GitBranchLoaded(generation, branch) => {
                if generation
                    == self
                        .git_branch_gen
                        .load(std::sync::atomic::Ordering::SeqCst)
                {
                    self.branch = branch;
                }
            }
            Message::GitLogLoaded(generation, result) => {
                self.last_git_operation = Some(std::time::Instant::now());
                let panel_open = self.registry.is_open(tools::gitlog::ID);
                if generation == self.git_log_gen.load(std::sync::atomic::Ordering::SeqCst)
                    && let Some(tool) = self.registry.get_mut(tools::gitlog::ID)
                {
                    if panel_open {
                        tool.update(&ToolEvent::new(tools::gitlog::Event::Loaded(result)));
                    } else {
                        let _ = tool.on_visibility_changed(false);
                    }
                }
            }
            Message::JjLoaded(generation, result) => {
                if generation == self.jj_gen {
                    self.jj_pending_load = None;
                    self.jj_loading = false;
                    self.send_to_jj_tool(tools::jj::Event::Loaded(result));
                }
            }
            Message::JjLoadCollected(signal) => {
                if self
                    .jj_pending_load
                    .as_ref()
                    .is_some_and(|job| job.matches(&signal))
                {
                    return self.reconcile_jj_load_result();
                }
            }
            Message::JjActionCollected(signal) => {
                if self
                    .jj_pending_action
                    .as_ref()
                    .is_some_and(|job| job.matches(&signal))
                {
                    return self.reconcile_jj_completed_action();
                }
            }
            Message::JjActionDone(generation, action, result) => {
                if self.jj_action_generation != Some(generation) {
                    return Task::none();
                }
                self.jj_busy = false;
                self.jj_pending_action = None;
                self.jj_action_generation = None;
                let deferred_load = self.jj_load_pending;
                self.jj_load_pending = false;
                if generation != self.jj_gen {
                    if deferred_load && self.jj_workspace.is_some() {
                        let load = self.run_jj_load();
                        return Task::batch([load, self.drain_tool_requests()]);
                    }
                    return Task::none();
                }
                let label = action.label();
                let ok = result.is_ok();
                let (kind, text) = match result {
                    Ok(text) => (
                        NoticeKind::Info,
                        format!("Jujutsu - {label}: {}", jj_actions::one_line(&text)),
                    ),
                    Err(text) if jj_actions::is_cancelled(&text) => {
                        (NoticeKind::Info, format!("Jujutsu - {label}: annullato"))
                    }
                    Err(text) => (
                        NoticeKind::Error,
                        format!(
                            "Jujutsu - {label} non riuscita: {}",
                            jj_actions::one_line(&text)
                        ),
                    ),
                };
                self.send_to_jj_tool(tools::jj::Event::ActionDone { action, ok });
                let notice = self.notify(kind, text);
                let load = if deferred_load && self.jj_workspace.is_some() {
                    self.run_jj_load()
                } else {
                    Task::none()
                };
                return Task::batch([notice, load, self.drain_tool_requests()]);
            }
            #[cfg(feature = "sql")]
            Message::Sql(message) => return self.apply_sql_message(message),
            Message::IdleElapsed(feature, epoch) => {
                // Riaperto e richiuso nel frattempo: questo timer e' di
                // un'epoca superata e non deve buttare giu' quella corrente.
                // L'uso si legge adesso, non quando il timer e' stato armato.
                if self
                    .idle
                    .accepts(feature, epoch, self.feature_is_in_use(feature))
                {
                    let stop = self.stop_feature(feature);
                    return Task::batch([stop, self.drain_tool_requests()]);
                }
            }
            #[cfg(feature = "sql")]
            Message::SqlSecretRecalled {
                connection,
                secret,
                error,
            } => {
                // La lettura è asincrona: il profilo può essere stato
                // eliminato nel frattempo. Senza questa guardia il campo
                // password comparirebbe sotto una riga che non c'è più.
                if self.sql_profile(&connection).is_none() {
                    return Task::none();
                }
                let mut tasks = Vec::new();
                if let Some(error) = error
                    && !self.sql_vault_warned
                {
                    // Una volta sola: la cassaforte assente non cambia stato
                    // fra un'espansione e l'altra, e ripeterlo sarebbe rumore.
                    self.sql_vault_warned = true;
                    tasks.push(self.notify_error(format!(
                        "{error} — il segreto verrà chiesto a ogni connessione."
                    )));
                }
                match secret {
                    // C'era: si connette senza chiedere niente. È il caso per
                    // cui esiste tutto questo.
                    Some(secret) => {
                        tasks.push(
                            self.sql_connect(&connection, lume_sql::Secret::new(secret.expose())),
                        );
                    }
                    None => {
                        self.sql_secret_for = Some(connection);
                        self.sync_db_tool();
                    }
                }
                return Task::batch(tasks);
            }
            Message::ShowExtensions => {
                let requests = self.registry.click(tools::ext::ID);
                self.pending_tool_requests.extend(requests);
            }
            Message::ClipboardPasted(Some(testo)) => {
                let testo = testo.replace("\r\n", "\n").replace('\r', "\n");
                let task = self.apply_editor_command(EditorCommand::Paste(testo));
                let lsp = self.lsp_change_task();
                let autosave = self.notes_autosave_schedule();
                return Task::batch([task, lsp, autosave]);
            }
            Message::ClipboardPasted(None) => {}
            Message::Rename(message) => return self.apply_rename_message(message),
            #[cfg(not(test))]
            Message::WorkspaceEditPrepared {
                root_epoch,
                feedback,
                result,
            } => {
                return self.apply_workspace_edit_prepared(root_epoch, feedback, result);
            }
            Message::GotoLine(message) => return self.apply_goto_message(&message),
            Message::FoldToggle => return self.fold_toggle(),
            Message::UnfoldAll => return self.unfold_all(),
            Message::Albero(message) => {
                if self.albero_eliminazione_in_corso()
                    && matches!(message, crate::prompt::PromptMessage::Confirm)
                {
                    return self.notify_error(
                        "Spostamento nel Cestino in corso: attendi prima di creare".into(),
                    );
                }
                return self.apply_albero_message(&message);
            }
            Message::CodeActionChosen(indice) => return self.code_action_chosen(indice),
            Message::AlberoElimina(message) => return self.apply_albero_elimina_message(&message),
            Message::AlberoRinomina(message) => {
                if self.albero_eliminazione_in_corso()
                    && matches!(message, crate::prompt::PromptMessage::Confirm)
                {
                    return self.notify_error(
                        "Spostamento nel Cestino in corso: attendi prima di rinominare".into(),
                    );
                }
                return self.apply_albero_rinomina_message(&message);
            }
            Message::AlberoEliminato(richiesta, esito) => {
                return self.albero_eliminato(richiesta, esito);
            }
            Message::Forge(message) => return self.apply_forge_message(message),
            Message::RestoredTabAction(id, action) => {
                let task = self.resume_restored_tab_action(id, action);
                return Task::batch([task, self.restored_tab_action_task(id)]);
            }
            Message::Recents(message) => return self.apply_recents_message(message),
            Message::Find(message) => {
                if matches!(
                    message,
                    crate::find_replace::FindMessage::Replace
                        | crate::find_replace::FindMessage::ReplaceAll
                ) && let Some(doc) = self.find.as_ref().map(|stato| stato.doc)
                    && self.rifiuta_modifica_per_scrittura(doc)
                {
                    return Task::none();
                }
                return self.apply_find_message(message);
            }
            Message::Feature(command) => return self.apply_feature_command(command),
            Message::FeatureStopConfirmed(feature) => {
                // La conferma si consuma: `apply_feature_command` la vede
                // già impostata e non la richiede una seconda volta.
                return self.apply_feature_command(crate::features::FeatureCommand::Stop(feature));
            }
            Message::FeatureStopCancelled => self.feature_stop_pending = None,
            #[cfg(feature = "sql")]
            #[cfg(feature = "sql")]
            Message::SqlVaultWritten(result) => {
                if let Err(error) = result {
                    return self.notify_error(error);
                }
            }
            #[cfg(feature = "sql")]
            Message::SqlResponse {
                connection,
                pane,
                generation,
                result,
            } => return self.apply_sql_response(&connection, pane, generation, result),
            #[cfg(feature = "sql")]
            Message::SqlQueryEvent {
                connection,
                pane,
                generation,
                delivery,
            } => {
                let accepted = delivery.is_current()
                    && self
                        .sql
                        .as_ref()
                        .and_then(|workspace| workspace.pane(pane))
                        .is_some_and(|target| {
                            target.connection.as_deref() == Some(connection.as_str())
                                && target.query_gen == generation
                        });
                let task = match delivery.take() {
                    Some(event) if accepted => {
                        self.apply_sql_query_event(&connection, pane, generation, event)
                    }
                    _ => Task::none(),
                };
                // Solo dopo applicazione/rigetto. Rigetto fa droppare il
                // receiver async, non drena un vecchio stream verso la UI.
                delivery.acknowledge(accepted);
                return task;
            }
            Message::DiffLoaded {
                request,
                result,
                refresh,
            } => {
                // Un diff superato: nel frattempo se n'e' aperto un altro.
                if self.diff_request.as_ref() != Some(&request) {
                    return Task::none();
                }
                match result {
                    Ok(diff) => self.diff = Some(diff),
                    Err(error) => {
                        self.close_diff();
                        // Un ricalcolo non è un gesto dell'utente e non deve
                        // parlare come tale: `diff_to_refresh` chiude già i
                        // casi previsti, questo è la difesa per gli altri.
                        if !refresh {
                            return self.notify_error(error);
                        }
                    }
                }
            }
            Message::CloseDiff => self.close_diff(),
            Message::StageHunk { path, hunk } => {
                return self.run_git(GitOperation::StageHunk { path, hunk });
            }
            Message::UnstageHunk { path, hunk } => {
                return self.run_git(GitOperation::UnstageHunk { path, hunk });
            }
            Message::SwitchDiffBase(base) => {
                // Il path lo dice la richiesta aperta: l'interruttore esiste
                // solo mentre un diff e' a schermo. `refresh: false` perche'
                // e' un gesto dell'utente, e un errore deve parlare.
                if let Some((path, _)) = self.diff_request.clone() {
                    return self.open_diff(path, base, false);
                }
            }
            Message::GitCommitFilesLoaded(id, result) => {
                let panel_open = self.registry.is_open(tools::gitlog::ID);
                if let Some(tool) = self.registry.get_mut(tools::gitlog::ID) {
                    if panel_open {
                        tool.update(&ToolEvent::new(tools::gitlog::Event::FilesLoaded(
                            id, result,
                        )));
                    } else {
                        let _ = tool.on_visibility_changed(false);
                    }
                }
            }
            Message::FsChanged(paths) => return self.fs_changed_task(paths, true),
            Message::FsRescan(directories) => {
                let paths = self.fs_rescan_paths(directories);
                if paths.is_empty() {
                    return Task::none();
                }
                return self.fs_changed_task(paths, false);
            }
            Message::FsWatchFailed(scope, error) => {
                if self.accepts_watch_scope(&scope) {
                    self.watcher_error = Some((scope, error));
                }
            }
            Message::EnvironmentsDetected(detected) => {
                self.wsl.finita();
                let selected = self.environment_picker.selected_id().to_string();
                let stored = self.environment_picker.profiles().to_vec();
                self.environment_picker
                    .set_profiles(environments::merge_profiles(stored, detected));
                self.environment_picker.select_id(&selected);
                self.sync_terminal_shell();
            }
            Message::LspResolved(server, epoch, esito) => {
                return self.lsp_resolved(&server, epoch, &esito);
            }
            Message::LspInstalled(server, epoch, esito) => {
                return self.lsp_installed(&server, epoch, &esito);
            }
            Message::LspInstallProgress(server, epoch, progress) => {
                return self.lsp_install_progress(&server, epoch, &progress);
            }
            Message::Lsp(server, epoch, evento) => {
                let task = self.lsp_event(&server, epoch, evento);
                self.mcp_sync_tools();
                return task;
            }
            #[cfg(feature = "mcp")]
            Message::Mcp(epoch, evento) => {
                let task = self.mcp_event(epoch, evento);
                return Task::batch([task, self.drain_tool_requests()]);
            }
            #[cfg(feature = "mcp")]
            Message::McpCard(message) => {
                if !self.mcp_card_aperta() {
                    return self.notify(
                        NoticeKind::Info,
                        "La richiesta MCP non è più visibile".into(),
                    );
                }
                let task = self.mcp_card_decide(message == crate::mcp_card::CardMessage::Allow);
                return Task::batch([task, self.drain_tool_requests()]);
            }
            #[cfg(feature = "mcp")]
            Message::McpSetMode(mode) => return self.mcp_apply_mode(mode),
            #[cfg(feature = "mcp")]
            Message::McpModeLoaded(epoch, result) => return self.mcp_mode_loaded(epoch, result),
            #[cfg(feature = "mcp")]
            Message::McpModeSaved(result) => {
                if let Err(error) = result {
                    self.persistence_error = Some(error);
                }
            }
            #[cfg(all(feature = "mcp", not(test)))]
            Message::McpRenamePrepared {
                epoch,
                request_id,
                job_generation,
                conn,
                new_name,
                result,
            } => {
                return self.mcp_rename_prepared(
                    epoch,
                    request_id,
                    job_generation,
                    conn,
                    &new_name,
                    result,
                );
            }
            #[cfg(feature = "mcp")]
            Message::McpOpenFinished {
                epoch,
                request_id,
                path,
                line,
                result,
            } => {
                let task = self.mcp_open_finished(epoch, request_id, path, line, result);
                return Task::batch([task, self.drain_tool_requests()]);
            }
            Message::LspChangeDebounced(id, version) => {
                if self.lsp_flush_change(id, Some(version)) {
                    return self.lsp_change_retry_task(id, 0);
                }
            }
            Message::LspChangeRetry(id, tentativi) => {
                if self.lsp_flush_change(id, None) {
                    if tentativi >= LSP_CHANGE_RETRY_LIMIT {
                        return self.notify_error(
                            "Language server sovraccarico: modifica non sincronizzata".into(),
                        );
                    }
                    return self.lsp_change_retry_task(id, tentativi + 1);
                }
            }
            Message::HoverDue(id, generation, pos) => {
                if generation == self.hover_gen {
                    self.hover_request(id, pos);
                }
            }
            Message::OccurrenceSelected(ticket, result) => {
                return self.occurrence_selected(ticket, result);
            }
            Message::EditorTabBar(message) => {
                let intent = self.editor_tab_bar.update(message);
                tasks.push(self.apply_tab_intent(intent));
            }
            Message::EditorIn(_, EditorCommand::KeepPrimary)
                if self.shell.active_overlay().is_some() => {}
            Message::EditorIn(group, EditorCommand::HoverAt(pos)) => {
                return self.hover_at(group, pos);
            }
            Message::EditorIn(group, EditorCommand::NoteMarkerClicked { line }) => {
                return self.click_note_marker(group, line);
            }
            Message::EditorIn(group, EditorCommand::FoldClicked { line }) => {
                return self.click_fold_marker(group, line);
            }
            Message::EditorIn(group, cmd) => {
                self.hover_clear();
                // battere in una colonna ci porta il fuoco: è il gesto stesso
                // a dirlo, e non serve un `mouse_area` fra l'utente e il testo
                let fuoco_prima = self.groups.focus();
                if let Some(id) = self.groups.active_in(group) {
                    if cmd == EditorCommand::Save
                        && let Some(testo) = self.rifiuto_salvataggio()
                    {
                        return self.notify(NoticeKind::Info, testo);
                    }
                    if self.rifiuta_comando_per_scrittura(id, &cmd) {
                        return Task::none();
                    }
                    let activation = self.activate_tab(id);
                    if self.pending_restored_tabs.contains(id) {
                        match &cmd {
                            EditorCommand::AddNextOccurrence => {
                                let _ = self.queue_restored_editor_command(id, cmd.clone());
                            }
                            EditorCommand::ToggleCursor { .. }
                            | EditorCommand::AddCursorLine { .. }
                            | EditorCommand::KeepPrimary => {
                                self.notify_persistente(
                                    NoticeKind::Info,
                                    "Comando rinviato: attendi il caricamento".into(),
                                );
                            }
                            EditorCommand::Copy => {
                                let _ = self.defer_restored_action(
                                    id,
                                    DeferredRestoredAction::CopySelection,
                                );
                            }
                            EditorCommand::Cut => {
                                let _ = self.defer_restored_action(
                                    id,
                                    DeferredRestoredAction::CutSelection,
                                );
                            }
                            EditorCommand::TriggerComplete => {
                                let _ = self
                                    .defer_restored_action(id, DeferredRestoredAction::Completion);
                            }
                            EditorCommand::Save => {
                                let _ = self.defer_restored_action(
                                    id,
                                    DeferredRestoredAction::SaveDocument,
                                );
                            }
                            _ if can_defer_restored_command(&cmd) => {
                                let _ = self.queue_restored_editor_command(id, cmd.clone());
                            }
                            _ => {}
                        }
                        return activation;
                    }
                }
                if self.groups.focus() != fuoco_prima {
                    // Struttura segue il fuoco (#10). Questo ramo ritorna
                    // prima di `drain_tool_requests`, che altrimenti la
                    // risincronizzerebbe; la guardia sul pannello aperto sta
                    // dentro `sync_structure_tool`.
                    self.sync_outlines();
                    let _ = self.sync_structure_tool();
                }
                if let Some(id) = self.active_doc() {
                    if cmd == EditorCommand::AddNextOccurrence {
                        self.completion_close();
                        return self.next_occurrence_for(id);
                    }
                    if self
                        .multi_occurrence
                        .as_ref()
                        .is_some_and(|ticket| ticket.doc == id)
                        && self.restored_tab_commands.contains_key(&id)
                    {
                        if can_defer_restored_command(&cmd) {
                            let _ = self.queue_restored_editor_command(id, cmd);
                        } else {
                            self.notify_persistente(
                                NoticeKind::Info,
                                "Attendi la ricerca durante il ripristino".into(),
                            );
                        }
                        return Task::none();
                    }
                }
                let save = matches!(cmd, EditorCommand::Save);
                let follow = crate::completion_ui::follow_of(&cmd);
                let inserito = match &cmd {
                    EditorCommand::Insert(testo) => Some(testo.clone()),
                    _ => None,
                };
                self.signature_help_clear();
                let _ = self.apply_editor_command(cmd);
                self.completion_follow(follow, inserito.as_deref());
                if let Some(trigger) = inserito
                    .as_deref()
                    .filter(|text| text.chars().count() == 1 && matches!(*text, "(" | ","))
                    && let Some(id) = self.active_doc()
                {
                    self.signature_help_request(id, Some(trigger.to_owned()));
                }
                self.workspace.trim_histories_to_budget();
                let history_trimmed = self.take_history_trimmed();
                if save {
                    let save = self.save_doc_active();
                    return if history_trimmed {
                        Task::batch([save, self.history_trim_notice()])
                    } else {
                        save
                    };
                }
                let lsp = self.lsp_change_task();
                let autosave = self.notes_autosave_schedule();
                let update = Task::batch([lsp, autosave]);
                return if history_trimmed {
                    Task::batch([update, self.history_trim_notice()])
                } else {
                    update
                };
            }
            Message::Dock(DockMessage::FeatureStart(feature)) => {
                return self.apply_feature_command(crate::features::FeatureCommand::Start(feature));
            }
            Message::Dock(DockMessage::FeatureStop(feature)) => {
                return self.apply_feature_command(crate::features::FeatureCommand::Stop(feature));
            }
            Message::Dock(DockMessage::FeatureStopConfirmed(feature)) => {
                return self.apply_feature_command(crate::features::FeatureCommand::Stop(feature));
            }
            Message::Dock(DockMessage::FeatureStopCancelled) => self.feature_stop_pending = None,
            Message::Dock(DockMessage::Close) => {
                if let Some(id) = self.registry.active_dock {
                    let requests = self.registry.close(id);
                    self.pending_tool_requests.extend(requests);
                }
            }
            Message::SaveActive => return self.save_doc_active(),
            #[cfg(test)]
            Message::SaveDocFixture(id) => return self.save_doc(id),
            #[cfg(test)]
            Message::SaveAllFixture => return self.save_all(),
            #[cfg(test)]
            Message::SaveFinished(completion) => {
                // Le fixture storiche possono eseguire Request direttamente,
                // ma non scavalcare un job posseduto e ancora da raccogliere.
                if self.save_job.is_none() {
                    return self.finish_save(*completion);
                }
            }
            Message::SaveReady(ready) => return self.save_as_ready(ready),
            Message::SaveCollected(receipt) => {
                if self
                    .save_job
                    .as_ref()
                    .is_some_and(|job| job.matches(&receipt))
                    && let Some(completion) = receipt.take()
                {
                    self.save_as_permission = None;
                    self.save_job = None;
                    self.save_job_root_epoch = None;
                    return self.finish_save(completion);
                }
            }
            Message::ReplaceReady(ready) => return self.replace_prepared(ready),
            Message::ReplaceCollected(receipt) => return self.replace_committed(receipt),
            Message::MenuAction(act) => {
                if politica.ingresso == Ingresso::Gesto
                    && self.rifiuta_azioni_sulla_chiusura_durante_cestino()
                {
                    return Task::none();
                }
                return self.apply_menu_intent(MenuIntent::Activate(act));
            }
            Message::Titlebar(message) => match self.titlebar.update(message) {
                TitlebarIntent::ToggleMenuRoot
                | TitlebarIntent::OpenClassic(_)
                | TitlebarIntent::OpenSearch
                    if self.rifiuta_azioni_sulla_chiusura_durante_cestino() => {}
                TitlebarIntent::ToggleMenuRoot => {
                    self.global_search.close();
                    self.cancel_global_file_search();
                    self.menu_bar.update(MenuBarMessage::ToggleRoot);
                    self.environment_picker
                        .update(EnvironmentPickerMessage::Dismiss);
                    self.settings_panel.update(SettingsMessage::Dismiss);
                    if self.menu_bar.is_open() {
                        self.apri_overlay(OverlayId::Menu);
                    } else {
                        self.chiudi_overlay_se(OverlayId::Menu);
                    }
                }
                TitlebarIntent::OpenClassic(index) => {
                    self.global_search.close();
                    self.cancel_global_file_search();
                    self.menu_bar.update(MenuBarMessage::OpenClassic(index));
                    self.environment_picker
                        .update(EnvironmentPickerMessage::Dismiss);
                    self.settings_panel.update(SettingsMessage::Dismiss);
                    self.apri_overlay(OverlayId::Menu);
                }
                TitlebarIntent::HoverClassic(index) => {
                    self.menu_bar.update(MenuBarMessage::HoverClassic(index));
                }
                TitlebarIntent::DragWindow => {
                    return iced::window::latest().and_then(iced::window::drag);
                }
                TitlebarIntent::ToggleMaximize => {
                    return iced::window::latest().and_then(iced::window::toggle_maximize);
                }
                TitlebarIntent::MinimizeWindow => {
                    return iced::window::latest().and_then(|id| iced::window::minimize(id, true));
                }
                TitlebarIntent::CloseWindow => return self.chiudi_finestra(None),
                TitlebarIntent::OpenSearch => return self.open_search(SearchTab::Tutto),
            },
            Message::MenuBar(message) => {
                if politica.ingresso == Ingresso::Gesto
                    && self.rifiuta_azioni_sulla_chiusura_durante_cestino()
                {
                    return Task::none();
                }
                let intent = self.menu_bar.update(message);
                if politica.ingresso == Ingresso::ChiusuraFinestra {
                    self.chiudi_overlay_se(OverlayId::Menu);
                } else if self.menu_bar.is_open() {
                    self.apri_overlay(OverlayId::Menu);
                } else {
                    self.chiudi_overlay_se(OverlayId::Menu);
                }
                return self.apply_menu_intent(intent);
            }
            Message::CopyAbout => return iced::clipboard::write(crate::updates::informazioni()),
            Message::CloseAll => {
                let chiusura = self.chiudi_overlay();
                // Esc revoca sempre il lavoro; se chiude un'altra vista,
                // la fotografia della ricerca nascosta rimane disponibile.
                if chiusura.chiuso == Some(OverlayId::Search) || !self.global_search.is_open() {
                    self.cancel_global_file_search();
                } else {
                    self.suspend_global_file_search();
                }
                match chiusura.chiuso {
                    Some(OverlayId::Menu) => {
                        self.menu_bar.update(MenuBarMessage::Escape);
                    }
                    Some(OverlayId::Environment) => {
                        self.environment_picker
                            .update(EnvironmentPickerMessage::Dismiss);
                    }
                    Some(OverlayId::Settings) => {
                        self.settings_panel.update(SettingsMessage::Dismiss);
                    }
                    Some(OverlayId::Search) => self.global_search.close(),
                    // Il manager ha già chiuso: non chiamare close_rename,
                    // che proverebbe a chiudere ancora.
                    Some(OverlayId::Rename) => self.rename = None,
                    Some(OverlayId::GotoLine) => self.goto_line = None,
                    Some(OverlayId::Albero) => {
                        self.albero = None;
                        self.albero_rinomina = None;
                    }
                    Some(OverlayId::ConfermaElimina) => {
                        // Il lease del Cestino appartiene al coordinatore e
                        // la ricevuta autentica deve poter arrivare.
                        if !self.albero_elimina.as_ref().is_some_and(|r| r.in_corso) {
                            self.albero_elimina = None;
                        }
                    }
                    Some(OverlayId::Find) => self.find = None,
                    Some(OverlayId::Context) => self.context_menu = None,
                    Some(OverlayId::Recents) => self.recents_selected = 0,
                    Some(OverlayId::ConfermaChiusura) => self.chiusura = None,
                    Some(OverlayId::CodeActions) => self.code_actions.clear(),
                    #[cfg(feature = "mcp")]
                    Some(OverlayId::ConsensoMcp) => self.mcp_dimentica_consenso(),
                    #[cfg(feature = "mcp")]
                    Some(OverlayId::CardMcp) => {
                        let _ = self.mcp_card_decide(false);
                    }
                    Some(OverlayId::Forge | OverlayId::About | OverlayId::Scorciatoie) | None => {}
                }
            }
            Message::GlobalSearch(message) => {
                if self.rifiuta_azioni_sulla_chiusura_durante_cestino() {
                    return Task::none();
                }
                // ATTENZIONE: Le frecce e l'Invio del listener globale sono cablati
                // alla ricerca. Quando è aperto l'elenco dei recenti sono
                // **suoi**, o Ctrl+E — che è un comando da tastiera — darebbe
                // un elenco che si può solo cliccare.
                if self.shell.mostra(OverlayId::Recents)
                    && let Some(dirottato) = Self::recents_key(&message)
                {
                    return self.apply_recents_key(dirottato);
                }
                // Il modello conservato non autorizza input dalla vista
                // sostituita, inclusi clic o cambi di testo già in coda.
                if !self.shell.mostra(OverlayId::Search)
                    && matches!(
                        &message,
                        GlobalSearchMessage::Accept
                            | GlobalSearchMessage::Activate(_)
                            | GlobalSearchMessage::Navigate(_)
                            | GlobalSearchMessage::CycleTab
                            | GlobalSearchMessage::QueryChanged(_)
                            | GlobalSearchMessage::TabClicked(_)
                            | GlobalSearchMessage::Escape
                    )
                {
                    // Il listener globale invia questi tasti anche a ricerca
                    // chiusa: in quel caso sono eventi di sfondo, non rifiuti.
                    if !self.global_search.is_open()
                        && matches!(
                            &message,
                            GlobalSearchMessage::Accept
                                | GlobalSearchMessage::Navigate(_)
                                | GlobalSearchMessage::CycleTab
                        )
                    {
                        return Task::none();
                    }
                    return self
                        .notify(NoticeKind::Info, "La ricerca non è più visibile".to_owned());
                }
                // Doppio Maiusc riapre anche una ricerca il cui modello è
                // rimasto dopo la sostituzione, senza cancellarne la query.
                let riapri_con_maiusc = matches!(&message, GlobalSearchMessage::ShiftPressed)
                    && !self.shell.mostra(OverlayId::Search)
                    && self.global_search.is_open()
                    && self.global_search.shift_window_open();
                let refresh_file_results = matches!(
                    &message,
                    GlobalSearchMessage::Open(_)
                        | GlobalSearchMessage::QueryChanged(_)
                        | GlobalSearchMessage::TabClicked(_)
                        | GlobalSearchMessage::CycleTab
                );
                let intent = self.global_search.update(message);
                if riapri_con_maiusc && !self.global_search.shift_window_open() {
                    let task =
                        Task::batch([self.focus_global_search(), self.run_global_file_search()]);
                    self.sync_search_symbols();
                    return task;
                }
                match intent {
                    // ATTENZIONE: Ordine obbligato: `run_global_file_search` chiama
                    // `cancel_file_search` quando la query è vuota o manca la
                    // radice — e quello svuota anche i Simboli (è lo stesso
                    // collo di bottiglia). Sincronizzarli **dopo** garantisce
                    // che l'ultima parola sia dei Simboli freschi, non del
                    // loro azzeramento.
                    GlobalSearchIntent::None if refresh_file_results => {
                        let task = self.run_global_file_search();
                        self.sync_search_symbols();
                        return task;
                    }
                    GlobalSearchIntent::None => {}
                    GlobalSearchIntent::Closed => {
                        self.cancel_global_file_search();
                        self.chiudi_overlay_se(OverlayId::Search);
                    }
                    GlobalSearchIntent::Opened => {
                        let task = Task::batch([
                            self.focus_global_search(),
                            self.run_global_file_search(),
                        ]);
                        self.sync_search_symbols();
                        return task;
                    }
                    GlobalSearchIntent::Activate(index) => {
                        return self.activate_search_entry(index);
                    }
                }
            }
            Message::Statusbar(message) => match self.statusbar.update(message) {
                StatusbarIntent::Environment(message) => {
                    let toggled = message == EnvironmentPickerMessage::Toggle;
                    if toggled {
                        self.global_search.close();
                        self.cancel_global_file_search();
                    }
                    let intent = self.environment_picker.update(message);
                    if toggled {
                        self.menu_bar.close();
                        self.settings_panel.update(SettingsMessage::Dismiss);
                        if self.environment_picker.is_open() {
                            self.apri_overlay(OverlayId::Environment);
                        } else {
                            self.chiudi_overlay_se(OverlayId::Environment);
                        }
                    }
                    match intent {
                        EnvironmentPickerIntent::None => {}
                        EnvironmentPickerIntent::Selected(_) => {
                            self.chiudi_overlay_se(OverlayId::Environment);
                            self.sync_terminal_shell();
                        }
                    }
                    if toggled && self.environment_picker.is_open() {
                        return self.rileva_wsl_se_serve();
                    }
                }
                StatusbarIntent::ChiudiNotifica => self.notice = None,
                StatusbarIntent::CopiaNotifica => {
                    if let Some((_, notice)) = &self.notice {
                        return iced::clipboard::write(notice.text.clone());
                    }
                }
                StatusbarIntent::ShowExtensions => {
                    if !self.registry.is_open(tools::ext::ID) {
                        let requests = self.registry.click(tools::ext::ID);
                        self.pending_tool_requests.extend(requests);
                    }
                }
                StatusbarIntent::McpMode(index) => {
                    return self.mcp_statusbar_mode(index);
                }
                StatusbarIntent::McpCards => return self.mcp_statusbar_cards(),
            },
            Message::Settings(message) => {
                let intent = self.settings_panel.update(message);
                self.apply_settings_intent(intent);
            }
            Message::WindowResizeEdge(dir) => {
                return iced::window::latest()
                    .and_then(move |id| iced::window::drag_resize(id, dir));
            }
            Message::DragStart(kind) => self.layout.start_drag(kind),
            Message::DragMoved(pos) => {
                let intent = self
                    .activity_bar
                    .update(ActivityBarMessage::CursorMoved(pos));
                self.apply_activity_intent(intent);
                // lo stesso movimento alimenta il gesto sulle schede: la
                // soglia è la stessa, e una seconda subscription per gli
                // stessi eventi sarebbe puro spreco
                let intent = self
                    .editor_tab_bar
                    .update(EditorTabBarMessage::CursorMoved(pos));
                tasks.push(self.apply_tab_intent(intent));
                // drag maniglie (pannello/bottom)
                let side = self
                    .registry
                    .active_panel
                    .map(|id| self.registry.side_of(id))
                    .unwrap_or(Side::Left);
                let ctx = crate::layout::DragContext {
                    panel_side: side,
                    dock_height: self.registry.dock_height,
                    editor_width: self.editor_width(),
                };
                if let Some(crate::layout::DragOutcome::DockHeight(height)) =
                    self.layout.drag_moved(pos, ctx)
                {
                    self.registry.set_dock_height(height);
                }
            }
            Message::DragEnd => {
                self.layout.drag = None;
                let intent = self.activity_bar.update(ActivityBarMessage::Release);
                self.apply_activity_intent(intent);
                let intent = self.editor_tab_bar.update(EditorTabBarMessage::Release);
                tasks.push(self.apply_tab_intent(intent));
            }
            Message::ActivityBar(message) => {
                let intent = self.activity_bar.update(message);
                self.apply_activity_intent(intent);
            }
            Message::WindowResized(size) => self.layout.window = size,
            // Blame
            Message::GitBlameLoaded(generation, path, result, dirty) => {
                let panel_open = self.registry.is_open(tools::git_native::ID);
                if generation == self.blame_gen.load(std::sync::atomic::Ordering::SeqCst)
                    && let Some(tool) = self.registry.get_mut(tools::git_native::ID)
                {
                    if panel_open {
                        tool.update(&ToolEvent::new(tools::git_native::Event::BlameLoaded(
                            result, dirty,
                        )));
                    } else {
                        let _ = tool.on_visibility_changed(false);
                    }
                    let _ = path;
                }
            }
            Message::EditorBlameLoaded(id, generation, path, result) => {
                let owned = self.editor_blame_generation.get(&id) == Some(&generation);
                if owned {
                    self.editor_blame_generation.remove(&id);
                    let current = self.editor_blame_enabled
                        && self.workspace.get(id).and_then(|doc| doc.path())
                            == Some(path.as_path());
                    if current && let Ok(rows) = result {
                        self.editor_blame_cache.insert(
                            id,
                            rows.into_iter()
                                .map(|row| {
                                    row.map(|line| lume_ui::editor::BlameAuthorLine {
                                        author_name: line.author_name,
                                    })
                                })
                                .collect(),
                        );
                    }
                }
            }
            // Stash
            Message::GitStashLoaded(generation, result) => {
                let panel_open = self.registry.is_open(tools::git_native::ID);
                if generation == self.git_stash_gen.load(std::sync::atomic::Ordering::SeqCst)
                    && let Some(tool) = self.registry.get_mut(tools::git_native::ID)
                {
                    if panel_open {
                        tool.update(&ToolEvent::new(tools::git_native::Event::StashListLoaded(
                            result,
                        )));
                    } else {
                        let _ = tool.on_visibility_changed(false);
                    }
                }
            }
            // RAM
            Message::PeakMemoryRead(bytes) => {
                self.peak_memory_bytes = bytes;
                self.sync_ext_tool();
            }
            Message::SessionSaved(result) => {
                self.persistence_error = result.err();
            }
            Message::ConfermaChiusura(risposta) => {
                if matches!(risposta, crate::conferma_chiusura::ConfermaMessage::Salva)
                    && let Some(testo) = self.rifiuto_salvataggio()
                {
                    return self.notify(NoticeKind::Info, testo);
                }
                // la chiusura vera passa da qui e non dal fondo di `update`:
                // le richieste dei tool vanno drenate a mano
                let risposta = self.rispondi_chiusura(risposta);
                return Task::batch([risposta, self.drain_tool_requests()]);
            }
            #[cfg(feature = "mcp")]
            Message::ConsensoMcp(risposta) => {
                let risposta = self.rispondi_consenso_mcp(risposta);
                return Task::batch([risposta, self.drain_tool_requests()]);
            }
            Message::WindowCloseRequested(id) => return self.chiudi_finestra(Some(id)),
        }
        tasks.push(self.drain_tool_requests());
        if politica.persistenza == ingresso::Persistenza::DopoGestione {
            tasks.push(self.persist_session());
        }
        Task::batch(tasks)
    }

    fn exit_ready((epoch, success): (u64, bool)) -> Message {
        Message::SearchExitReady(epoch, success)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        static PRIMA_SUBSCRIPTION: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if crate::startup::prima_volta(&PRIMA_SUBSCRIPTION) {
            crate::startup::traccia("prima subscription");
        }
        // listener globale: usa listen_with perché vede anche gli eventi
        // catturati dai widget con focus (a differenza di on_key_press)
        let mut subs = vec![event::listen_with(|e, status, _id| {
            use iced::keyboard::key::Named;
            use iced::keyboard::{Event as K, Key};
            if ignora_evento_globale_catturato(&e, status) {
                return None;
            }
            match e {
                Event::Window(iced::window::Event::Resized(size)) => {
                    Some(Message::WindowResized(size))
                }
                Event::Keyboard(K::KeyPressed {
                    key,
                    repeat,
                    modifiers,
                    ..
                }) => match key {
                    Key::Named(Named::Shift) if !repeat => {
                        Some(Message::GlobalSearch(GlobalSearchMessage::ShiftPressed))
                    }
                    Key::Named(Named::Escape) => Some(Message::CloseAll),
                    Key::Named(Named::ArrowUp) => {
                        Some(Message::GlobalSearch(GlobalSearchMessage::Navigate(-1)))
                    }
                    Key::Named(Named::ArrowDown) => {
                        Some(Message::GlobalSearch(GlobalSearchMessage::Navigate(1)))
                    }
                    Key::Named(Named::Enter) => {
                        Some(Message::GlobalSearch(GlobalSearchMessage::Accept))
                    }
                    Key::Named(Named::Tab) => {
                        Some(Message::GlobalSearch(GlobalSearchMessage::CycleTab))
                    }
                    Key::Character(c)
                        if modifiers.command()
                            && modifiers.shift()
                            && c.eq_ignore_ascii_case("s") =>
                    {
                        Some(Message::MenuAction(MenuAct::SaveAll))
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "s" => {
                        Some(Message::SaveActive)
                    }
                    Key::Character(c)
                        if modifiers.command()
                            && modifiers.shift()
                            && c.eq_ignore_ascii_case("f") =>
                    {
                        Some(Message::MenuAction(MenuAct::FindInFiles))
                    }
                    Key::Character(c)
                        if modifiers.command()
                            && modifiers.shift()
                            && c.eq_ignore_ascii_case("x") =>
                    {
                        Some(Message::ShowExtensions)
                    }
                    Key::Character(c)
                        if modifiers.command()
                            && modifiers.alt()
                            && c.eq_ignore_ascii_case("n") =>
                    {
                        Some(Message::NewNote)
                    }
                    Key::Character(c)
                        if modifiers.command()
                            && !modifiers.alt()
                            && !modifiers.shift()
                            && c.eq_ignore_ascii_case("n") =>
                    {
                        Some(Message::MenuAction(MenuAct::NewFile))
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "o" => {
                        Some(Message::MenuAction(MenuAct::OpenFolder))
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "p" => {
                        Some(Message::MenuAction(MenuAct::SearchFile))
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "b" => {
                        Some(Message::MenuAction(MenuAct::GotoDefinition))
                    }
                    Key::Named(Named::F12) if modifiers.command() => {
                        Some(Message::MenuAction(MenuAct::ShowStructure))
                    }
                    Key::Named(Named::Space) if modifiers.command() && modifiers.shift() => {
                        Some(Message::MenuAction(MenuAct::SignatureHelp))
                    }
                    Key::Named(Named::ArrowLeft) if modifiers.command() && modifiers.alt() => {
                        Some(Message::MenuAction(MenuAct::NavBack))
                    }
                    Key::Named(Named::ArrowRight) if modifiers.command() && modifiers.alt() => {
                        Some(Message::MenuAction(MenuAct::NavForward))
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "e" => {
                        Some(Message::MenuAction(MenuAct::Recents))
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "g" => {
                        Some(Message::MenuAction(MenuAct::GotoLine))
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "f" => {
                        Some(Message::MenuAction(MenuAct::Find))
                    }
                    Key::Character(c) if modifiers.command() && c.as_str() == "r" => {
                        Some(Message::MenuAction(MenuAct::Replace))
                    }
                    Key::Named(Named::F6) if modifiers.shift() => {
                        Some(Message::MenuAction(MenuAct::Rename))
                    }
                    Key::Named(Named::F7) if modifiers.alt() => {
                        Some(Message::MenuAction(MenuAct::FindUsages))
                    }
                    Key::Named(Named::F11) => Some(Message::ToggleBookmark),
                    Key::Named(Named::F12) if modifiers.alt() => {
                        Some(Message::MenuAction(MenuAct::ToggleTerminal))
                    }
                    _ => None,
                },
                _ => None,
            }
        })];
        for lane in [
            lume_syntax::SyntaxLane::Primary,
            lume_syntax::SyntaxLane::Secondary,
        ] {
            subs.push(
                lume_ui::editor::syntax_subscription::subscription(
                    lane,
                    self.syntax_pool.receiver(lane),
                )
                .map(Message::Syntax),
            );
        }
        if let Some(exit) = &self.search_exit {
            subs.push(
                Subscription::run_with(
                    exit.completion.clone(),
                    crate::search_events::exit_completed,
                )
                .map(Self::exit_ready),
            );
        }
        // La X di sistema e Alt+F4: con `exit_on_close_request: false` sono
        // solo una richiesta, e la risposta passa dai file sporchi.
        subs.push(iced::window::close_requests().map(Message::WindowCloseRequested));
        if self.search_jobs.busy() {
            subs.push(
                Subscription::run_with(
                    self.search_jobs.snapshot(),
                    crate::search_events::completions,
                )
                .map(|(ticket, result)| Message::SearchCollected(ticket, result)),
            );
        }
        if let Some(job) = &self.jj_pending_action {
            // Presente anche Pending: sola attesa, mai ammissione del lavoro.
            // Conserva l'invocazione ammessa se sparisce la Task originale.
            subs.push(
                Subscription::run_with(job.collector(), jj_actions::admission::completed)
                    .map(Message::JjActionCollected),
            );
        }
        if let Some(job) = &self.jj_pending_load {
            // Presente anche Pending: raccoglie l'unico worker ammesso, ma non
            // lo ammette. L'esito sopravvive alla perdita della Task originale.
            subs.push(
                Subscription::run_with(job.collector(), jj_actions::load_admission::completed)
                    .map(Message::JjLoadCollected),
            );
        }
        if let Some(picker) = &self.save_as_picker {
            // Raccoglie il dialogo ammesso anche quando la Task è perduta.
            subs.push(
                Subscription::run_with(picker.job.clone(), crate::save::picker_completed)
                    .map(Self::save_picker_event),
            );
        }
        if let Some(job) = &self.save_job {
            // Fallback di sola raccolta: non avvia un save freddo. Rimane
            // anche se la Task originale di consegna viene abbandonata.
            subs.push(
                Subscription::run_with(job.clone(), crate::save::completed).map(Self::save_event),
            );
        }
        if let Some(job) = &self.replace_job {
            subs.push(
                Subscription::run_with(job.clone(), crate::replace_batch::completed)
                    .map(Self::replace_event),
            );
        }
        // Secondo listener, presente solo nei 400 ms dopo uno Shift: un tasto
        // qualsiasi annulla il doppio Shift. Fuori dalla finestra nessuna
        // battuta produce un update.
        if self.global_search.shift_window_open() {
            subs.push(event::listen_with(|e, status, _id| {
                use iced::keyboard::key::Named;
                use iced::keyboard::{Event as K, Key};
                if status == event::Status::Captured
                    && matches!(&e, Event::Keyboard(_) | Event::InputMethod(_))
                {
                    return None;
                }
                match e {
                    Event::Keyboard(K::KeyPressed { key, .. })
                        if !matches!(key, Key::Named(Named::Shift)) =>
                    {
                        Some(Message::GlobalSearch(GlobalSearchMessage::AnyKey))
                    }
                    _ => None,
                }
            }));
        }
        // Un solo watcher nativo, limitato alle directory visibili/in uso.
        subs.push(self.fs_watcher_subscription());
        if self.layout.drag.is_some()
            || self.activity_bar.drag().is_some()
            || self.editor_tab_bar.drag().is_some()
        {
            subs.push(event::listen_with(|e, _status, _id| match e {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::DragMoved(position))
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::DragEnd)
                }
                _ => None,
            }));
        }
        if self.terminal_needs_wakeups() {
            subs.push(terminal_subscription(self.terminal_wakeup.clone()));
        }
        // Timer d'inattivita': **uno sleep per funzionalita' Dormiente**, e
        // solo finche' c'e' davvero qualcosa da rilasciare. Non e' un
        // processo in background — quando scatta e libera l'ultima risorsa,
        // o l'utente riapre una vista, questo ramo smette di essere
        // prodotto, iced droppa lo stream e il costo torna a zero.
        // La chiave comprende l'epoca e non l'istante: `Instant` non e'
        // `Hash`, e cosi' una nuova chiusura ricostruisce lo stream.
        for (feature, epoch) in self.idle.dormant() {
            if let Some(timeout) = crate::features::spec(feature).idle {
                subs.push(idle_subscription(feature, epoch, timeout));
            }
        }
        // ATTENZIONE: Il ramo che **possiede** il processo. Finché è prodotto lo stream
        // vive; smesso di produrlo, iced lo droppa, il future muore e il Job
        // Object porta via l'albero intero. Il vincolo «niente processi da
        // inattivo» è qui, ed è strutturale.
        for spawn in self.lsp_spawns() {
            subs.push(lsp_subscription(spawn));
        }
        self.mcp_subscriptions(&mut subs);
        Subscription::batch(subs)
    }

    pub(crate) fn etichetta_nota_attiva(&self) -> Option<String> {
        let doc = self.active_doc()?;
        self.notes.as_ref()?.salvataggi.get(&doc)?.etichetta()
    }

    pub(crate) fn conflitto_della_scheda(&self, doc: DocId) -> bool {
        self.notes
            .as_ref()
            .and_then(|notes| notes.salvataggi.get(&doc))
            .is_some_and(|state| *state == crate::autosave::StatoSalvataggio::Conflitto)
    }

    fn nuova_generazione_scansione_appunti(&mut self) -> Option<u64> {
        let Some(generation) = self.notes_scan_sequence.checked_add(1) else {
            self.notify_persistente(
                NoticeKind::Error,
                "Impossibile caricare gli Appunti: identità di scansione esaurite".into(),
            );
            return None;
        };
        self.notes_scan_sequence = generation;
        Some(generation)
    }

    fn next_notes_scan_generation(&mut self) -> Option<u64> {
        self.notes.as_ref()?;
        let generation = self.nuova_generazione_scansione_appunti()?;
        let notes = self.notes.as_mut()?;
        notes.scan_generation = generation;
        notes.scan_request = generation;
        self.notes_library_sequence = generation;
        Some(generation)
    }

    /// Generazione e cartella della riscansione da lanciare alla riapertura,
    /// se la cache era stata scaricata; azzera `view_unloaded`.
    fn take_notes_rescan(&mut self) -> Option<(u64, PathBuf)> {
        let notes = self.notes.as_ref()?;
        if !notes.view_unloaded || notes.store.is_none() {
            return None;
        }
        let directory = notes.directory.clone()?;
        let request = self.nuova_generazione_scansione_appunti()?;
        let notes = self.notes.as_mut()?;
        notes.view_unloaded = false;
        notes.scan_request = request;
        Some((request, directory))
    }

    /// Nessun caricamento è mai riuscito (`store` assente): un primo
    /// caricamento fallito, o ancora in volo, non lascerebbe a chi riapre il
    /// pannello né una lista né un errore. Si riparte con una generazione di
    /// scansione nuova (il risultato del tentativo precedente, se arriva, si
    /// scarta) e il pannello, che si sta aprendo, non è più «scaricato».
    fn restart_notes_first_load(&mut self) -> Option<u64> {
        if self.notes.as_ref()?.store.is_some() {
            return None;
        }
        let generation = self.next_notes_scan_generation()?;
        let notes = self.notes.as_mut()?;
        notes.view_unloaded = false;
        Some(generation)
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn mcp_spawn(&self) -> Option<crate::mcp_mode::McpSpawn> {
        self.mcp.as_ref().map(crate::mcp_mode::McpWorkspace::spawn)
    }

    #[cfg(feature = "mcp")]
    fn mcp_subscriptions(&self, subs: &mut Vec<Subscription<Message>>) {
        if let Some(spawn) = self.mcp_spawn() {
            subs.push(mcp_subscription(spawn));
        }
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_subscriptions(&self, _subs: &mut Vec<Subscription<Message>>) {}

    #[cfg(feature = "mcp")]
    fn mcp_forget_discovery(&self) {
        if let Some(workspace) = &self.mcp {
            workspace.cleanup_discovery();
        }
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_forget_discovery(&self) {}

    #[cfg(feature = "mcp")]
    fn mcp_drop_for_new_root(&mut self) {
        self.mcp_shutdown_cards();
        self.mcp_lsp_cancel_all();
        self.mcp = None;
        if self.consenso_mcp_aperto() {
            self.chiudi_overlay_se(OverlayId::ConsensoMcp);
        }
        self.mcp_dimentica_consenso();
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_drop_for_new_root(&mut self) {}

    #[cfg(feature = "mcp")]
    fn mcp_can_start(&self) -> bool {
        self.project_root.is_some() && self.mcp.is_none()
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn mcp_status_text(&self) -> Option<String> {
        crate::mcp_mode::status_text(self.mcp.as_ref())
    }

    #[cfg(not(feature = "mcp"))]
    pub(crate) fn mcp_status_text(&self) -> Option<String> {
        None
    }

    #[cfg(feature = "mcp")]
    fn mcp_detail_text(&self) -> String {
        crate::mcp_mode::detail_text(self.mcp.as_ref())
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_detail_text(&self) -> String {
        String::new()
    }

    #[cfg(feature = "mcp")]
    fn mcp_copies(&self) -> Vec<(&'static str, String)> {
        let Some(exe) = self.mcp.as_ref().and_then(|_| std::env::current_exe().ok()) else {
            return Vec::new();
        };
        let mut copies = vec![
            (
                "Copia per Claude Code",
                crate::mcp_mode::claude_code_command(&exe),
            ),
            ("Copia per Codex", crate::mcp_mode::codex_command(&exe)),
            (
                "Copia JSON (Cursor, Claude Desktop)",
                crate::mcp_mode::mcp_servers_json(&exe),
            ),
        ];
        if let Some(instance) = self.mcp.as_ref().and_then(|mcp| mcp.endpoint.instance()) {
            copies.push(("Copia ID istanza", instance));
        }
        copies
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_copies(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_can_start(&self) -> bool {
        false
    }

    #[cfg(feature = "mcp")]
    fn mcp_availability(&self) -> lume_mcp::Availability {
        let mut availability = lume_mcp::Availability {
            git: self.feature_is_enabled(crate::features::Feature::Git),
            diagnostics: self
                .lsp
                .iter()
                .any(|lsp| matches!(lsp.state, crate::lsp_mode::LspState::Attivo { .. })),
            ..lume_mcp::Availability::default()
        };
        for lsp in self.lsp.iter() {
            availability.hover |= lsp.can_hover();
            availability.definition |= lsp.can_define();
            availability.references |= lsp.can_find_usages();
            availability.symbols |= lsp.can_symbols();
        }
        availability
    }

    #[cfg(feature = "mcp")]
    fn start_mcp(&mut self) -> Task<Message> {
        if self.mcp.is_some() || self.mcp_consenso.is_some() {
            return Task::none();
        }
        let Some(root) = self.project_root.clone() else {
            return self
                .notify_error("Apri una cartella prima di avviare il server MCP".to_owned());
        };
        match lume_mcp::confine::canonical(&root) {
            Ok(root) => {
                // Un solo overlay alla volta: la richiesta MCP sostituisce
                // menu, pannelli e prompt eventualmente ancora aperti.
                self.menu_bar.close();
                self.environment_picker
                    .update(EnvironmentPickerMessage::Dismiss);
                self.settings_panel.update(SettingsMessage::Dismiss);
                self.global_search.close();
                self.cancel_global_file_search();
                self.close_rename();
                self.goto_line = None;
                self.find = None;
                self.context_menu = None;
                self.mcp_consenso = Some(root);
                self.apri_overlay(OverlayId::ConsensoMcp);
                Task::none()
            }
            Err(errore) => self.notify_error(format!(
                "Server MCP: la radice {} non si legge: {errore}",
                root.display()
            )),
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_accendi(&mut self, root: PathBuf) -> Task<Message> {
        let Some(epoch) = self.mcp_epoch.checked_add(1) else {
            return self.notify_error("Server MCP: identificativi di accensione esauriti".into());
        };
        self.mcp_epoch = epoch;
        let data_dir = lume_store::application_data_dir().ok();
        match crate::mcp_mode::McpWorkspace::new(epoch, root, data_dir, self.mcp_availability()) {
            Ok(mcp) => {
                let root = mcp.root.clone();
                self.mcp = Some(mcp);
                let _ = self.sync_extensions_tool();
                match self.store.clone() {
                    Some(store) => Task::perform(
                        async move {
                            store
                                .load_mcp_mode(&root)
                                .await
                                .map_err(|error| error.to_string())
                        },
                        move |result| Message::McpModeLoaded(epoch, result),
                    ),
                    None => {
                        if let Some(mcp) = self.mcp.as_mut() {
                            mcp.mode_fixed = true;
                        }
                        Task::none()
                    }
                }
            }
            Err(errore) => self.notify_error(format!("Server MCP: {errore}")),
        }
    }

    #[cfg(feature = "mcp")]
    fn consenso_mcp_attivo(&self) -> Option<&PathBuf> {
        (self.shell.mostra(OverlayId::ConsensoMcp))
            .then_some(self.mcp_consenso.as_ref())
            .flatten()
    }

    #[cfg(feature = "mcp")]
    fn consenso_mcp_aperto(&self) -> bool {
        self.consenso_mcp_attivo().is_some()
    }

    #[cfg(feature = "mcp")]
    fn mcp_dimentica_consenso(&mut self) {
        self.revoca_attesa_overlay(OverlayId::ConsensoMcp);
        self.mcp_consenso = None;
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn riquadro_consenso_mcp(&self) -> Option<crate::consenso_mcp::Riquadro> {
        self.consenso_mcp_attivo()
            .map(|radice| crate::consenso_mcp::Riquadro {
                radice: radice.display().to_string(),
            })
    }

    #[cfg(feature = "mcp")]
    fn rispondi_consenso_mcp(
        &mut self,
        risposta: crate::consenso_mcp::ConsensoMessage,
    ) -> Task<Message> {
        let Some(radice) = self.consenso_mcp_attivo().cloned() else {
            return Task::none();
        };
        self.mcp_consenso = None;
        self.chiudi_overlay_se(OverlayId::ConsensoMcp);
        match risposta {
            crate::consenso_mcp::ConsensoMessage::Avvia => self.mcp_accendi(radice),
            crate::consenso_mcp::ConsensoMessage::Annulla => Task::none(),
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_event(&mut self, epoch: u64, evento: lume_mcp::McpEvent) -> Task<Message> {
        use lume_mcp::McpEvent;
        if self.mcp.as_ref().is_none_or(|mcp| mcp.epoch != epoch) {
            return Task::none();
        }
        match evento {
            McpEvent::Ready(mailbox) => {
                if let Some(mcp) = self.mcp.as_mut() {
                    mcp.mailbox = Some(mailbox);
                }
                self.mcp_sync_tools();
                let _ = self.sync_extensions_tool();
                Task::none()
            }
            McpEvent::ClientConnected { conn, name } => {
                if let Some(mcp) = self.mcp.as_mut() {
                    mcp.clients.insert(conn, name.clone());
                }
                let _ = self.sync_extensions_tool();
                self.notify(NoticeKind::Info, format!("Client MCP connesso: {name}"))
            }
            McpEvent::ClientGone(conn) => {
                if let Some(mcp) = self.mcp.as_mut() {
                    mcp.clients.remove(&conn);
                }
                let removed = self
                    .mcp
                    .as_mut()
                    .map(|mcp| mcp.gate.remove_conn(conn))
                    .unwrap_or_default();
                if let Some(mcp) = self.mcp.as_mut() {
                    for waiting in removed {
                        mcp.actions.remove(&waiting.id);
                    }
                    for rename in mcp
                        .renames
                        .values_mut()
                        .filter(|rename| rename.conn == conn)
                    {
                        rename.abandoned = true;
                    }
                }
                let _ = self.sync_extensions_tool();
                Task::none()
            }
            McpEvent::Ask { id, conn, query } => self.mcp_ask(id, conn, query),
            McpEvent::AskGone(id) => {
                if let Some(rename) = self.mcp.as_mut().and_then(|mcp| mcp.renames.get_mut(&id)) {
                    // La richiesta LSP resta registrata: la sua risposta va
                    // intercettata e scartata, mai consegnata alla UI utente.
                    rename.abandoned = true;
                    return Task::none();
                }
                self.mcp_remove_card(id);
                let pending = self
                    .mcp
                    .as_mut()
                    .map(|mcp| mcp.lsp_waits.remove_mcp(id))
                    .unwrap_or_default();
                for (server, request) in pending {
                    if let Some(lsp) = self.lsp.get_mut(&server) {
                        let _ = lsp.answered(request);
                    }
                }
                Task::none()
            }
            McpEvent::Failed(motivo) => {
                self.mcp_shutdown_cards();
                self.mcp_lsp_cancel_all();
                self.mcp = None;
                let _ = self.sync_extensions_tool();
                self.notify_error(format!("Server MCP: {motivo}"))
            }
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_ask(&mut self, id: u64, conn: u64, query: lume_mcp::AppQuery) -> Task<Message> {
        if matches!(&query, lume_mcp::AppQuery::Rename { .. }) {
            return self.mcp_rename_start(id, conn, &query);
        }
        let Some(workspace) = self.mcp.as_mut() else {
            return Task::none();
        };
        let effect = crate::mcp_mode::effect_of(&query);
        let target = crate::mcp_mode::target_of(&query);
        let client = workspace
            .clients
            .get(&conn)
            .cloned()
            .unwrap_or_else(|| "Agente".to_owned());
        let (title, lines) = crate::mcp_card::describe(&query);
        let action = crate::mcp_mode::McpAction::Query(query);
        let admission = workspace.gate.admit(
            workspace.mode,
            crate::mcp_gate::Waiting {
                id,
                conn,
                effect,
                target,
                client,
                title,
                lines,
            },
        );
        match admission {
            crate::mcp_gate::Admission::Run => self.mcp_execute(id, action),
            crate::mcp_gate::Admission::Queued => {
                if let Some(workspace) = self.mcp.as_mut() {
                    workspace.actions.insert(id, action);
                }
                Task::none()
            }
            crate::mcp_gate::Admission::Refused(reason) => {
                self.mcp_reply(id, Err(reason.message().to_owned()));
                Task::none()
            }
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_execute(&mut self, id: u64, action: crate::mcp_mode::McpAction) -> Task<Message> {
        let query = match action {
            crate::mcp_mode::McpAction::Query(query) => query,
            crate::mcp_mode::McpAction::Rename { edit, files } => {
                return self.mcp_rename_apply(id, *edit, &files);
            }
        };
        match query {
            lume_mcp::AppQuery::OpenInEditor { relative, line } => {
                let epoch = self.mcp.as_ref().map(|mcp| mcp.epoch).unwrap_or_default();
                self.mcp_open_in_editor(epoch, id, relative, line)
            }
            query @ (lume_mcp::AppQuery::Hover { .. }
            | lume_mcp::AppQuery::Definition { .. }
            | lume_mcp::AppQuery::References { .. }
            | lume_mcp::AppQuery::DocumentSymbols { .. }) => self.mcp_lsp_ask(id, &query),
            lume_mcp::AppQuery::RunCommand { program, .. } => {
                self.push_history(
                    lume_store::HistoryAction::AgentRan,
                    crate::mcp_card::clip(&program, 60),
                    None,
                );
                self.mcp_reply(id, Ok(lume_mcp::AppAnswer::Granted));
                Task::none()
            }
            lume_mcp::AppQuery::WriteFile { relative, content } => {
                let result = self.mcp_write_file(&relative, &content);
                self.mcp_finish(id, result)
            }
            lume_mcp::AppQuery::ReplaceText {
                relative,
                old_text,
                new_text,
                all,
            } => {
                let result = self.mcp_replace_text(&relative, &old_text, &new_text, all);
                self.mcp_finish(id, result)
            }
            query => {
                let answer = self.mcp_answer(query);
                self.mcp_reply(id, answer);
                Task::none()
            }
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_finish(
        &mut self,
        id: u64,
        result: Result<(serde_json::Value, Task<Message>), String>,
    ) -> Task<Message> {
        match result {
            Ok((value, task)) => {
                self.mcp_reply(id, Ok(lume_mcp::AppAnswer::Json(value)));
                task
            }
            Err(error) => {
                self.mcp_reply(id, Err(error));
                Task::none()
            }
        }
    }

    /// Riconvalida il percorso dopo il consenso: fra la domanda e la risposta
    /// la struttura delle directory può essere cambiata.
    #[cfg(feature = "mcp")]
    fn mcp_confine_write(&self, relative: &Path) -> Result<(PathBuf, Option<DocId>), String> {
        let mcp = self.mcp.as_ref().ok_or("il server MCP non è acceso")?;
        let root = lume_mcp::confine::Root::new(&mcp.root)
            .map_err(|error| format!("radice del progetto non leggibile: {error}"))?;
        let resolved = root
            .resolve_for_write(&lume_mcp::confine::to_slash(relative))
            .map_err(|error| error.to_string())?;
        Ok((resolved.absolute, self.mcp_doc_at(&mcp.root, relative)))
    }

    #[cfg(feature = "mcp")]
    fn mcp_refuse_dirty(&self, doc: Option<DocId>, relative: &Path) -> Result<(), String> {
        if doc
            .and_then(|id| self.workspace.get(id))
            .is_some_and(lume_core::Document::is_dirty)
        {
            return Err(format!(
                "{} ha modifiche non salvate nell'editor: salvale o scartale, poi riprova",
                lume_mcp::confine::to_slash(relative)
            ));
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    fn mcp_write_file(
        &mut self,
        relative: &Path,
        content: &str,
    ) -> Result<(serde_json::Value, Task<Message>), String> {
        lume_mcp::edit::check_size(content)?;
        let (absolute, doc) = self.mcp_confine_write(relative)?;
        self.mcp_refuse_dirty(doc, relative)?;
        let created = !absolute.exists();
        let slash = lume_mcp::confine::to_slash(relative);
        std::fs::write(&absolute, content)
            .map_err(|error| format!("scrittura di {slash} fallita: {error}"))?;
        let task = self.mcp_after_write(&absolute, doc);
        Ok((
            serde_json::json!({
                "path": slash,
                "bytes": content.len(),
                "created": created,
                "open_in_editor": doc.is_some(),
            }),
            task,
        ))
    }

    #[cfg(feature = "mcp")]
    fn mcp_replace_text(
        &mut self,
        relative: &Path,
        old_text: &str,
        new_text: &str,
        all: bool,
    ) -> Result<(serde_json::Value, Task<Message>), String> {
        let slash = lume_mcp::confine::to_slash(relative);
        let (absolute, doc) = self.mcp_confine_write(relative)?;
        self.mcp_refuse_dirty(doc, relative)?;
        let metadata = std::fs::metadata(&absolute).map_err(|error| format!("{slash}: {error}"))?;
        if !metadata.is_file() {
            return Err(format!("non è un file: {slash}"));
        }
        if metadata.len() > lume_mcp::edit::MAX_WRITE_BYTES as u64 {
            return Err(format!(
                "{slash} è più grande di 1 MiB: replace_text non lo modifica"
            ));
        }
        let text = std::fs::read_to_string(&absolute)
            .map_err(|error| format!("lettura di {slash} fallita (serve testo UTF-8): {error}"))?;
        let (updated, replacements) = lume_mcp::edit::apply_replace(&text, old_text, new_text, all)
            .map_err(|error| error.to_string())?;
        lume_mcp::edit::check_size(&updated)?;
        std::fs::write(&absolute, &updated)
            .map_err(|error| format!("scrittura di {slash} fallita: {error}"))?;
        let task = self.mcp_after_write(&absolute, doc);
        Ok((
            serde_json::json!({
                "path": slash,
                "replacements": replacements,
                "bytes": updated.len(),
                "open_in_editor": doc.is_some(),
            }),
            task,
        ))
    }

    #[cfg(feature = "mcp")]
    fn mcp_after_write(&mut self, absolute: &Path, doc: Option<DocId>) -> Task<Message> {
        let task = if let Some(id) = doc {
            let path = self
                .workspace
                .get(id)
                .and_then(lume_core::Document::path)
                .map_or_else(|| absolute.to_path_buf(), Path::to_path_buf);
            let Some(version) = self.workspace.get(id).map(lume_core::Document::version) else {
                return Task::none();
            };
            let path_for_read = path.clone();
            let snapshot = self
                .workspace
                .get(id)
                .map(lume_core::Document::read_snapshot);
            let reading = self.readers.read(move || {
                crate::file_read::read_testo_capped(
                    &path_for_read,
                    crate::file_read::MAX_FILE_BYTES,
                )
                .map(|text| text.prepare(snapshot))
                .map_err(|error| error.to_string())
            });
            Task::perform(
                async move { (path, reading.await.unwrap_or_else(Err)) },
                move |(path, result)| Message::McpDocReloaded(id, path, version, result),
            )
        } else {
            Task::none()
        };
        self.push_history(
            lume_store::HistoryAction::AgentWrote,
            tools::bookmarks::display_name(absolute),
            Some(absolute.to_path_buf()),
        );
        task
    }

    #[cfg(feature = "mcp")]
    fn mcp_doc_ricaricato(
        &mut self,
        id: DocId,
        path: &Path,
        version: u64,
        result: Result<crate::file_read::Testo, String>,
    ) -> Task<Message> {
        if self
            .scritture
            .guarda(Some((
                id,
                self.workspace.get(id).and_then(lume_core::Document::path),
            )))
            .file
            != crate::scritture::Presa::Libero
            || self.git_operazione.is_some()
        {
            return Task::none();
        }
        // Anche un errore tardivo deve appartenere al binding corrente.
        let Some(doc) = self.workspace.get(id) else {
            return Task::none();
        };
        if doc.version() != version || doc.path() != Some(path) {
            return Task::none();
        }
        let content = match result {
            Ok(content) => content,
            Err(error) => {
                return self.notify_error(format!(
                    "Ricaricamento di {} dopo la scrittura MCP fallito: {error}",
                    path.display()
                ));
            }
        };
        let fingerprint = content.fingerprint();
        if doc.is_dirty() {
            if self.disk_fingerprints.get(&id) != Some(&fingerprint) {
                self.external_conflicts.insert(id);
                self.notify_persistente(
                    NoticeKind::Error,
                    format!(
                        "{} è cambiato sul disco durante la scrittura MCP: il buffer locale è conservato; verifica il file",
                        path.display()
                    ),
                );
            }
            return Task::none();
        }
        if self.disk_fingerprints.get(&id) == Some(&fingerprint) {
            return Task::none();
        }
        let Some(doc) = self.workspace.get_mut(id) else {
            return Task::none();
        };
        let Some(edit) = content.apply_reload(doc) else {
            return self
                .notify_error("Ricaricamento MCP preparato obsoleto o già consumato".to_owned());
        };
        self.resync_syntax(id, edit);
        self.disk_fingerprints.insert(id, fingerprint);
        self.external_conflicts.remove(&id);
        self.lsp_flush_change(id, None);
        self.lsp_did_save(id);
        Task::none()
    }

    #[cfg(feature = "mcp")]
    fn mcp_reply(&self, id: u64, answer: Result<lume_mcp::AppAnswer, String>) {
        if let Some(mcp) = self.mcp.as_ref() {
            mcp.answer(id, answer);
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_card_decide(&mut self, allow: bool) -> Task<Message> {
        let waiting = self.mcp.as_mut().and_then(|mcp| mcp.gate.resolve_front());
        let Some(waiting) = waiting else {
            return Task::none();
        };
        let action = self
            .mcp
            .as_mut()
            .and_then(|mcp| mcp.actions.remove(&waiting.id));
        if !allow {
            self.mcp_reply(waiting.id, Err("azione negata dall'utente".to_owned()));
            return Task::none();
        }
        action
            .map(|action| self.mcp_execute(waiting.id, action))
            .unwrap_or_else(Task::none)
    }

    #[cfg(feature = "mcp")]
    fn mcp_remove_card(&mut self, id: u64) {
        if let Some(mcp) = self.mcp.as_mut() {
            let _ = mcp.gate.remove(id);
            mcp.actions.remove(&id);
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_shutdown_cards(&mut self) {
        self.revoca_attesa_overlay(OverlayId::CardMcp);
        let drained = self
            .mcp
            .as_mut()
            .map(|mcp| {
                let waiting = mcp.gate.drain();
                let actions = &mut mcp.actions;
                waiting
                    .into_iter()
                    .map(|entry| (entry.id, actions.remove(&entry.id)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (id, _) in drained {
            self.mcp_reply(id, Err("server MCP fermato".to_owned()));
        }
        if !self.uscita_in_corso {
            self.chiudi_overlay_se(OverlayId::CardMcp);
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_apply_mode(&mut self, mode: lume_mcp::Mode) -> Task<Message> {
        let Some((root, decided)) = self.mcp.as_mut().map(|mcp| {
            mcp.mode = mode;
            mcp.mode_fixed = true;
            (mcp.root.clone(), mcp.gate.redecide(mode))
        }) else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        for decision in decided {
            let action = self
                .mcp
                .as_mut()
                .and_then(|mcp| mcp.actions.remove(&decision.id));
            match (decision.verdict, action) {
                (lume_mcp::Verdict::Allow, Some(action)) => {
                    tasks.push(self.mcp_execute(decision.id, action))
                }
                (lume_mcp::Verdict::Refuse(reason), _) => {
                    self.mcp_reply(decision.id, Err(reason.message().to_owned()))
                }
                _ => {}
            }
        }
        if let Some(store) = self.store.clone() {
            let saved = mode.as_str().to_owned();
            tasks.push(Task::perform(
                async move {
                    store
                        .save_mcp_mode(&root, &saved)
                        .await
                        .map_err(|error| error.to_string())
                },
                Message::McpModeSaved,
            ));
        }
        Task::batch(tasks)
    }

    #[cfg(feature = "mcp")]
    fn mcp_mode_loaded(
        &mut self,
        epoch: u64,
        result: Result<Option<String>, String>,
    ) -> Task<Message> {
        let Some(mcp) = self
            .mcp
            .as_mut()
            .filter(|mcp| mcp.epoch == epoch && !mcp.mode_fixed)
        else {
            return Task::none();
        };
        mcp.mode = result
            .ok()
            .flatten()
            .as_deref()
            .and_then(lume_mcp::Mode::from_str_name)
            .unwrap_or_default();
        mcp.mode_fixed = true;
        Task::none()
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn mcp_mode(&self) -> Option<lume_mcp::Mode> {
        self.mcp.as_ref().map(|mcp| mcp.mode)
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn mcp_mode_index(&self) -> Option<u8> {
        self.mcp_mode().map(lume_mcp::Mode::index)
    }

    #[cfg(not(feature = "mcp"))]
    #[allow(clippy::unused_self)]
    pub(crate) fn mcp_mode_index(&self) -> Option<u8> {
        None
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn mcp_pending(&self) -> usize {
        self.mcp.as_ref().map_or(0, |mcp| mcp.gate.len())
    }

    #[cfg(not(feature = "mcp"))]
    #[allow(clippy::unused_self)]
    pub(crate) fn mcp_pending(&self) -> usize {
        0
    }

    #[cfg(feature = "mcp")]
    fn mcp_statusbar_mode(&mut self, index: u8) -> Task<Message> {
        self.update_inner(Message::McpSetMode(lume_mcp::Mode::from_index(index)))
    }

    #[cfg(not(feature = "mcp"))]
    #[allow(clippy::unused_self)]
    fn mcp_statusbar_mode(&mut self, _index: u8) -> Task<Message> {
        Task::none()
    }

    #[cfg(feature = "mcp")]
    fn mcp_statusbar_cards(&mut self) -> Task<Message> {
        if self.mcp_card_aperta() {
            Task::none()
        } else {
            self.update_inner(Message::CloseAll)
        }
    }

    #[cfg(not(feature = "mcp"))]
    #[allow(clippy::unused_self)]
    fn mcp_statusbar_cards(&mut self) -> Task<Message> {
        Task::none()
    }

    #[cfg(feature = "mcp")]
    fn mcp_card_aperta(&self) -> bool {
        self.shell.mostra(OverlayId::CardMcp)
    }

    #[cfg(feature = "mcp")]
    fn mcp_sync_card(&mut self) {
        // L'overlay fotografato dal ticket non cambia durante la raccolta.
        if self.uscita_in_corso {
            return;
        }
        if self.mcp_card_view().is_some() {
            self.apri_overlay(OverlayId::CardMcp);
        } else {
            self.revoca_attesa_overlay(OverlayId::CardMcp);
            self.chiudi_overlay_se(OverlayId::CardMcp);
        }
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_sync_card(&mut self) {}

    #[cfg(feature = "mcp")]
    fn mcp_sync_tools(&mut self) {
        let availability = self.mcp_availability();
        if let Some(mcp) = self.mcp.as_mut() {
            let _ = mcp.sync_tools(availability);
        }
    }

    #[cfg(not(feature = "mcp"))]
    fn mcp_sync_tools(&mut self) {}

    #[cfg(feature = "mcp")]
    fn mcp_doc_at(&self, root: &Path, relative: &Path) -> Option<DocId> {
        self.workspace.ids().find(|&id| {
            self.workspace
                .get(id)
                .and_then(lume_core::Document::path)
                .and_then(|path| crate::mcp_mode::relative_to_root(root, path))
                .is_some_and(|found| crate::mcp_mode::same_relative(&found, relative))
        })
    }

    #[cfg(feature = "mcp")]
    fn mcp_open_finished(
        &mut self,
        epoch: u64,
        request_id: u64,
        path: PathBuf,
        line: Option<usize>,
        result: Result<crate::file_read::Testo, String>,
    ) -> Task<Message> {
        let Some(root) = self
            .mcp
            .as_ref()
            .filter(|mcp| mcp.epoch == epoch)
            .map(|mcp| mcp.root.clone())
        else {
            return Task::none();
        };
        if let Err(error) = self.check_file_binding(&path) {
            if let Some(mcp) = self.mcp.as_ref() {
                mcp.answer(request_id, Err(error.to_string()));
            }
            return Task::none();
        }
        match result {
            Err(error) => {
                if let Some(mcp) = self.mcp.as_ref() {
                    mcp.answer(request_id, Err(error));
                }
            }
            Ok(text) => {
                let existing = crate::mcp_mode::relative_to_root(&root, &path)
                    .and_then(|relative| self.mcp_doc_at(&root, &relative));
                if let Some(id) = existing {
                    // Una seconda apertura può arrivare mentre il worker
                    // legge: conserva il buffer già aperto, anche se sporco.
                    self.activate_editor_tab(id);
                    self.close_diff_if_primary();
                    self.refresh_editor_blame_availability();
                    self.queue_editor_blame(id);
                    self.push_history(
                        lume_store::HistoryAction::Opened,
                        tools::bookmarks::display_name(&path),
                        Some(path.clone()),
                    );
                } else {
                    if let Err(error) = self.apri_preparato(&path, &text) {
                        self.mcp_reply(request_id, Err(error));
                        return Task::none();
                    }
                }
                if let Some(line) = line {
                    self.goto_line(line);
                }
                let answer = crate::mcp_mode::relative_to_root(&root, &path)
                    .map(|relative| {
                        serde_json::json!({
                            "opened": lume_mcp::confine::to_slash(&relative),
                            "line": line,
                        })
                    })
                    .ok_or_else(|| "il percorso aperto è fuori dalla radice MCP".to_owned())
                    .map(lume_mcp::AppAnswer::Json);
                if let Some(mcp) = self.mcp.as_ref() {
                    mcp.answer(request_id, answer);
                }
            }
        }
        Task::none()
    }

    #[cfg(feature = "mcp")]
    fn mcp_answer(&mut self, query: lume_mcp::AppQuery) -> Result<lume_mcp::AppAnswer, String> {
        use lume_mcp::{AppAnswer, AppQuery};
        use serde_json::json;

        // La radice autorizzata è lo snapshot canonico dell'accensione MCP.
        // Così le risposte non dipendono da un percorso UI alias o relativo.
        let root = self
            .mcp
            .as_ref()
            .map(|mcp| mcp.root.clone())
            .ok_or_else(|| "server MCP fermo".to_owned())?;
        match query {
            AppQuery::OpenFiles => {
                let active = self.active_doc();
                let mut files = Vec::new();
                let mut outside_root = 0_usize;
                for id in self.workspace.ids() {
                    let Some(doc) = self.workspace.get(id) else {
                        continue;
                    };
                    let Some(path) = doc.path() else {
                        continue;
                    };
                    match crate::mcp_mode::relative_to_root(&root, path) {
                        Some(relative) => {
                            let path = lume_mcp::confine::to_slash(&relative);
                            files.push((
                                path.clone(),
                                json!({
                                    "path": path,
                                    "version": doc.version(),
                                    "dirty": doc.is_dirty(),
                                    "active": active == Some(id),
                                }),
                            ));
                        }
                        None => outside_root += 1,
                    }
                }
                files.sort_by(|a, b| a.0.cmp(&b.0));
                let files: Vec<_> = files.into_iter().map(|(_, file)| file).collect();
                Ok(AppAnswer::Json(json!({
                    "files": files,
                    "outside_root": outside_root,
                })))
            }
            AppQuery::ActiveFile => {
                let id = self
                    .active_doc()
                    .ok_or_else(|| "nessun file attivo".to_owned())?;
                let doc = self
                    .workspace
                    .get(id)
                    .ok_or_else(|| "nessun file attivo".to_owned())?;
                let path = doc
                    .path()
                    .ok_or_else(|| "il documento attivo non è un file".to_owned())?;
                let relative = crate::mcp_mode::relative_to_root(&root, path)
                    .ok_or_else(|| "il file attivo è fuori dalla radice del progetto".to_owned())?;
                Ok(AppAnswer::Json(crate::mcp_mode::active_file_json(
                    &relative, doc,
                )))
            }
            AppQuery::Buffer { relative } => Ok(AppAnswer::Buffer(
                self.mcp_doc_at(&root, &relative)
                    .and_then(|id| self.workspace.get(id))
                    .map(crate::mcp_mode::buffer_snapshot),
            )),
            AppQuery::Diagnostics {
                relative,
                max_severity,
            } => {
                let active: Vec<&lume_lsp::DiagnosticStore> = self
                    .lsp
                    .iter()
                    .filter(|lsp| matches!(lsp.state, crate::lsp_mode::LspState::Attivo { .. }))
                    .map(|lsp| &lsp.problems)
                    .collect();
                if active.is_empty() {
                    return Err("nessun language server attivo".to_owned());
                }
                Ok(AppAnswer::Json(crate::mcp_mode::diagnostics_json(
                    &root,
                    &lume_lsp::DiagnosticStore::merged_snapshot(active),
                    relative.as_deref(),
                    max_severity,
                )))
            }
            AppQuery::OpenInEditor { .. } => {
                Err("open_in_editor richiede la lettura asincrona".to_owned())
            }
            AppQuery::WriteFile { .. }
            | AppQuery::ReplaceText { .. }
            | AppQuery::RunCommand { .. }
            | AppQuery::Rename { .. } => {
                Err("tool MCP mutante non ancora integrato nell'app".to_owned())
            }
            AppQuery::Hover { .. }
            | AppQuery::Definition { .. }
            | AppQuery::References { .. }
            | AppQuery::DocumentSymbols { .. } => {
                Err("le richieste LSP devono passare dal routing asincrono".to_owned())
            }
        }
    }

    fn apply_prepared(
        &mut self,
        prepared: crate::rename::PreparedEdit,
    ) -> Result<usize, crate::rename::RenameError> {
        use crate::rename::RenameError;

        // L'I/O è già stato eseguito nel worker. Qui si verifica che nessuna
        // revisione in memoria sia cambiata mentre il worker lavorava.
        for file in &prepared.files {
            if let Some(id) = self.workspace.get_by_path(&file.path) {
                if self.documento_bloccato_da_scrittura(id) {
                    return Err(RenameError::Stale(file.path.clone()));
                }
                let document = self
                    .workspace
                    .get(id)
                    .ok_or_else(|| RenameError::Stale(file.path.clone()))?;
                let version = document.version();
                let synced = self
                    .lsp
                    .holding(id)
                    .is_some_and(|lsp| lsp.is_synced(id, version));
                if file.source_document.is_some_and(|expected| expected != id)
                    || file
                        .source_version
                        .is_some_and(|expected| version != expected)
                    || !synced
                    || document.rope() != &file.source
                    || self.disk_fingerprints.get(&id).copied() != file.source_fingerprint
                {
                    return Err(RenameError::Stale(file.path.clone()));
                }
            } else if file.source_document.is_some()
                || self.note_delete.is_some()
                || self.albero_eliminazione_in_corso()
            {
                return Err(RenameError::Stale(file.path.clone()));
            }
        }

        // I file chiusi entrano nell'editor solo dopo il preflight completo.
        for file in &prepared.files {
            if self.workspace.get_by_path(&file.path).is_none() {
                let text = file.source.to_string();
                let id = self.workspace.open(Some(file.path.clone()), &text);
                if let Some(fingerprint) = file.source_fingerprint {
                    self.disk_fingerprints.insert(id, fingerprint);
                }
                self.attach_syntax(id, &file.path);
                self.groups.open_quietly(self.groups.focus(), id);
            }
        }

        // Risolve tutti i documenti prima di applicare la prima modifica.
        let mut targets = Vec::with_capacity(prepared.files.len());
        for file in prepared.files {
            let id = self
                .workspace
                .get_by_path(&file.path)
                .ok_or_else(|| RenameError::Stale(file.path.clone()))?;
            targets.push((id, file));
        }

        let count = targets.len();
        for (id, file) in targets {
            let Some(document) = self.workspace.get_mut(id) else {
                return Err(RenameError::Stale(file.path));
            };
            document.apply(lume_core::Transaction {
                changes: file.changeset,
                selection_after: None,
            });
            self.rebuild_syntax(id, &file.path);
            self.lsp_flush_change(id, None);
        }
        Ok(count)
    }

    fn open_shortcuts(&mut self) -> Task<Message> {
        self.menu_bar.close();
        self.apri_overlay(OverlayId::Scorciatoie);
        Task::none()
    }

    /// Le diagnostiche vere nel dock Problemi. È la riga che toglie l'ultimo
    /// mock del progetto: da qui il badge si accende perché c'è qualcosa,
    /// non perché lo abbiamo disegnato.
    fn refresh_marks(&mut self, id: DocId) {
        let Some(document) = self.workspace.get(id) else {
            self.doc_marks.remove(&id);
            return;
        };
        let problems: Vec<lume_lsp::Problem> = self
            .lsp
            .iter()
            .filter_map(|lsp| lsp.opened.get(&id).map(|uri| lsp.problems.for_uri(uri)))
            .flatten()
            .collect();
        if problems.is_empty() {
            self.doc_marks.remove(&id);
        } else {
            let marks = crate::diag_marks::marks_for(&problems, document.rope());
            self.doc_marks.insert(id, marks);
        }
    }

    /// Aggiorna i documenti con segni correnti o aperti da un server.
    fn refresh_all_marks(&mut self) {
        let mut ids: std::collections::HashSet<DocId> = self.doc_marks.keys().copied().collect();
        ids.extend(self.lsp.iter().flat_map(|lsp| lsp.opened.keys().copied()));
        for id in ids {
            self.refresh_marks(id);
        }
    }

    /// Svuota la coda dei cambi di visibilita' in sospeso e recupera il Git
    /// diventato stantio. Idempotente: si puo' chiamare sia sui percorsi con
    /// `return` anticipato sia in coda a `update`.
    fn drain_tool_requests(&mut self) -> Task<Message> {
        self.reconcile_feature_visibility();
        self.reconcile_file_palette();
        let mut tasks = vec![self.refresh_stale_git()];
        if !self.pending_tool_requests.is_empty() {
            let requests = std::mem::take(&mut self.pending_tool_requests);
            tasks.push(self.apply_tool_requests(requests));
        }
        let _ = self.sync_extensions_tool();
        self.sync_outlines();
        let _ = self.sync_structure_tool();
        Task::batch(tasks)
    }

    /// All'avvio nessun tool ha ricevuto una transizione: notifica una volta
    /// chi risulta aperto dopo il ripristino della sessione, altrimenti un
    /// tool gia' attivo non allocherebbe mai le proprie risorse.
    fn notify_initial_visibility(&mut self) {
        let open: Vec<ToolId> = [self.registry.active_panel, self.registry.active_dock]
            .into_iter()
            .flatten()
            .collect();
        for id in open {
            let requests = self.registry.notify_visibility(id, true);
            self.pending_tool_requests.extend(requests);
        }
    }

    fn apply_settings_intent(&mut self, intent: SettingsIntent) {
        match intent {
            SettingsIntent::None => {}
            SettingsIntent::SelectColorTheme(index) => {
                if let Some(theme) = self.theme_state.list.get(index) {
                    self.theme_state.iced = theme.iced_theme();
                    self.theme_state.sel = index;
                }
            }
            SettingsIntent::SelectIconTheme(id) => {
                if !self.icon_themes.select(&id) {
                    self.persistence_error = Some(format!("tema icone non disponibile: {id}"));
                }
            }
            SettingsIntent::RescanIconThemes => {
                self.icon_themes.rescan();
                self.settings_panel
                    .set_selected_icon_theme(self.icon_themes.preferred_id());
            }
            // Solo righe del selettore: preferenza e tema attivo non cambiano.
            SettingsIntent::ScanIconThemes => self.icon_themes.scan_external_once(),
        }
    }

    fn apply_activity_intent(&mut self, intent: ActivityBarIntent) {
        match intent {
            ActivityBarIntent::None => {}
            ActivityBarIntent::Click(id) => {
                self.global_search.close();
                self.cancel_global_file_search();
                let requests = self.registry.click(id);
                self.pending_tool_requests.extend(requests);
                self.settings_panel.update(SettingsMessage::Dismiss);
                self.chiudi_overlay_se(OverlayId::Settings);
            }
            ActivityBarIntent::Drop(drag) => self.drop_icon(&drag),
            ActivityBarIntent::ToggleSettings => {
                self.global_search.close();
                self.cancel_global_file_search();
                let message = if self.settings_panel.is_open() {
                    SettingsMessage::Dismiss
                } else {
                    SettingsMessage::Open
                };
                self.settings_panel.update(message);
                self.menu_bar.close();
                self.environment_picker
                    .update(EnvironmentPickerMessage::Dismiss);
                if self.settings_panel.is_open() {
                    // Il selettore dei temi icone è visibile: prima richiesta
                    // di Aspetto, scansione completa dei pacchetti esterni.
                    let intent = self
                        .settings_panel
                        .update(SettingsMessage::AppearanceRequested);
                    self.apply_settings_intent(intent);
                    self.apri_overlay(OverlayId::Settings);
                } else {
                    self.chiudi_overlay_se(OverlayId::Settings);
                }
            }
        }
    }

    fn drop_icon(&mut self, d: &IconDrag) {
        let w = self.layout.window.width;
        let side = if d.cursor.x <= ACTIVITY_W {
            Side::Left
        } else if d.cursor.x >= w - ACTIVITY_W {
            Side::Right
        } else {
            return; // drop fuori dalle barre: annulla
        };
        // barra sinistra a due gruppi (pannelli in alto, dock in basso sopra
        // l'ingranaggio); barra destra a gruppo unico
        let bar_height = self.layout.window.height - TITLEBAR_H - crate::metrics::STATUS_H;
        let y = d.cursor.y - TITLEBAR_H;
        let others =
            |ids: Vec<ToolId>| -> Vec<ToolId> { ids.into_iter().filter(|&i| i != d.id).collect() };
        let (top, bottom) = match side {
            Side::Left => (
                others(self.registry.bar_of_kind(side, ToolKind::SidePanel)),
                others(self.registry.bar_of_kind(side, ToolKind::Dock)),
            ),
            Side::Right => (others(self.registry.bar(side)), Vec::new()),
        };
        let slot =
            crate::activity::drop_slot(y, bar_height, top.len(), bottom.len(), side == Side::Left);
        let before = match slot.group {
            crate::activity::DropGroup::Top => top.get(slot.index),
            crate::activity::DropGroup::Bottom => bottom.get(slot.index),
        }
        .copied();
        self.registry.move_icon(d.id, side, before);
    }

    // ── RAM ──────────────────────────────────────────────────────────

    fn run_read_peak_memory(&mut self) -> Task<Message> {
        self.ram_gen = self.ram_gen.wrapping_add(1);
        Task::perform(
            async move { crate::memory::read_peak_memory_bytes() },
            Message::PeakMemoryRead,
        )
    }

    /// Sincronizza il picco di memoria nel tool Extensions.
    fn sync_ext_tool(&mut self) {
        if let Some(tool) = self.registry.get_mut(tools::ext::ID) {
            tool.update(&ToolEvent::new(tools::ext::Event::PeakMemoryUpdated(
                self.peak_memory_bytes,
            )));
        }
    }

    fn send_to_jj_tool(&mut self, event: tools::jj::Event) {
        let requests = self
            .registry
            .get_mut(tools::jj::ID)
            .map(|tool| tool.update(&ToolEvent::new(event)))
            .unwrap_or_default();
        self.pending_tool_requests
            .extend(requests.into_iter().map(|request| (tools::jj::ID, request)));
    }

    fn sync_jj_workspace(&mut self) {
        let workspace = self
            .project_root
            .as_deref()
            .and_then(lume_jj::find_workspace_root);
        if workspace == self.jj_workspace {
            return;
        }
        self.jj_cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.jj_gen += 1;
        self.jj_loading = false;
        if self
            .jj_pending_load
            .as_ref()
            .is_some_and(jj_actions::load_admission::Pending::revoke)
        {
            self.jj_pending_load = None;
        }
        if self
            .jj_pending_action
            .as_ref()
            .is_some_and(crate::jj_actions::admission::Pending::revoke)
        {
            self.jj_busy = false;
            self.jj_pending_action = None;
            self.jj_action_generation = None;
            self.jj_load_pending = false;
        }
        let unavailable = workspace.is_none();
        if unavailable {
            self.jj_load_pending = false;
        }
        self.jj_workspace = workspace.clone();
        self.send_to_jj_tool(tools::jj::Event::SetWorkspace(workspace));
        let requests = self.registry.set_unavailable(tools::jj::ID, unavailable);
        self.pending_tool_requests.extend(requests);
    }

    fn reconcile_jj_cold_action(&mut self) {
        if !self
            .jj_pending_action
            .as_ref()
            .is_some_and(|pending| pending.abandoned())
        {
            return;
        }
        let Some(pending) = self.jj_pending_action.take() else {
            return;
        };
        // Permit perso PRIMA dell'ammissione: nessun worker da raccogliere.
        // Admitted non passa da qui, neppure se Task/risultato vengono persi.
        self.jj_busy = false;
        self.jj_action_generation = None;
        self.send_to_jj_tool(tools::jj::Event::ActionDone {
            action: pending.into_action(),
            ok: false,
        });
    }

    fn reconcile_jj_completed_action(&mut self) -> Task<Message> {
        let Some(generation) = self.jj_action_generation else {
            return Task::none();
        };
        let Some(result) = self
            .jj_pending_action
            .as_ref()
            .and_then(|job| job.take_completion())
        else {
            return Task::none();
        };
        let Some(pending) = self.jj_pending_action.take() else {
            return Task::none();
        };
        // Current ticket conserva il risultato autentico dopo l'await; non
        // risultato ricostruito da un messaggio perso o retry della Describe.
        self.update_inner(Message::JjActionDone(
            generation,
            pending.into_action(),
            result,
        ))
    }

    fn reconcile_jj_cold_load(&mut self) {
        if !self
            .jj_pending_load
            .as_ref()
            .is_some_and(|pending| pending.abandoned())
        {
            return;
        }
        self.jj_pending_load = None;
        self.jj_loading = false;
        // Solo Pending abbandonato: nessun worker da raccogliere/rollback.
        // Loaded(Err) libera anche il gate del tool PRIMA del nuovo input.
        self.send_to_jj_tool(tools::jj::Event::Loaded(Err(
            "Caricamento Jujutsu non avviato: richiesta abbandonata".into(),
        )));
    }

    fn reconcile_jj_load_result(&mut self) -> Task<Message> {
        self.reconcile_jj_cold_load();
        let Some((generation, result)) = self.jj_pending_load.as_ref().and_then(|pending| {
            pending
                .take_completion()
                .map(|result| (pending.generation(), result))
        }) else {
            return self.drain_tool_requests();
        };
        if generation != self.jj_gen {
            self.jj_pending_load = None;
            self.jj_loading = false;
            // Il load del nuovo workspace può partire solo dopo aver raccolto
            // e rilasciato l'owner ammesso del workspace precedente.
            if self.jj_load_pending {
                self.jj_load_pending = false;
                return self.run_jj_load();
            }
            return self.drain_tool_requests();
        }
        // Solo il ticket app ancora corrente può trasformare il risultato
        // condiviso nell'evento del tool. Duplicati producer/subscription
        // vengono ignorati quando il primo ha rimosso `jj_pending_load`.
        self.update_inner(Message::JjLoaded(generation, result))
    }

    fn run_jj_load(&mut self) -> Task<Message> {
        if self.search_exit.is_some() {
            self.jj_load_pending = true;
            return Task::none();
        }
        self.reconcile_jj_cold_load();
        self.reconcile_jj_cold_action();
        let Some(root) = self.jj_workspace.clone() else {
            return Task::none();
        };
        if self.jj_busy {
            self.jj_load_pending = true;
            return Task::none();
        }
        if self.jj_loading {
            return Task::none();
        }
        if self.jj_pending_load.is_some() {
            // Un worker ammesso del workspace precedente conserva l'owner;
            // il collector lo rilascerà prima di avviare il successore.
            self.jj_load_pending = true;
            return Task::none();
        }
        self.jj_loading = true;
        self.jj_load_pending = false;
        self.jj_gen += 1;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.jj_cancel = cancel.clone().into();
        let generation = self.jj_gen;
        let (pending, permit) =
            jj_actions::load_admission::Pending::with_final(generation, self.jj_final.clone());
        let collector = pending.collector();
        self.jj_pending_load = Some(pending);
        Task::perform(
            async move {
                if !permit.admit() {
                    return collector;
                }
                if let Some(worker) = permit.start(move || jj_actions::load(&root, &cancel)) {
                    let _ = worker.await;
                }
                collector
            },
            Message::JjLoadCollected,
        )
    }

    fn run_jj_action(&mut self, action: lume_ui::tool::JjAction) -> Task<Message> {
        if self.search_exit.is_some() {
            return self.notify_error(
                "Chiusura in corso: ripeti l’azione Jujutsu dopo averla annullata".into(),
            );
        }
        self.reconcile_jj_cold_load();
        self.reconcile_jj_cold_action();
        let Some(root) = self.jj_workspace.clone() else {
            return Task::none();
        };
        if self.albero_eliminazione_in_corso() {
            return self.notify_error(
                "Spostamento nel Cestino in corso: attendi prima di avviare Jujutsu".into(),
            );
        }
        if self.jj_busy || self.jj_loading {
            return self.notify(
                NoticeKind::Info,
                "Un'altra operazione Jujutsu è ancora in corso.".to_owned(),
            );
        }
        self.jj_busy = true;
        self.jj_gen += 1;
        let generation = self.jj_gen;
        self.jj_action_generation = Some(generation);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.jj_cancel = cancel.clone().into();
        let done = action.clone();
        let (pending, permit) =
            jj_actions::admission::Pending::with_final(action.clone(), self.jj_final.clone());
        self.jj_pending_action = Some(pending);
        Task::perform(
            async move {
                if !permit.admit() {
                    return Err("Azione Jujutsu non avviata: ammissione revocata".to_owned());
                }
                let Some(execution) =
                    permit.start(move || jj_actions::execute(&root, &action, &cancel))
                else {
                    return Err("Azione Jujutsu non avviata: invocazione non disponibile".into());
                };
                execution.await
            },
            move |result| Message::JjActionDone(generation, done.clone(), result),
        )
    }

    /// Il thread della palette nasce alla prima ricerca. Un errore d'avvio
    /// resta finché la palette è visibile: ritentare a ogni battuta
    /// vorrebbe dire provare a creare un thread per tasto.
    fn ensure_file_palette_worker(&mut self) {
        if self.global_file_search_worker.is_some()
            || self.global_file_search_worker_error.is_some()
        {
            return;
        }
        match search::GlobalFileSearchWorker::start() {
            Ok(worker) => self.global_file_search_worker = Some(worker),
            Err(error) => self.global_file_search_worker_error = Some(error.to_string()),
        }
    }

    /// Modello chiuso o palette non visibile → il thread va via (il `Drop`
    /// fa shutdown e un `join` breve: `file_entries` controlla ogni voce).
    /// Riconciliato anche nella sostituzione e in `drain_tool_requests`.
    fn reconcile_file_palette(&mut self) {
        if !self.global_search.is_open() || !self.shell.mostra(OverlayId::Search) {
            self.global_file_search_worker = None;
            self.global_file_search_worker_error = None;
        }
    }

    fn replace_mapping(&self) -> Vec<(DocId, PathBuf)> {
        let mut mapping: Vec<_> = self
            .workspace
            .ids()
            .filter_map(|id| {
                self.workspace
                    .get(id)
                    .and_then(lume_core::Document::path)
                    .map(|p| (id, p.to_path_buf()))
            })
            .collect();
        mapping.sort_by_key(|(id, _)| id.0);
        mapping
    }

    fn replacement_valid(&self) -> bool {
        self.replacement.as_ref().is_some_and(|s| {
            self.scritture
                .possiede(s.lease, crate::scritture::Tipo::Sostituisci)
                && s.root == self.project_root
                && self.replace_root_epoch == self.git_root_epoch
                && s.mapping == self.replace_mapping()
                && s.opened.iter().all(|o| {
                    self.workspace.get(o.snapshot.id()).is_some_and(|d| {
                        d.path() == Some(o.path.as_path()) && d.matches_snapshot(&o.snapshot)
                    })
                })
        })
    }

    fn replace_event(event: crate::replace_batch::Event) -> Message {
        match event {
            crate::replace_batch::Event::Ready(ready) => Message::ReplaceReady(ready),
            crate::replace_batch::Event::Done(receipt) => Message::ReplaceCollected(receipt),
        }
    }

    fn replace_prepared(&mut self, ready: crate::replace_batch::Ready) -> Task<Message> {
        use crate::replace_batch::Phase;
        let Some(job) = self.replace_job.as_ref() else {
            return Task::none();
        };
        if !job.matches_ready(&ready) {
            return Task::none(); // Foreign/replay non consumano il job corrente.
        }
        if !self.replacement.as_ref().is_some_and(|state| {
            state.id == job.id()
                && self
                    .scritture
                    .possiede(state.lease, crate::scritture::Tipo::Sostituisci)
        }) {
            ready.deny();
            job.revoke();
            return Task::none();
        }
        let valid = self.replacement_valid()
            && self.replacement.as_ref().is_some_and(|state| {
                state.id == job.id()
                    && state.phase == Phase::Preparing
                    && !state.cancel.load(std::sync::atomic::Ordering::Acquire)
            });
        if valid && ready.grant() {
            self.replace_permission = Some(ready);
            if let Some(state) = self.replacement.as_mut() {
                state.phase = Phase::Committing;
            }
        } else {
            ready.deny();
            job.revoke();
            if let Some(state) = self.replacement.as_mut() {
                state.phase = Phase::Aborting;
            }
        }
        // Sola raccolta della stessa invocazione già ammessa, nessun nuovo
        // worker. Serve anche se il producer originale è stato perso.
        Task::run(crate::replace_batch::completed(job), Self::replace_event)
    }

    fn replace_committed(&mut self, receipt: crate::replace_batch::Receipt) -> Task<Message> {
        use crate::replace_batch::Phase;
        if !self.replace_job.as_ref().is_some_and(|job| {
            job.matches(&receipt)
                && self.replacement.as_ref().is_some_and(|state| {
                    state.id == job.id()
                        && self
                            .scritture
                            .possiede(state.lease, crate::scritture::Tipo::Sostituisci)
                })
        }) {
            return Task::none();
        }
        let Some(mut outcome) = receipt.take() else {
            return Task::none();
        };
        let valid = self.replacement_valid();
        if let Some(state) = self.replacement.as_ref() {
            self.scritture.avanzamento_sostituzione(state.lease);
        }
        self.replace_permission = None;
        self.replace_job = None;
        let Some(mut state) = self.replacement.take() else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        for path in &outcome.report.committed {
            // Una scheda aperta durante il commit non va pulita o riscritta:
            // il buffer nuovo/dirty resta intatto; il clean si rilegge.
            if let Some(doc) = self.workspace.get_by_path(path)
                && !self.chiede_salvataggio(doc)
            {
                state.reloads.push((doc, path.clone()));
            }
        }
        if valid && outcome.report.ok() && !state.cancel.load(std::sync::atomic::Ordering::Acquire)
        {
            for open in outcome.opened {
                let doc_id = open.input.snapshot.id();
                if let Some(doc) = self.workspace.get_mut(doc_id) {
                    doc.apply(lume_core::Transaction {
                        changes: open.changes,
                        selection_after: None,
                    });
                }
                self.rebuild_syntax(doc_id, &open.input.path);
                self.lsp_flush_change(doc_id, None);
                if open.input.clean {
                    state.autosaves.insert(doc_id, open.input.path);
                    if let Some(doc) = self.workspace.get(doc_id) {
                        self.replace_versions.insert(doc_id, doc.version());
                    }
                } else {
                    state.dirty += 1;
                }
            }
        } else {
            if outcome.report.ok() {
                outcome.report.error = Some("buffer o progetto cambiati durante i commit: buffer intatti, nessun rollback dei file scritti".into());
            }
            outcome
                .report
                .not_attempted
                .extend(outcome.opened.into_iter().map(|o| o.input.path));
        }
        state.phase = Phase::Saving;
        state.report = outcome.report;
        state.opened.clear();
        self.replacement = Some(state);
        tasks.push(self.start_replace_save());
        Task::batch(tasks)
    }

    fn finish_replace_report(&mut self) -> Task<Message> {
        use crate::replace_batch::Phase;
        if !self.replacement.as_ref().is_some_and(|s| {
            s.phase == Phase::Saving
                && s.autosaves.is_empty()
                && self
                    .scritture
                    .possiede(s.lease, crate::scritture::Tipo::Sostituisci)
        }) {
            return Task::none();
        }
        let Some(state) = self.replacement.take() else {
            return Task::none();
        };
        self.replace_versions.clear();
        let esito = if state.report.ok() {
            crate::scritture::Esito::Riuscito
        } else {
            crate::scritture::Esito::Fallito
        };
        self.scritture.rilascia(state.lease, esito);
        if let Some(tool) = self.registry.get_mut(tools::search_tool::ID) {
            let requests = tool.update(&ToolEvent::new(tools::search_tool::Event::Replaced));
            self.pending_tool_requests
                .extend(requests.into_iter().map(|r| (tools::search_tool::ID, r)));
        }
        let notice = if let Some(warning) = state
            .report
            .ok()
            .then(|| state.report.sync_warning())
            .flatten()
        {
            self.notify(NoticeKind::Error, warning)
        } else if state.report.ok() {
            let tail = if state.dirty > 0 {
                format!(" — {} già modificati restano da salvare", state.dirty)
            } else {
                String::new()
            };
            self.notify(
                NoticeKind::Info,
                format!(
                    "Sostituite {} occorrenze in {} file{tail}",
                    state.occurrences, state.files
                ),
            )
        } else {
            let mut text = state.report.describe();
            if let Some(warning) = state.report.sync_warning() {
                text = format!("{text}. {warning}");
            }
            self.save_failed(text)
        };
        let mut tasks = vec![notice, self.drain_tool_requests()];
        for (id, path) in state.reloads {
            tasks.push(self.reload_doc(id, path));
        }
        Task::batch(tasks)
    }

    /// F11: aggiunge o toglie il segnalibro sulla riga del caret del
    /// documento attivo (solo file su disco), con notifica e persistenza.
    fn toggle_bookmark(&mut self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::ToggleBookmark;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        self.toggle_bookmark_for(id)
    }

    fn push_history(
        &mut self,
        action: lume_store::HistoryAction,
        label: String,
        path: Option<PathBuf>,
    ) {
        tools::history::push(
            &mut self.history,
            lume_store::HistoryRecord {
                at: tools::history::now_unix(),
                action,
                label,
                path,
            },
        );
        self.sync_history_tool();
    }

    /// Spinge le righe al pannello Estensioni. Il tool non può leggere
    /// `Lume`, quindi la fotografia la fa l'app — ed è l'unico posto in cui
    /// si decide **cosa** si elenca: solo le funzionalità cablate.
    /// La fotografia: una riga per funzionalità **cablata**, con i numeri
    /// presi da chi possiede davvero le risorse. Separata dalla spinta perché
    /// è la parte su cui si può asserire — il `Registry` dà solo
    /// `&dyn Tool`, e quel che è entrato nel pannello non si rilegge.
    fn extension_rows(&self) -> Vec<tools::ext::Row> {
        crate::features::FEATURES
            .iter()
            .filter(|spec| spec.wired)
            .map(|spec| {
                let (detail_override, copies) = match spec.feature {
                    crate::features::Feature::LanguageServer => (
                        crate::lsp_mode::lsp_detail_line(self.lsp_active()),
                        Vec::new(),
                    ),
                    crate::features::Feature::Mcp => {
                        (Some(self.mcp_detail_text()), self.mcp_copies())
                    }
                    _ => (None, Vec::new()),
                };
                let start_label = (spec.feature == crate::features::Feature::LanguageServer)
                    .then(|| crate::lsp_mode::lsp_start_label(self.lsp_active()))
                    .flatten();
                tools::ext::Row {
                    feature: spec.feature,
                    enabled: self.feature_is_enabled(spec.feature),
                    live: self.feature_live_count(spec.feature),
                    confirming: self
                        .feature_stop_pending
                        .is_some_and(|pending| pending.feature() == spec.feature),
                    startup_ms: self.feature_startup_ms(spec.feature),
                    can_start: self.feature_can_start(spec.feature),
                    detail_override,
                    start_label,
                    copies,
                }
            })
            .collect()
    }

    fn language_server_section(&self) -> Option<tools::ext::LanguageServerSection> {
        let spec = self.lsp_active().map(|lsp| lsp.spec).or_else(|| {
            crate::lsp_mode::server_for(self.project_root.as_deref(), self.active_path())
        })?;
        let (detail, can_download, downloading) = match self.lsp_active() {
            Some(lsp) => (
                crate::lsp_mode::lsp_detail_line(Some(lsp)).unwrap_or_default(),
                crate::lsp_mode::lsp_start_action(Some(lsp))
                    == crate::lsp_mode::LspStartAction::Scarica,
                matches!(lsp.state, crate::lsp_mode::LspState::Scaricamento { .. }),
            ),
            None => ("non avviata".to_owned(), false, false),
        };
        Some(tools::ext::LanguageServerSection {
            name: spec.name.clone(),
            version: spec.release.version.clone(),
            detail,
            can_download,
            downloading,
        })
    }

    /// Spinge la fotografia al pannello. `true` se l’ha fatta davvero.
    ///
    /// Solo a pannello aperto: `drain_tool_requests` gira a **ogni**
    /// messaggio, compresi i `DragMoved` del mouse e il tick del terminale, e
    /// una fotografia per frame di un pannello che nessuno guarda sarebbe
    /// esattamente lo spreco che il budget vieta. La prima all’apertura
    /// arriva comunque, perché aprire un pannello passa sempre dalla coda.
    fn sync_extensions_tool(&mut self) -> bool {
        if !self.registry.is_open(tools::ext::ID) {
            return false;
        }
        let rows = self.extension_rows();
        let language_server = self.language_server_section();
        if let Some(tool) = self.registry.get_mut(tools::ext::ID) {
            tool.update(&ToolEvent::new(tools::ext::Event::Replace {
                rows,
                language_server,
            }));
        }
        true
    }

    fn sync_bookmarks_tool(&mut self) {
        if let Some(tool) = self.registry.get_mut(tools::bookmarks::ID) {
            tool.update(&ToolEvent::new(tools::bookmarks::Event::Replace(
                self.bookmarks.clone(),
            )));
        }
    }

    fn sync_history_tool(&mut self) {
        if let Some(tool) = self.registry.get_mut(tools::history::ID) {
            tool.update(&ToolEvent::new(tools::history::Event::Replace(
                self.history.clone(),
            )));
        }
    }

    /// Chiude lo stato del timer quando la richiesta non arriva a un Job.
    fn termina_autosave_non_avviato(&mut self, id: DocId, errore: Option<&str>) -> bool {
        let pulito = errore.is_none() && self.workspace.get(id).is_some_and(|doc| !doc.is_dirty());
        let Some(stato) = self
            .notes
            .as_mut()
            .and_then(|notes| notes.salvataggi.get_mut(&id))
        else {
            return false;
        };
        if !matches!(stato, crate::autosave::StatoSalvataggio::InCorso { .. }) {
            return false;
        }
        if pulito {
            *stato = crate::autosave::StatoSalvataggio::Pulito;
            false
        } else {
            stato.fallito(
                errore
                    .unwrap_or("Nessuna scrittura automatica disponibile")
                    .to_owned(),
            );
            true
        }
    }

    /// Traduce soltanto un'ammissione del coordinatore in Job e Task.
    fn avvia_save_autorizzato(
        &mut self,
        lease: crate::scritture::Lease,
        op: crate::scritture::Op,
        rimandato: bool,
    ) -> Task<Message> {
        use crate::scritture::{Causa, Esito, Op, Tipo};
        let Op::Salva {
            doc: id,
            mut versione,
            causa,
        } = op
        else {
            self.scritture.rilascia(lease, Esito::NonAvviato);
            return self.notify_error("Scrittura non avviata: richiesta non valida".into());
        };
        if !self.scritture.possiede(lease, Tipo::Salvataggio) {
            return Task::none();
        }
        let origin = if self
            .save_active_origin
            .as_ref()
            .is_some_and(|(l, _)| *l == lease)
        {
            self.save_active_origin.take().map(|(_, origin)| origin)
        } else {
            self.save_origins.remove(&id)
        };
        let current = origin.as_ref().is_some_and(|origin| {
            origin.epoch == self.git_root_epoch
                && self
                    .workspace
                    .get(id)
                    .is_some_and(|doc| doc.path() == origin.path.as_deref())
                && self.notes.as_ref().and_then(|n| {
                    n.tabs
                        .get(&id)
                        .map(|note| (n.scan_generation, note.clone()))
                }) == origin.note
        });
        let result = if !current {
            Err(
                "Salvataggio accodato scaduto: progetto o binding cambiati; salva di nuovo"
                    .to_owned(),
            )
        } else {
            if causa == Causa::Automatico
                && let Some(doc) = self.workspace.get(id)
            {
                versione = doc.version();
            }
            self.prepare_save(id, versione)
        };
        match result {
            Ok(Some(request)) => {
                // Un save utente può assorbire il rimandato della stessa
                // nota: il suo Job diventa responsabile dell'autosave.
                if let Some(stato) = self.notes.as_mut().and_then(|n| n.salvataggi.get_mut(&id))
                    && matches!(stato, crate::autosave::StatoSalvataggio::InCorso { .. })
                {
                    *stato = crate::autosave::StatoSalvataggio::InCorso {
                        versione: request.snapshot.version(),
                    };
                }
                self.save_causa = Some((request.ticket, causa));
                let job = crate::save::Job::new(request);
                let delivery = job.delivery();
                self.save_job = Some(job);
                self.save_lease = Some(lease);
                self.save_job_root_epoch = Some(self.git_root_epoch);
                let notice = if rimandato {
                    if let Some((_, notice)) = self
                        .notice
                        .as_mut()
                        .filter(|(_, notice)| notice.kind == NoticeKind::Error)
                    {
                        let suffisso = " — Salvataggio automatico ripartito";
                        if !notice.text.ends_with(suffisso) {
                            notice.text.push_str(suffisso);
                        }
                        Task::none()
                    } else {
                        self.notify(NoticeKind::Info, "Salvataggio automatico ripartito".into())
                    }
                } else {
                    Task::none()
                };
                Task::batch([notice, Task::run(delivery, Self::save_event)])
            }
            Ok(None) => {
                self.scritture.rilascia(lease, Esito::NonAvviato);
                if self.termina_autosave_non_avviato(id, None) {
                    self.save_failed("Nessuna scrittura automatica disponibile".into())
                } else if causa == Causa::Automatico {
                    self.notify(
                        NoticeKind::Info,
                        "Salvataggio automatico non avviato: documento pulito".into(),
                    )
                } else {
                    Task::none()
                }
            }
            Err(error) => {
                self.scritture.rilascia(lease, Esito::NonAvviato);
                if causa == Causa::Automatico && self.external_conflicts.contains(&id) {
                    if let Some(stato) = self
                        .notes
                        .as_mut()
                        .and_then(|notes| notes.salvataggi.get_mut(&id))
                    {
                        stato.in_conflitto();
                    }
                } else {
                    self.termina_autosave_non_avviato(id, Some(&error));
                }
                self.save_failed(error)
            }
        }
    }

    /// La sequenza dei save interni appartiene al batch, non alla coda generale.
    fn start_replace_save(&mut self) -> Task<Message> {
        use crate::scritture::{Causa, Op, Permesso, Tipo};
        if self.saves.active_document().is_some() || self.save_job.is_some() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        while let Some((lease, id, path, annullata)) = self.replacement.as_ref().and_then(|state| {
            (!state.blocks_saves())
                .then(|| {
                    state
                        .autosaves
                        .iter()
                        .min_by_key(|(id, _)| id.0)
                        .map(|(&id, path)| {
                            (
                                state.lease,
                                id,
                                path.clone(),
                                state.cancel.load(std::sync::atomic::Ordering::Acquire),
                            )
                        })
                })
                .flatten()
        }) {
            let version = self.replace_versions.remove(&id);
            let autorizzato = version.is_some_and(|versione| {
                matches!(
                    self.scritture.ammetti(Op::Salva {
                        doc: id,
                        versione,
                        causa: Causa::DiSostituzione(lease)
                    }),
                    Ok(Permesso::Continuazione)
                )
            });
            let current = self.scritture.possiede(lease, Tipo::Sostituisci)
                && self.replace_root_epoch == self.git_root_epoch
                && self
                    .workspace
                    .get(id)
                    .is_some_and(|doc| doc.path() == Some(path.as_path()));
            let result = if annullata || !autorizzato || !current {
                Err("Salvataggio della sostituzione non avviato: operazione revocata o non autorizzata".to_owned())
            } else if let Some(version) = version {
                self.prepare_save(id, version)
            } else {
                Ok(None)
            };
            match result {
                Ok(Some(request)) => {
                    let job = crate::save::Job::new(request);
                    let delivery = job.delivery();
                    self.save_job = Some(job);
                    self.save_job_root_epoch = Some(self.git_root_epoch);
                    tasks.push(Task::run(delivery, Self::save_event));
                    return Task::batch(tasks);
                }
                Ok(None) => {
                    let error =
                        "Salvataggio della sostituzione non avviato: richiesta non disponibile"
                            .to_owned();
                    self.replace_save_not_started(id, error.clone());
                    self.scritture.revoca_salvataggi(Some(id));
                    self.save_origins.remove(&id);
                    self.termina_autosave_non_avviato(id, Some(&error));
                    tasks.push(self.save_failed(error));
                }
                Err(error) => {
                    self.replace_save_not_started(id, error.clone());
                    self.scritture.revoca_salvataggi(Some(id));
                    self.save_origins.remove(&id);
                    self.termina_autosave_non_avviato(id, Some(&error));
                    tasks.push(self.save_failed(error));
                }
            }
        }
        tasks.push(self.finish_replace_report());
        Task::batch(tasks)
    }

    fn apply_menu_intent(&mut self, intent: MenuIntent) -> Task<Message> {
        if self.albero_eliminazione_in_corso()
            && matches!(
                intent,
                MenuIntent::Activate(
                    MenuAct::NewTreeFile
                        | MenuAct::NewTreeFolder
                        | MenuAct::RenameTreeFile
                        | MenuAct::LinkToNewNote
                )
            )
        {
            return self.notify_error(
                "Spostamento nel Cestino in corso: attendi prima di modificare i file".into(),
            );
        }
        // Il soggetto del menù: si legge **prima** che il menù si chiuda,
        // perché le voci dell'albero e delle schede agiscono su di esso.
        let soggetto = self.context_menu.as_ref().map(|menu| menu.soggetto.clone());
        let selezione_menu = soggetto.as_ref().and_then(|soggetto| soggetto.selezione());
        // Le operazioni LSP usano il punto cliccato; le operazioni clipboard
        // conservano la selezione dentro cui si è aperto il menu.
        if matches!(
            intent,
            MenuIntent::Activate(MenuAct::GotoDefinition | MenuAct::FindUsages | MenuAct::Rename)
        ) && let Some(pos) = soggetto.as_ref().and_then(|soggetto| soggetto.pos())
            && let Some(id) = self.active_doc()
            && let Some(document) = self.workspace.get_mut(id)
        {
            set_primary(document, Range::point(pos.min(document.rope().len_chars())));
        }
        // scegliere una voce chiude il menù contestuale: vale per tutte e
        // tre, e per l'Esc ci pensa `CloseAll`
        if self.context_menu.is_some() {
            self.context_menu = None;
            self.chiudi_overlay_se(OverlayId::Context);
        }
        match intent {
            MenuIntent::None | MenuIntent::Closed => Task::none(),
            MenuIntent::Activate(MenuAct::OpenFolder) => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .pick_folder()
                        .await
                        .map(|handle| handle.path().to_path_buf())
                },
                Message::FolderPicked,
            ),
            MenuIntent::Activate(MenuAct::SaveAll) => {
                Task::batch([self.save_all(), self.persist_session()])
            }
            MenuIntent::Activate(MenuAct::SearchFile) => self.open_search(SearchTab::File),
            MenuIntent::Activate(MenuAct::FindInFiles) => {
                // ATTENZIONE: Non un `click`, che è un interruttore: Ctrl+Maiusc+F
                // premuto due volte chiuderebbe il pannello che si è appena
                // aperto per cercare. Apre se serve, e in ogni caso porta il
                // cursore nel campo.
                if self.registry.active_panel != Some(tools::search_tool::ID) {
                    let requests = self.registry.click(tools::search_tool::ID);
                    self.pending_tool_requests.extend(requests);
                }
                Task::batch([
                    iced::widget::operation::focus(iced::widget::Id::new(
                        tools::search_tool::INPUT_ID,
                    )),
                    self.drain_tool_requests(),
                ])
            }
            MenuIntent::Activate(MenuAct::ToggleBookmark) => self.toggle_bookmark(),
            MenuIntent::Activate(MenuAct::Save) => self.save_doc_active(),
            MenuIntent::Activate(MenuAct::SaveAs) => self.save_as_active(),
            MenuIntent::Activate(MenuAct::NewFile) => match self.new_untitled() {
                Ok(_) => {
                    // MenuAction ritorna prima della coda comune di update:
                    // riconcilia anche Struttura con il nuovo documento.
                    self.drain_tool_requests()
                }
                Err(error) => self.notify_error(error),
            },
            MenuIntent::Activate(MenuAct::Exit) => self.chiudi_finestra(None),
            MenuIntent::Activate(MenuAct::ToggleTerminal) => {
                self.show_dock(tools::terminal::ID, true)
            }
            MenuIntent::Activate(MenuAct::ToggleEditorBlame) => {
                if self
                    .groups
                    .active()
                    .is_some_and(|id| self.pending_restored_tabs.contains(id))
                {
                    return Task::none();
                }
                if !self.editor_blame_available {
                    return Task::none();
                }
                self.editor_blame_enabled = !self.editor_blame_enabled;
                if !self.editor_blame_enabled {
                    self.editor_blame_cache.clear();
                    self.editor_blame_generation.clear();
                    return Task::none();
                }
                if let Some(id) = self.groups.active()
                    && let Some(path) = self
                        .workspace
                        .get(id)
                        .and_then(|doc| doc.path().map(Path::to_path_buf))
                {
                    self.pending_tool_requests
                        .push((tools::git_native::ID, ToolRequest::GitBlameFile(path)));
                }
                self.drain_tool_requests()
            }
            MenuIntent::Activate(MenuAct::ShowGitLog) => self.show_dock(tools::gitlog::ID, false),
            MenuIntent::Activate(MenuAct::ShowProblems) => {
                self.show_dock(tools::problems::ID, false)
            }
            MenuIntent::Activate(MenuAct::GotoDefinition) => self.goto_definition(),
            MenuIntent::Activate(MenuAct::FindUsages) => self.find_usages(),
            MenuIntent::Activate(MenuAct::Rename) => self.start_rename(),
            MenuIntent::Activate(MenuAct::FormatDocument) => self.format_document(),
            MenuIntent::Activate(MenuAct::FormatSelection) => self.format_selection(),
            MenuIntent::Activate(MenuAct::CodeActions) => self.code_actions(),
            MenuIntent::Activate(MenuAct::LinkToOpenNote) => {
                self.collega_a_nota_aperta(selezione_menu)
            }
            MenuIntent::Activate(MenuAct::LinkToNewNote) => {
                self.collega_a_nota_nuova(selezione_menu)
            }
            MenuIntent::Activate(MenuAct::NavBack) => self.navigate(-1),
            MenuIntent::Activate(MenuAct::NavForward) => self.navigate(1),
            MenuIntent::Activate(MenuAct::Recents) => self.open_recents(),
            MenuIntent::Activate(MenuAct::GotoLine) => self.open_goto_line(),
            MenuIntent::Activate(MenuAct::Shortcuts) => self.open_shortcuts(),
            MenuIntent::Activate(MenuAct::CheckUpdates) => self.check_updates(),
            MenuIntent::Activate(MenuAct::About) => {
                self.apri_overlay(OverlayId::About);
                Task::none()
            }
            MenuIntent::Activate(MenuAct::ShowStructure) => {
                let requests = self.registry.click(tools::structure::ID);
                self.pending_tool_requests.extend(requests);
                Task::none()
            }
            MenuIntent::Activate(MenuAct::ShowExtensions) => {
                let requests = self.registry.click(tools::ext::ID);
                self.pending_tool_requests.extend(requests);
                Task::none()
            }
            MenuIntent::Activate(MenuAct::SplitEditor) => {
                self.request_editor_focus();
                // `split` sposta l'attiva del gruppo col fuoco nell'altro: se
                // quella è la console e il fuoco è a sinistra, finirebbe a
                // destra, dove non sa mostrarsi
                let spostabile = self
                    .groups
                    .active()
                    .is_some_and(|id| self.si_puo_dividere(id));
                if spostabile {
                    self.groups.split();
                    self.syntax_reconcile_groups();
                }
                self.persist_session()
            }
            MenuIntent::Activate(MenuAct::UnsplitEditor) => {
                self.request_editor_focus();
                self.groups.unsplit();
                self.syntax_reconcile_groups();
                self.persist_session()
            }
            MenuIntent::Activate(MenuAct::Undo) => {
                let command = self.apply_editor_command(EditorCommand::Undo);
                Task::batch([command, self.lsp_change_task()])
            }
            MenuIntent::Activate(MenuAct::Redo) => {
                let command = self.apply_editor_command(EditorCommand::Redo);
                Task::batch([command, self.lsp_change_task()])
            }
            MenuIntent::Activate(MenuAct::AddNextOccurrence) => {
                self.request_editor_focus();
                self.apply_editor_command(EditorCommand::AddNextOccurrence)
            }
            MenuIntent::Activate(MenuAct::AddCursorAbove) => {
                self.request_editor_focus();
                self.apply_editor_command(EditorCommand::AddCursorLine { above: true })
            }
            MenuIntent::Activate(MenuAct::AddCursorBelow) => {
                self.request_editor_focus();
                self.apply_editor_command(EditorCommand::AddCursorLine { above: false })
            }
            MenuIntent::Activate(MenuAct::Copy) => self.copy_selection_to_clipboard(),
            MenuIntent::Activate(MenuAct::Cut) => self.cut_selection_to_clipboard(),
            MenuIntent::Activate(MenuAct::Paste) => {
                iced::clipboard::read().map(Message::ClipboardPasted)
            }
            MenuIntent::Activate(MenuAct::Find) => self.open_find(false),
            MenuIntent::Activate(MenuAct::Replace) => self.open_find(true),
            MenuIntent::Activate(MenuAct::Complete) => self.request_completion_from_menu(),
            MenuIntent::Activate(MenuAct::QuickDoc) => self.hover_at_cursor(),
            MenuIntent::Activate(MenuAct::SignatureHelp) => self.signature_help_at_cursor(),
            MenuIntent::Activate(MenuAct::Forge) => self.open_forge(),
            MenuIntent::Activate(MenuAct::CloseTab) => {
                if let Some(crate::context_menu::Soggetto::Scheda { id, .. }) = soggetto.as_ref() {
                    self.chiudi_scheda(*id);
                }
                Task::none()
            }
            MenuIntent::Activate(MenuAct::CloseOtherTabs) => {
                if let Some(crate::context_menu::Soggetto::Scheda { id, .. }) = soggetto.as_ref() {
                    return self.chiudi_altre_schede(*id);
                }
                Task::none()
            }
            MenuIntent::Activate(MenuAct::CopyPath) => {
                let percorso = soggetto
                    .as_ref()
                    .and_then(|soggetto| soggetto.path())
                    .map(|path| path.to_string_lossy().to_string());
                match percorso {
                    Some(testo) => Task::batch([
                        iced::clipboard::write(testo),
                        self.notify(
                            NoticeKind::Info,
                            "Percorso copiato negli appunti".to_owned(),
                        ),
                    ]),
                    None => Task::none(),
                }
            }
            MenuIntent::Activate(MenuAct::OpenPath) => {
                let percorso = soggetto
                    .as_ref()
                    .and_then(|soggetto| soggetto.file_da_aprire())
                    .map(|path| path.to_path_buf());
                let richiesto = percorso.clone();
                match percorso.and_then(|path| self.resolve_path(&path)) {
                    Some(percorso) => {
                        self.open_file_task(percorso, crate::file_read::DopoApertura::Niente)
                    }
                    // ATTENZIONE: nessun silenzio. Un percorso che non si
                    // risolve (file sparito, fuori dal progetto) è un'azione
                    // rifiutata: si dice in barra di stato, perché il menù
                    // si chiude e l'utente deve sapere perché non è successo
                    // niente.
                    None => {
                        let testo = match richiesto {
                            Some(percorso) => format!(
                                "Non si apre {}: il file non c'è più o non è del progetto",
                                percorso.display()
                            ),
                            None => "Non c'è un file da aprire".to_owned(),
                        };
                        self.notify_error(testo)
                    }
                }
            }
            MenuIntent::Activate(MenuAct::NewTreeFile) => {
                self.apri_albero_azione(crate::app::albero::Azione::NuovoFile, soggetto.as_ref())
            }
            MenuIntent::Activate(MenuAct::NewTreeFolder) => self
                .apri_albero_azione(crate::app::albero::Azione::NuovaCartella, soggetto.as_ref()),
            MenuIntent::Activate(MenuAct::DeleteTreeFile) => {
                self.apri_elimina_albero(soggetto.as_ref())
            }
            MenuIntent::Activate(MenuAct::RenameTreeFile) => {
                self.apri_rinomina_albero(soggetto.as_ref())
            }
            MenuIntent::Activate(MenuAct::FoldToggle) => self.fold_toggle(),
            MenuIntent::Activate(MenuAct::UnfoldAll) => self.unfold_all(),
            MenuIntent::Activate(MenuAct::MenuBar | MenuAct::None) => Task::none(),
        }
    }

    /// Dock via menù/scorciatoia: `toggle` = come il click sull'icona
    /// (apre/chiude), altrimenti apre solo se non è già mostrato.
    fn show_dock(&mut self, id: ToolId, toggle: bool) -> Task<Message> {
        if toggle || !self.registry.is_open(id) {
            self.apply_activity_intent(ActivityBarIntent::Click(id));
        }
        Task::batch([self.drain_tool_requests(), self.persist_session()])
    }

    fn open_forge(&mut self) -> Task<Message> {
        if self.forge_config.is_none() {
            self.forge_config = Some(crate::forge::RunConfig::blank(crate::forge::Family::Rust));
        }
        self.apri_overlay(OverlayId::Forge);
        iced::widget::operation::focus(iced::widget::Id::new(crate::forge_panel::TARGET_ID))
    }

    fn close_forge(&mut self) -> Task<Message> {
        self.chiudi_overlay_se(OverlayId::Forge);
        Task::none()
    }

    pub(crate) fn forge_can_run(&self) -> bool {
        self.feature_is_enabled(crate::features::Feature::Terminal)
    }

    fn apply_forge_message(&mut self, message: crate::forge_panel::ForgeMessage) -> Task<Message> {
        use crate::forge_panel::ForgeMessage;
        match message {
            ForgeMessage::Close => self.close_forge(),
            ForgeMessage::Run => self.run_forge(),
            other => {
                if let Some(config) = &mut self.forge_config {
                    crate::forge_panel::edit(config, other);
                }
                Task::none()
            }
        }
    }

    fn run_forge(&mut self) -> Task<Message> {
        if !self.forge_can_run() {
            return Task::none();
        }
        let Some(input) = self
            .forge_config
            .as_ref()
            .filter(|c| c.is_runnable())
            .and_then(crate::forge::RunConfig::terminal_input)
        else {
            return Task::none();
        };
        let needs_session = self
            .feature_live_count(crate::features::Feature::Terminal)
            .unwrap_or(0)
            == 0;
        self.chiudi_overlay_se(OverlayId::Forge);
        let dock = self.show_dock(tools::terminal::ID, false);
        if let Some(tool) = self.registry.get_mut(tools::terminal::ID) {
            if needs_session {
                let _ = tool.update(&ToolEvent::new(tools::terminal::Event::NewSession));
            }
            let _ = tool.update(&ToolEvent::new(tools::terminal::Event::Input(input)));
        }
        Task::batch([dock, self.drain_tool_requests()])
    }

    /// Spunte dei menù calcolate dallo stato corrente.
    pub(crate) fn menu_checks(&self) -> MenuChecks<'_> {
        let (can_link_open_note, can_link_new_note) = self.note_link_menu();
        MenuChecks {
            terminal_open: self.registry.is_open(tools::terminal::ID),
            disabled_features: self.registry.disabled_features(),
            lsp_definition: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_define),
            lsp_references: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_find_usages),
            lsp_formatting: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_format),
            lsp_range_formatting: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_format_range),
            lsp_code_actions: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_code_action),
            // «Espandi tutti» deve restare raggiungibile finché c'è un blocco
            // compresso, anche se lo snapshot sintattico è in aggiornamento.
            folding_available: self.active_doc().is_some_and(|id| {
                self.folded.get(&id).is_some_and(|c| !c.is_empty())
                    || !self.regioni_fold(id).is_empty()
            }),
            lsp_rename: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_rename),
            can_link_open_note,
            can_link_new_note,
            lsp_complete: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_complete),
            lsp_hover: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_hover),
            lsp_signature_help: self
                .lsp_active()
                .is_some_and(crate::lsp_mode::LspWorkspace::can_signature_help),
            can_save_as: self
                .active_doc()
                .is_some_and(|id| self.puo_salvare_con_nome(id)),
            git_blame_available: self.editor_blame_available,
            editor_blame_enabled: self.editor_blame_enabled,
        }
    }
}

/// Il countdown verso il freddo di una funzionalità Dormiente. La chiave è
/// `(funzionalità, epoca, durata)` e **non l'istante**: contiene già tutto ciò
/// che identifica questa dormienza, resta stabile mentre l'utente continua a
/// lavorare altrove (altrimenti iced ricostruirebbe lo stream a ogni battuta)
/// e cambia a ogni nuova chiusura. Riaprendo, la tabella non la elenca più,
/// il ramo non viene più prodotto e lo sleep muore con lui.
///
/// Sono i due idiomi insieme: la subscription esiste solo nello stato
/// Dormiente, e il timer è una risorsa posseduta dallo stream di
/// `iced::stream::channel` costruito sotto `Subscription::run_with`.
fn idle_subscription(
    feature: crate::features::Feature,
    epoch: u64,
    timeout: std::time::Duration,
) -> Subscription<Message> {
    Subscription::run_with((feature, epoch, timeout), |key| {
        let (feature, epoch, timeout) = *key;
        iced::stream::channel(
            1,
            move |mut out: futures::channel::mpsc::Sender<Message>| async move {
                use futures::SinkExt;
                tokio::time::sleep(timeout).await;
                let _ = out.send(Message::IdleElapsed(feature, epoch)).await;
            },
        )
    })
}

/// Lo stream che **possiede** l'attore, e quindi il processo del server.
///
/// ATTENZIONE: Il costruttore di `run_with` è una `fn` **senza catture**: riceve solo i
/// dati. Da qui la stretta di mano — il canale dei comandi lo crea lo stream,
/// e il **primo** messaggio che manda è la sua estremità. Prima di quello
/// l'app non ha nessuno a cui parlare, ed è giusto così.
fn lsp_subscription(spawn: crate::lsp_mode::LspSpawn) -> Subscription<Message> {
    Subscription::run_with(spawn, |spawn: &crate::lsp_mode::LspSpawn| {
        let epoch = spawn.epoch;
        let server = spawn.server_id.clone();
        let command = spawn.command.clone();
        let spec = (*spawn.spec).clone();
        iced::stream::channel(
            16,
            move |mut out: futures::channel::mpsc::Sender<Message>| async move {
                use futures::SinkExt;
                let (comandi, rx_comandi) = lume_lsp::LspCommandMailbox::channel();
                let (tx_eventi, mut eventi) = tokio::sync::mpsc::unbounded_channel();
                if out
                    .send(Message::Lsp(
                        server.clone(),
                        epoch,
                        crate::lsp_mode::LspEvent::Ready(comandi),
                    ))
                    .await
                    .is_err()
                {
                    return;
                }
                let attore = lume_lsp::actor::run(command, spec, rx_comandi, tx_eventi);
                let inoltro = async {
                    // finisce da sé quando l'attore lascia andare il suo capo
                    // del canale: nessuna corsa fra chi chiude e chi drena
                    while let Some(evento) = eventi.recv().await {
                        if out
                            .send(Message::Lsp(
                                server.clone(),
                                epoch,
                                crate::lsp_mode::LspEvent::From(evento),
                            ))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                };
                futures::join!(attore, inoltro);
                // ATTENZIONE: **Non finire mai.** Uno stream che termina mentre la sua
                // chiave è ancora prodotta viene rilanciato al frame dopo: un
                // ciclo di rinascite a spese della macchina dell'utente. Muore
                // quando l'app smette di produrlo, che è l'unica cosa che
                // sappia davvero se il server serve ancora.
                std::future::pending::<()>().await;
            },
        )
    })
}

/// Lo stream possiede server, connessioni, mailbox e file discovery.
#[cfg(feature = "mcp")]
fn mcp_subscription(spawn: crate::mcp_mode::McpSpawn) -> Subscription<Message> {
    Subscription::run_with(spawn, |spawn: &crate::mcp_mode::McpSpawn| {
        let spawn = spawn.clone();
        iced::stream::channel(
            16,
            move |mut out: futures::channel::mpsc::Sender<Message>| async move {
                use futures::SinkExt;
                let epoch = spawn.epoch;
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let server = async move {
                    match lume_mcp::Root::new(&spawn.root) {
                        Ok(root) => {
                            let config = lume_mcp::server::ServeConfig {
                                endpoint: spawn.endpoint,
                                token: spawn.token,
                                root,
                                server_version: env!("CARGO_PKG_VERSION").to_owned(),
                                availability: spawn.availability,
                                preamble_timeout: lume_mcp::server::PREAMBLE_TIMEOUT,
                                line_cap: lume_mcp::framing::LINE_CAP,
                                ask_timeout: lume_mcp::server::ASK_TIMEOUT,
                            };
                            lume_mcp::server::serve(config, tx).await;
                        }
                        Err(error) => {
                            let _ = tx.send(lume_mcp::McpEvent::Failed(format!(
                                "la radice non si legge: {error}"
                            )));
                        }
                    }
                };
                let inoltro = async {
                    while let Some(evento) = rx.recv().await {
                        if out.send(Message::Mcp(epoch, evento)).await.is_err() {
                            break;
                        }
                    }
                };
                futures::join!(server, inoltro);
                // Evita che una chiave ancora attiva rilanci uno stream finito.
                std::future::pending::<()>().await;
            },
        )
    })
}

/// Il ponte fra i risvegli PTY e l'app: un `Tick` per ogni permesso di
/// `Notify`. Niente thread e niente lock: droppare lo stream (dock chiuso)
/// droppa l'attesa, e una notifica già consegnata passa al ponte successivo
/// (tokio `Notify`, `drop_notified`).
async fn terminal_wakeups(
    wakeup: std::sync::Arc<tokio::sync::Notify>,
    mut out: futures::channel::mpsc::Sender<Message>,
) {
    use futures::SinkExt;
    loop {
        wakeup.notified().await;
        if out
            .send(Message::Tool(
                tools::terminal::ID,
                ToolEvent::new(tools::terminal::Event::Tick),
            ))
            .await
            .is_err()
        {
            // il ricevitore non c'è più: iced ha già droppato il ramo
            break;
        }
    }
}

fn terminal_subscription(wakeup: std::sync::Arc<tokio::sync::Notify>) -> Subscription<Message> {
    // `Arc<Notify>` non è `Hash`: la chiave è costante, c'è un solo ponte.
    #[derive(Clone)]
    struct TerminalWakeup(std::sync::Arc<tokio::sync::Notify>);
    impl std::hash::Hash for TerminalWakeup {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            "terminal-pty".hash(state);
        }
    }
    impl PartialEq for TerminalWakeup {
        fn eq(&self, _: &Self) -> bool {
            true
        }
    }
    impl Eq for TerminalWakeup {}

    Subscription::run_with(TerminalWakeup(wakeup), |TerminalWakeup(wakeup)| {
        let wakeup = wakeup.clone();
        iced::stream::channel(32, move |out: futures::channel::mpsc::Sender<Message>| {
            terminal_wakeups(wakeup, out)
        })
    })
}

/// Subscription del watcher FS: crea il watcher dentro lo stream (event-driven,
/// zero polling); un thread fa da ponte tra il canale std e quello async.
fn fs_subscription(scope: fs_bridge::WatchScope) -> Subscription<Message> {
    Subscription::run_with(scope, |scope: &fs_bridge::WatchScope| {
        let directories = scope.directories().to_vec();
        let scope = scope.clone();
        iced::stream::channel(
            16,
            |mut out: futures::channel::mpsc::Sender<Message>| async move {
                use futures::{SinkExt, StreamExt};
                let (_owner, mut rx2) = match fs_bridge::FsBridge::start(&directories) {
                    Ok(watcher) => watcher,
                    Err(error) => {
                        let _ = out
                            .send(Message::FsWatchFailed(scope.clone(), error.to_string()))
                            .await;
                        return;
                    }
                };
                while let Some(event) = rx2.next().await {
                    let message = match event {
                        lume_core::FsEvent::Changed(paths) => Message::FsChanged(paths),
                        lume_core::FsEvent::Rescan => Message::FsRescan(directories.clone()),
                        lume_core::FsEvent::Error(error) => {
                            Message::FsWatchFailed(scope.clone(), error)
                        }
                    };
                    if out.send(message).await.is_err() {
                        break;
                    }
                }
            },
        )
    })
}

#[cfg(test)]
mod git_write_tests;
#[cfg(test)]
mod notes_data_directory_tests;
#[cfg(test)]
mod search_delivery_tests;
#[cfg(all(test, target_os = "linux"))]
mod search_measure_tests;
#[cfg(test)]
mod search_root_tests;
#[cfg(all(test, feature = "sql"))]
mod sql_delivery_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod diff_tests;
#[cfg(test)]
mod jj_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use futures::StreamExt;

    use lume_ui::tool::JjAction;
    use std::sync::atomic::Ordering;

    #[test]
    fn drop_app_cancella_ticket_jj_anche_senza_esci() {
        for load in [false, true] {
            let root = cartella_con_jj();
            let (mut app, _) = Lume::new_without_store();
            app.set_project_root(root.path().to_path_buf());
            let task = if load {
                app.run_jj_load()
            } else {
                app.run_jj_action(JjAction::New(None))
            };
            let cancel = app.jj_cancel.clone();
            assert!(!cancel.load(Ordering::Acquire));
            drop(app);
            let cancelled = cancel.load(Ordering::Acquire);
            drop(task);
            assert!(cancelled, "Drop app deve cancellare il ticket corrente");
        }
    }

    fn cartella_con_jj() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".jj")).unwrap();
        dir
    }

    fn cartella_senza_jj() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        if lume_jj::find_workspace_root(dir.path()).is_some() {
            return None;
        }
        Some(dir)
    }

    fn notifica(app: &Lume) -> (NoticeKind, String) {
        let (_, notice) = app.notice.as_ref().unwrap();
        (notice.kind, notice.text.clone())
    }

    #[test]
    fn pannello_disponibile_solo_se_il_progetto_ha_jj() {
        let (mut app, _) = Lume::new_without_store();
        assert!(!app.registry.is_available(tools::jj::ID));
        let con = cartella_con_jj();
        app.set_project_root(con.path().to_path_buf());
        assert!(app.registry.is_available(tools::jj::ID));
        assert_eq!(app.jj_workspace.as_deref(), Some(con.path()));
        let Some(senza) = cartella_senza_jj() else {
            return;
        };
        app.set_project_root(senza.path().to_path_buf());
        assert!(!app.registry.is_available(tools::jj::ID));
        assert_eq!(app.jj_workspace, None);
    }

    #[test]
    fn workspace_jj_trovato_nei_antenati() {
        let root = cartella_con_jj();
        let nested = root.path().join("servizi/api");
        std::fs::create_dir_all(&nested).unwrap();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(nested);
        assert_eq!(app.jj_workspace.as_deref(), Some(root.path()));
    }

    #[tokio::test]
    async fn uscita_attende_worker_jj_ammesso_senza_job_cerca()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let (mut app, _) = Lume::new_without_store();
        let (pending, permit) = jj_actions::admission::Pending::new(lume_ui::tool::JjAction::Push);
        assert!(permit.admit());
        let release = Arc::new(AtomicBool::new(false));
        let gate = release.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let worker = permit
            .start(move || {
                let _ = started.send(());
                while !gate.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok("worker raccolto".into())
            })
            .ok_or("worker non avviato")?;
        ready.await?;
        app.jj_pending_action = Some(pending);
        let close = app.esci(Some(iced::window::Id::unique()));
        let epoch = app.close_epoch;
        let guarded = app.search_exit.is_some();
        let _early = app.update(Message::SearchExitReady(epoch, true));
        let guarded_after_early = app.search_exit.is_some();
        release.store(true, Ordering::Release);
        let result = worker.await;
        assert!(
            result.is_ok(),
            "owner fixture raccolto prima delle assertion"
        );
        assert!(
            guarded,
            "chiusura deve possedere l’attesa JJ anche senza Cerca"
        );
        assert!(
            guarded_after_early,
            "ACK anticipato non deve liberare il gate"
        );
        use futures::StreamExt;
        let mut stream = iced_runtime::task::into_stream(close).ok_or("task uscita assente")?;
        let ready = stream.next().await.ok_or("esito raccolta uscita assente")?;
        let iced_runtime::Action::Output(message) = ready else {
            return Err("output uscita inatteso".into());
        };
        assert!(matches!(message, Message::SearchExitReady(e, true) if e == epoch));
        let _window_task = app.update(message);
        assert!(
            app.search_exit.is_none(),
            "gate raccolto prima di window close"
        );
        Ok(())
    }

    #[tokio::test]
    async fn subscription_app_raccoglie_il_load_e_applica_il_ticket_una_volta()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use iced_runtime::futures::subscription::{Hasher, Recipe, into_recipes};
        use std::hash::Hasher as _;

        let (mut app, _) = Lume::new_without_store();
        app.jj_gen = 1;
        app.jj_loading = true;
        let (pending, permit) = jj_actions::load_admission::Pending::new(1);
        assert!(permit.admit());
        let worker = permit
            .start(|| {
                Ok(lume_jj::JjSnapshot {
                    entries: Vec::new(),
                    status: "snapshot conservato".into(),
                })
            })
            .ok_or("worker load non avviato")?;
        let _ = worker.await;
        drop(permit);
        let collector = pending.collector();
        app.jj_pending_load = Some(pending);

        let expected =
            Subscription::run_with(collector.clone(), jj_actions::load_admission::completed)
                .map(Message::JjLoadCollected);
        let hash = |recipe: &dyn Recipe<Output = Message>| {
            let mut hasher = Hasher::default();
            recipe.hash(&mut hasher);
            hasher.finish()
        };
        let expected_hash = hash(
            &*into_recipes(expected)
                .pop()
                .ok_or("recipe attesa assente")?,
        );
        let actual = into_recipes(app.subscription());
        assert_eq!(
            actual
                .iter()
                .filter(|recipe| hash(&***recipe) == expected_hash)
                .count(),
            1,
            "la subscription reale app deve possedere il collector del load"
        );

        let signals = jj_actions::load_admission::completed(&collector)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(signals.len(), 1);
        let _ = app.update(Message::JjLoadCollected(signals[0].clone()));
        assert!(!app.jj_loading);
        assert!(app.jj_pending_load.is_none());
        let _ = app.update(Message::JjLoadCollected(signals[0].clone()));
        assert!(
            !app.jj_loading,
            "il segnale duplicato non riapplica il risultato"
        );
        Ok(())
    }

    #[tokio::test]
    async fn load_jj_vecchio_workspace_viene_scartato_prima_del_successore()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let root = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.jj_workspace = Some(root.path().to_path_buf());
        app.jj_gen = 1;
        app.jj_loading = true;
        let (pending, permit) = jj_actions::load_admission::Pending::new(1);
        assert!(permit.admit());
        let worker = permit
            .start(|| {
                Ok(lume_jj::JjSnapshot {
                    entries: Vec::new(),
                    status: "snapshot del workspace precedente".into(),
                })
            })
            .ok_or("worker load non avviato")?;
        let _ = worker.await;
        let collector = pending.collector();
        app.jj_pending_load = Some(pending);

        // Simula SetWorkspace: invalida la generazione e accoda il load nuovo.
        app.jj_gen = 2;
        app.jj_loading = false;
        app.jj_load_pending = true;
        let signals = jj_actions::load_admission::completed(&collector)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(signals.len(), 1);
        let _ = app.update(Message::JjLoadCollected(signals[0].clone()));

        let successor = app
            .jj_pending_load
            .as_ref()
            .ok_or("il load successore non possiede un ticket")?;
        assert_eq!(successor.generation(), app.jj_gen);
        assert_ne!(successor.generation(), 1);
        assert!(app.jj_loading);
        assert!(!app.jj_load_pending);
        Ok(())
    }

    #[test]
    fn cambio_progetto_annulla_e_invalida_il_lavoro_precedente() {
        let first = cartella_con_jj();
        let second = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(first.path().to_path_buf());
        app.jj_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)).into();
        let cancel = app.jj_cancel.clone();
        let generation = app.jj_gen;
        app.set_project_root(second.path().to_path_buf());
        assert!(cancel.load(Ordering::SeqCst));
        assert_eq!(app.jj_gen, generation + 1);
        assert_eq!(app.jj_workspace.as_deref(), Some(second.path()));
        let generation = app.jj_gen;
        app.set_project_root(second.path().to_path_buf());
        assert_eq!(app.jj_gen, generation);
    }

    #[test]
    fn cambio_workspace_non_rilascia_azione_jj_prima_del_ritorno_del_worker() {
        let first = cartella_con_jj();
        let second = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(first.path().to_path_buf());
        let _action = app.run_jj_action(JjAction::Push);
        let cancel = app.jj_cancel.clone();
        let pending = app.jj_pending_action.as_ref().unwrap().clone();

        app.set_project_root(second.path().to_path_buf());

        assert!(cancel.load(Ordering::SeqCst));
        assert!(
            pending.abandoned(),
            "l'azione fredda è revocata prima del worker"
        );
        assert!(
            !app.jj_busy,
            "non c'è worker da attendere prima dell'ammissione"
        );
        assert!(app.jj_action_generation.is_none());
    }

    #[test]
    fn azione_ammessa_resta_posseduta_dopo_cambio_workspace_e_poi_ricarica() {
        let first = cartella_con_jj();
        let second = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(first.path().to_path_buf());
        let _action = app.run_jj_action(JjAction::Push);
        let pending = app.jj_pending_action.as_ref().unwrap().clone();
        pending.simulate_admitted();
        let action_generation = app.jj_action_generation.unwrap();

        app.set_project_root(second.path().to_path_buf());
        assert!(app.jj_busy);
        assert_eq!(app.jj_action_generation, Some(action_generation));
        let _deferred = app.run_jj_load();
        assert!(app.jj_load_pending);
        assert!(!app.jj_loading);

        let _ = app.update(Message::JjActionDone(
            action_generation,
            JjAction::Push,
            Err(jj_actions::CANCELLED.into()),
        ));

        assert!(!app.jj_busy);
        assert!(app.jj_pending_action.is_none());
        assert!(app.jj_action_generation.is_none());
        assert!(!app.jj_load_pending);
        assert!(
            app.jj_loading,
            "la lettura del workspace nuovo parte dopo il join"
        );
    }

    #[test]
    fn caricamento_accodato_durante_azione_corrente_parte_al_completamento() {
        let root = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(root.path().to_path_buf());
        let _action = app.run_jj_action(JjAction::Push);
        let generation = app.jj_action_generation.unwrap();
        let _load = app.run_jj_load();
        assert!(app.jj_load_pending);

        let _ = app.update(Message::JjActionDone(
            generation,
            JjAction::Push,
            Err("errore di prova".into()),
        ));

        assert!(!app.jj_busy);
        assert!(!app.jj_load_pending);
        assert!(app.jj_loading);
        assert!(app.jj_gen > generation);
    }

    #[test]
    fn load_rinnova_interruttore_e_generazione() {
        let root = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(root.path().to_path_buf());
        let generation = app.jj_gen;
        let _task = app.run_jj_load();
        assert_eq!(app.jj_gen, generation + 1);
        assert!(!app.jj_cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn azione_fuori_sequenza_non_parte_durante_il_caricamento_jj() {
        let root = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(root.path().to_path_buf());
        let _load = app.run_jj_load();
        let generation = app.jj_gen;
        let cancel = app.jj_cancel.clone();

        let _action = app.run_jj_action(JjAction::Push);

        assert_eq!(app.jj_gen, generation);
        assert!(std::sync::Arc::ptr_eq(&cancel, &app.jj_cancel));
        assert!(!app.jj_busy);
        assert!(notifica(&app).1.contains("Jujutsu è ancora in corso"));
    }

    #[test]
    fn caricamento_fuori_sequenza_non_sostituisce_un_azione_jj() {
        let root = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(root.path().to_path_buf());
        let _action = app.run_jj_action(JjAction::Push);
        let generation = app.jj_gen;
        let cancel = app.jj_cancel.clone();

        let _load = app.run_jj_load();

        assert_eq!(app.jj_gen, generation);
        assert!(std::sync::Arc::ptr_eq(&cancel, &app.jj_cancel));
        assert!(app.jj_busy);
    }

    #[test]
    fn caricamento_duplicato_non_sostituisce_il_worker_jj_corrente() {
        let root = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(root.path().to_path_buf());
        let _first = app.run_jj_load();
        let generation = app.jj_gen;
        let cancel = app.jj_cancel.clone();

        let _duplicate = app.run_jj_load();

        assert!(app.jj_loading);
        assert_eq!(app.jj_gen, generation);
        assert!(std::sync::Arc::ptr_eq(&cancel, &app.jj_cancel));
    }

    #[test]
    fn risposta_jj_superata_non_cancella_il_caricamento_del_nuovo_workspace() {
        let first = cartella_con_jj();
        let second = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(first.path().to_path_buf());
        let _old = app.run_jj_load();
        let old_generation = app.jj_gen;

        app.set_project_root(second.path().to_path_buf());
        let _current = app.run_jj_load();
        assert!(app.jj_loading);
        assert_ne!(app.jj_gen, old_generation);

        let _ = app.update(Message::JjLoaded(
            old_generation,
            Err("risposta vecchia".into()),
        ));

        assert!(app.jj_loading);
    }

    #[test]
    fn senza_workspace_non_avvia_lavoro() {
        let (mut app, _) = Lume::new_without_store();
        let _ = app.run_jj_load();
        let _ = app.run_jj_action(JjAction::Push);
        assert_eq!(app.jj_gen, 0);
        assert!(!app.jj_busy);
        assert!(app.notice.is_none());
    }

    #[test]
    fn un_azione_alla_volta_e_cancel_ferma_il_worker() {
        let root = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(root.path().to_path_buf());
        let _first_task = app.run_jj_action(JjAction::Push);
        assert!(app.jj_busy);
        let cancel = app.jj_cancel.clone();
        let _ = app.run_jj_action(JjAction::New(None));
        assert!(std::sync::Arc::ptr_eq(&cancel, &app.jj_cancel));
        assert!(notifica(&app).1.contains("ancora in corso"));
        let _ = app.apply_tool_requests(vec![(tools::jj::ID, ToolRequest::JjCancel)]);
        assert!(app.jj_cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn esiti_azione_diventano_notifiche_info_error_o_annullato() {
        let (mut app, _) = Lume::new_without_store();
        app.jj_busy = true;
        app.jj_action_generation = Some(0);
        let _ = app.update(Message::JjActionDone(
            0,
            JjAction::Push,
            Ok("Nothing changed.".into()),
        ));
        assert_eq!(
            notifica(&app),
            (NoticeKind::Info, "Jujutsu - Push: Nothing changed.".into())
        );
        app.jj_busy = true;
        app.jj_action_generation = Some(0);
        let _ = app.update(Message::JjActionDone(
            0,
            JjAction::Push,
            Err("Error: remote\nHint: configure".into()),
        ));
        assert_eq!(
            notifica(&app),
            (
                NoticeKind::Error,
                "Jujutsu - Push non riuscita: Error: remote · Hint: configure".into()
            )
        );
        app.jj_busy = true;
        app.jj_action_generation = Some(0);
        let _ = app.update(Message::JjActionDone(
            0,
            JjAction::Push,
            Err(jj_actions::CANCELLED.into()),
        ));
        assert_eq!(
            notifica(&app),
            (NoticeKind::Info, "Jujutsu - Push: annullato".into())
        );
    }

    #[test]
    fn esito_di_un_azione_del_progetto_precedente_viene_scartato() {
        let prima = cartella_con_jj();
        let dopo = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(prima.path().to_path_buf());
        let _ = app.run_jj_action(JjAction::Push);
        let generation = app.jj_gen;
        app.set_project_root(dopo.path().to_path_buf());
        assert!(app.jj_gen > generation);
        assert!(!app.jj_busy);
        let _ = app.update(Message::JjActionDone(
            generation,
            JjAction::Push,
            Ok("vecchio".into()),
        ));
        assert!(app.notice.is_none());
    }

    #[test]
    fn cambio_progetto_chiude_jujutsu_e_ripristino_filtra_workspace_mancante() {
        let con = cartella_con_jj();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(con.path().to_path_buf());
        let _ = app.registry.set_active_panel(Some(tools::jj::ID));
        assert!(app.registry.is_open(tools::jj::ID));
        let Some(senza) = cartella_senza_jj() else {
            return;
        };
        app.set_project_root(senza.path().to_path_buf());
        assert!(!app.registry.is_open(tools::jj::ID));
        assert!(!app.registry.is_available(tools::jj::ID));

        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.workspace_root = Some(senza.path().to_path_buf());
        snapshot.left_tools = vec!["project".into(), "jj".into()];
        snapshot.active_panel = Some("jj".into());
        let (mut restored, _) = Lume::new_without_store();
        restored.restore_snapshot(snapshot);
        assert_ne!(restored.registry.active_panel, Some(tools::jj::ID));
        assert!(!restored.registry.is_available(tools::jj::ID));
        assert_eq!(
            restored
                .registry
                .left
                .iter()
                .filter(|&&id| id == tools::jj::ID)
                .count(),
            1
        );
    }
}

#[cfg(test)]
mod appunti_scarico_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// Esegue il test figlio con il vero Cestino in una cartella temporanea,
    /// senza cambiare le variabili d'ambiente degli altri test concorrenti.
    #[cfg(unix)]
    fn cestino_isolato(nome: &str, indisponibile: bool) -> bool {
        if std::env::var_os("LUME_TEST_DELETE_FIGLIO").is_some() {
            return true;
        }
        let ambiente = tempfile::tempdir().unwrap();
        let dati = ambiente.path().join("dati");
        if indisponibile {
            std::fs::write(&dati, "non è una cartella").unwrap();
        } else {
            std::fs::create_dir(&dati).unwrap();
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(format!("app::appunti_scarico_tests::{nome}"))
            .arg("--exact")
            .arg("--nocapture")
            .env("LUME_TEST_DELETE_FIGLIO", "1")
            .env("HOME", ambiente.path())
            .env("XDG_DATA_HOME", &dati)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "figlio: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "il test figlio non è stato eseguito: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        false
    }

    pub(super) fn risultato_task(task: Task<Message>) -> Message {
        use futures::StreamExt;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(task).unwrap();
            loop {
                match stream.next().await {
                    Some(iced_runtime::Action::Output(message)) => return message,
                    Some(_) => {}
                    None => panic!("il Task non ha prodotto un risultato"),
                }
            }
        })
    }

    fn cestino_assente(_store: &NoteStore, _path: &Path) -> Result<DeleteDisposition, String> {
        Ok(DeleteDisposition::NeedsPermanentConfirmation)
    }

    /// App con lo stato Appunti pronto su una cartella temporanea vera.
    fn app_con_appunti(cartella: &Path) -> (Lume, NoteStore, u64) {
        let (mut app, _) = Lume::new_without_store();
        let generation = app.begin_notes_load().expect("primo caricamento");
        let store = NoteStore::open(cartella).unwrap();
        app.finish_notes_load(generation, cartella.to_path_buf(), Vec::new())
            .unwrap();
        (app, store, generation)
    }

    /// Crea una nota vera su disco e la apre in una scheda.
    fn apri_nota(app: &mut Lume, store: &NoteStore, titolo: &str) -> (DocId, LoadedNote) {
        let nota = store.create(titolo).unwrap();
        let doc = app.open_loaded_note(nota.clone());
        (doc, nota)
    }

    #[cfg(unix)]
    #[test]
    fn save_active_durante_eliminazione_reale_e_rifiutato_senza_coda() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "save_active_durante_eliminazione_reale_e_rifiutato_senza_coda",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, note) = apri_nota(&mut app, &store, "Nota");
        let altro = dir.path().join("altro.txt");
        std::fs::write(&altro, "prima").unwrap();
        app.open_file(&altro).unwrap();
        let doc = app.active_doc().unwrap();
        scrivi(&mut app, doc, " dopo");
        let delete = app.delete_note(note.note.header.id.clone());
        let save = app.update(Message::SaveActive);
        assert!(app.notice.as_ref().is_some_and(
            |(_, n)| n.text.contains("salvataggio") && n.text.contains("eliminazione")
        ));
        assert!(
            !(app.scritture.contiene_salvataggio(doc) || app.saves.active_document() == Some(doc))
        );
        app.completa_salvataggi(Task::batch([delete, save]));
        assert_eq!(std::fs::read_to_string(altro).unwrap(), "prima");
        assert!(app.workspace.get(doc).unwrap().is_dirty());
    }

    #[cfg(unix)]
    #[test]
    fn durante_delete_la_scheda_in_ripristino_non_accoda_save_o_input_bloccato() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "durante_delete_la_scheda_in_ripristino_non_accoda_save_o_input_bloccato",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (nota_doc, note) = apri_nota(&mut app, &store, "Nota");
        app.pending_restored_tabs
            .insert(nota_doc, note.path().to_path_buf());
        let altro = dir.path().join("in-ripristino.txt");
        std::fs::write(&altro, "reale").unwrap();
        let altro_doc = app.workspace.open(Some(altro.clone()), "");
        app.groups.open(app.groups.focus(), altro_doc);
        app.pending_restored_tabs.insert(altro_doc, altro);
        let task = app.delete_note(note.note.header.id.clone());
        let _ = app.update(Message::EditorIn(app.groups.focus(), EditorCommand::Save));
        assert!(
            !app.restored_tab_actions
                .get(&altro_doc)
                .is_some_and(|actions| actions.contains(&DeferredRestoredAction::SaveDocument))
        );
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, notice)| notice.text.contains("eliminazione"))
        );
        let _ = app.apply_editor_command(EditorCommand::Save);
        assert!(
            !app.restored_tab_actions
                .get(&altro_doc)
                .is_some_and(|actions| actions.contains(&DeferredRestoredAction::SaveDocument))
        );
        app.groups.activate(nota_doc);
        let _ = app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert("da ripetere".into()),
        ));
        assert!(!app.pending_editor_input(nota_doc));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, notice)| notice.text.contains("eliminazione"))
        );
        let _ = app.update(Message::EditorIn(app.groups.focus(), EditorCommand::Cut));
        assert!(
            !app.restored_tab_actions
                .get(&nota_doc)
                .is_some_and(|actions| actions.contains(&DeferredRestoredAction::CutSelection))
        );
        app.completa_salvataggi(task);
    }

    #[cfg(unix)]
    #[test]
    fn delete_non_scarta_un_salvataggio_precedente_in_ripristino() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "delete_non_scarta_un_salvataggio_precedente_in_ripristino",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (doc, note) = apri_nota(&mut app, &store, "Nota");
        app.pending_restored_tabs
            .insert(doc, note.path().to_path_buf());
        let _ = app.defer_restored_action(doc, DeferredRestoredAction::SaveDocument);
        let task = app.delete_note(note.note.header.id.clone());
        app.completa_salvataggi(task);
        assert!(note.path().is_file());
        assert!(
            app.restored_tab_actions
                .get(&doc)
                .is_some_and(|actions| actions.contains(&DeferredRestoredAction::SaveDocument))
        );
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, notice)| notice.text.contains("caricamento"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn eliminazione_reale_impedisce_edit_sulla_nota() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato("eliminazione_reale_impedisce_edit_sulla_nota", false) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (doc, note) = apri_nota(&mut app, &store, "Nota");
        let versione = app.workspace.get(doc).unwrap().version();
        let delete = app.delete_note(note.note.header.id.clone());
        let _ = app.update(Message::EditorIn(
            crate::editor_groups::GroupId::Primary,
            EditorCommand::Insert("non perdere".into()),
        ));
        assert_eq!(app.workspace.get(doc).unwrap().version(), versione);
        app.completa_salvataggi(delete);
    }

    #[cfg(unix)]
    #[test]
    fn alias_modificato_prima_del_preflight_annulla_delete_e_preserva_buffer() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "alias_modificato_prima_del_preflight_annulla_delete_e_preserva_buffer",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, note) = apri_nota(&mut app, &store, "Nota");
        let alias = dir.path().join("alias.md");
        std::os::unix::fs::symlink(note.path(), &alias).unwrap();
        app.open_file(&alias).unwrap();
        let doc = app.active_doc().unwrap();
        let prima = std::fs::read(note.path()).unwrap();
        let task = app.delete_note(note.note.header.id.clone());
        let _ = app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert("modifica da conservare".into()),
        ));
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        app.completa_salvataggi(task);
        assert_eq!(std::fs::read(note.path()).unwrap(), prima);
        assert!(
            app.workspace
                .get(doc)
                .unwrap()
                .rope()
                .to_string()
                .contains("modifica da conservare")
        );
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("alias"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_sporco_e_riconosciuto_come_alias_della_nota() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "hard_link_sporco_e_riconosciuto_come_alias_della_nota",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, note) = apri_nota(&mut app, &store, "Nota");
        let alias = dir.path().join("hard-link.md");
        std::fs::hard_link(note.path(), &alias).unwrap();
        app.open_file(&alias).unwrap();
        let doc = app.active_doc().unwrap();
        scrivi(&mut app, doc, "da conservare");
        let task = app.delete_note(note.note.header.id.clone());
        app.completa_salvataggi(task);
        assert!(note.path().is_file());
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("alias"))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hard_link_pulito_resta_mentre_il_path_della_nota_va_nel_cestino_senza_ricreazione() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "hard_link_pulito_resta_mentre_il_path_della_nota_va_nel_cestino_senza_ricreazione",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (nota_doc, note) = apri_nota(&mut app, &store, "Nota");
        let bytes = std::fs::read(note.path()).unwrap();
        let alias = dir.path().join("hard-link.md");
        std::fs::hard_link(note.path(), &alias).unwrap();
        app.open_file(&alias).unwrap();
        let alias_doc = app.active_doc().unwrap();
        let altro = dir.path().join("altro.txt");
        std::fs::write(&altro, "prima").unwrap();
        app.open_file(&altro).unwrap();
        let altro_doc = app.active_doc().unwrap();

        let preparato = risultato_task(app.delete_note(note.note.header.id.clone()));
        let worker = app.update(preparato);
        for doc in [nota_doc, alias_doc] {
            assert!(
                (app.scritture
                    .guarda(Some((
                        doc,
                        app.workspace.get(doc).and_then(lume_core::Document::path)
                    )))
                    .file
                    != crate::scritture::Presa::Libero
                    || app.git_operazione.is_some())
            );
            let versione = app.workspace.get(doc).unwrap().version();
            app.groups.activate(doc);
            let _ = app.update(Message::EditorIn(
                app.groups.focus(),
                EditorCommand::Insert("vietato".into()),
            ));
            assert_eq!(app.workspace.get(doc).unwrap().version(), versione);
        }
        scrivi(&mut app, altro_doc, " dopo");
        // Richieste automatiche rimandate: solo l'estraneo sopravvive.
        app.queue_save_per(nota_doc, crate::scritture::Causa::Automatico);
        app.queue_save_per(alias_doc, crate::scritture::Causa::Automatico);
        app.queue_save_per(altro_doc, crate::scritture::Causa::Automatico);
        assert!(
            (app.scritture.contiene_salvataggio(nota_doc)
                || app.saves.active_document() == Some(nota_doc))
        );
        assert!(
            (app.scritture.contiene_salvataggio(alias_doc)
                || app.saves.active_document() == Some(alias_doc))
        );
        assert!(
            (app.scritture.contiene_salvataggio(altro_doc)
                || app.saves.active_document() == Some(altro_doc))
        );

        let esito = risultato_task(worker);
        let Message::NotesDeleted(request, id, scope, lease, result) = esito else {
            panic!("il worker non ha restituito l'esito di eliminazione");
        };
        assert!(matches!(&result, Ok(DeleteDisposition::Trashed)));
        // Verifica la revoca prima che il seguito consumi la coda legacy.
        let completamento = app.note_deleted(request, id, scope, lease, result);
        assert!(
            !(app.scritture.contiene_salvataggio(nota_doc)
                || app.saves.active_document() == Some(nota_doc))
        );
        assert!(
            !(app.scritture.contiene_salvataggio(alias_doc)
                || app.saves.active_document() == Some(alias_doc))
        );
        assert!(
            (app.scritture.contiene_salvataggio(altro_doc)
                || app.saves.active_document() == Some(altro_doc))
        );
        assert!(app.workspace.get(nota_doc).is_none());
        assert!(app.workspace.get(alias_doc).is_none());
        let seguito = app.applica_seguito();
        assert_eq!(app.saves.active_document(), Some(altro_doc));
        app.completa_salvataggi(Task::batch([completamento, seguito]));

        assert_eq!(std::fs::read(&altro).unwrap(), b"prima dopo");
        assert!(!note.path().exists());
        assert_eq!(std::fs::read(&alias).unwrap(), bytes);
        let cestino = PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap()).join("Trash");
        let nome = note.path().file_name().unwrap().to_string_lossy();
        assert_eq!(
            std::fs::read(cestino.join("files").join(&*nome)).unwrap(),
            bytes
        );
        let info = std::fs::read_to_string(cestino.join("info").join(format!("{nome}.trashinfo")))
            .unwrap();
        assert!(
            info.lines()
                .any(|riga| riga == format!("Path={}", note.path().display()))
        );
        let alias_canonico = lume_core::canonical_path(&alias).unwrap();
        assert!(
            store
                .scan()
                .unwrap()
                .iter()
                .any(|nota| nota.path == alias_canonico)
        );
        let _ = app.update(Message::NotesAutosaveDue(nota_doc, 0));
        assert!(!note.path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn eliminazione_consente_input_e_chiusura_di_documento_estraneo() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "eliminazione_consente_input_e_chiusura_di_documento_estraneo",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, note) = apri_nota(&mut app, &store, "Nota");
        let altro = dir.path().join("altro.txt");
        std::fs::write(&altro, "prima").unwrap();
        app.open_file(&altro).unwrap();
        let doc = app.active_doc().unwrap();
        let da_chiudere = dir.path().join("da-chiudere.txt");
        std::fs::write(&da_chiudere, "intatto").unwrap();
        app.open_file(&da_chiudere).unwrap();
        let doc_da_chiudere = app.active_doc().unwrap();
        let preparato = risultato_task(app.delete_note(note.note.header.id.clone()));
        let worker = app.update(preparato);
        assert!(app.note_delete.as_ref().unwrap().correlati.is_some());
        let _ = app.update(Message::EditorTabBar(
            crate::editor_tab_bar::EditorTabBarMessage::Close(doc_da_chiudere),
        ));
        assert!(!app.groups.contains(doc_da_chiudere));
        assert!(app.workspace.get(doc_da_chiudere).is_none());
        assert_eq!(std::fs::read(&da_chiudere).unwrap(), b"intatto");
        app.groups.activate(doc);
        let _ = app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert("nuovo".into()),
        ));
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert!(
            !(app
                .scritture
                .guarda(Some((
                    doc,
                    app.workspace.get(doc).and_then(lume_core::Document::path)
                )))
                .file
                != crate::scritture::Presa::Libero
                || app.git_operazione.is_some())
        );
        app.completa_salvataggi(worker);
        assert!(!note.path().exists());
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert!(
            app.workspace
                .get(doc)
                .unwrap()
                .rope()
                .to_string()
                .contains("nuovo")
        );
        assert_eq!(std::fs::read(&altro).unwrap(), b"prima");
    }

    #[cfg(unix)]
    #[test]
    fn autosave_legacy_della_nota_estranea_riprende_alla_quiete_del_delete() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "autosave_legacy_della_nota_estranea_riprende_alla_quiete_del_delete",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, da_eliminare) = apri_nota(&mut app, &store, "Da eliminare");
        let (estraneo_doc, estranea) = apri_nota(&mut app, &store, "Estranea");
        let prima = std::fs::read(estranea.path()).unwrap();
        let preparato = risultato_task(app.delete_note(da_eliminare.note.header.id.clone()));
        let worker = app.update(preparato);
        assert!(app.note_delete.as_ref().unwrap().correlati.is_some());

        let timer = app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert("salvato dalla coda".into()),
        ));
        let versione = app.workspace.get(estraneo_doc).unwrap().version();
        let scadenza = risultato_task(timer);
        assert!(matches!(
            &scadenza,
            Message::NotesAutosaveDue(doc, attesa)
                if *doc == estraneo_doc && *attesa == versione
        ));
        let _ = app.update(scadenza);
        assert!(
            (app.scritture.contiene_salvataggio(estraneo_doc)
                || app.saves.active_document() == Some(estraneo_doc))
        );
        assert_eq!(app.saves.active_document(), None);
        assert_eq!(std::fs::read(estranea.path()).unwrap(), prima);
        assert_eq!(
            app.notes.as_ref().unwrap().salvataggi.get(&estraneo_doc),
            Some(&crate::autosave::StatoSalvataggio::InCorso { versione })
        );

        let esito = risultato_task(worker);
        assert!(matches!(
            &esito,
            Message::NotesDeleted(_, _, _, _, Ok(DeleteDisposition::Trashed))
        ));
        let seguito = app.update(esito);
        assert_eq!(
            app.scritture.guarda(None).in_volo,
            Some(crate::scritture::Tipo::Salvataggio)
        );
        assert_eq!(app.saves.active_document(), Some(estraneo_doc));
        assert_eq!(std::fs::read(estranea.path()).unwrap(), prima);
        app.completa_salvataggi(seguito);

        let dopo = std::fs::read(estranea.path()).unwrap();
        assert_ne!(dopo, prima);
        assert!(dopo.ends_with(b"salvato dalla coda"));
        assert_eq!(
            store.load(estranea.path()).unwrap().note.markdown,
            "salvato dalla coda"
        );
        assert!(!app.workspace.get(estraneo_doc).unwrap().is_dirty());
        assert_eq!(
            app.notes.as_ref().unwrap().salvataggi.get(&estraneo_doc),
            Some(&crate::autosave::StatoSalvataggio::Pulito)
        );
        assert!(
            !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
                || app.saves.active_document().is_some())
        );
        assert!(!da_eliminare.path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn salva_tutto_e_salva_alla_chiusura_sono_rifiutati_senza_coda() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "salva_tutto_e_salva_alla_chiusura_sono_rifiutati_senza_coda",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, note) = apri_nota(&mut app, &store, "Nota");
        let altro = dir.path().join("altro.txt");
        std::fs::write(&altro, "prima").unwrap();
        app.open_file(&altro).unwrap();
        let doc = app.active_doc().unwrap();
        scrivi(&mut app, doc, " dopo");
        let task = app.delete_note(note.note.header.id.clone());
        assert!(app.begin_git_write().is_none());
        let _ = app.update(Message::MenuAction(MenuAct::SaveAll));
        assert!(
            !(app.scritture.contiene_salvataggio(doc) || app.saves.active_document() == Some(doc))
        );
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("eliminazione"))
        );
        app.chiudi_scheda(doc);
        let _ = app.update(Message::ConfermaChiusura(
            crate::conferma_chiusura::ConfermaMessage::Salva,
        ));
        assert!(app.close_after_save.is_none());
        assert!(
            !(app.scritture.contiene_salvataggio(doc) || app.saves.active_document() == Some(doc))
        );
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("eliminazione"))
        );
        app.completa_salvataggi(task);
        assert_eq!(std::fs::read_to_string(altro).unwrap(), "prima");
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert!(app.chiusura_attiva().is_some());
        assert!(app.begin_git_write().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn preflight_protegge_alias_e_risultati_duplicati_non_liberano_successore() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "preflight_protegge_alias_e_risultati_duplicati_non_liberano_successore",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (note_doc, note) = apri_nota(&mut app, &store, "Nota");
        let bytes = std::fs::read(note.path()).unwrap();
        let alias = dir.path().join("alias.md");
        std::os::unix::fs::symlink(note.path(), &alias).unwrap();
        app.open_file(&alias).unwrap();
        let alias_doc = app.active_doc().unwrap();
        let altro = dir.path().join("altro.txt");
        std::fs::write(&altro, "prima").unwrap();
        app.open_file(&altro).unwrap();
        let altro_doc = app.active_doc().unwrap();
        let preflight = risultato_task(app.delete_note(note.note.header.id.clone()));
        let worker = app.update(preflight.clone());
        let epoca = app.scritture.guarda(None).epoca;
        assert_eq!(
            app.scritture.guarda(Some((alias_doc, None))).file,
            crate::scritture::Presa::Bloccato
        );
        let _ = app.update(preflight);
        assert_eq!(app.scritture.guarda(None).epoca, epoca);
        app.groups.activate(alias_doc);
        let versione = app.workspace.get(alias_doc).unwrap().version();
        let _ = app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert("vietato".into()),
        ));
        assert_eq!(app.workspace.get(alias_doc).unwrap().version(), versione);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("eliminazione"))
        );
        app.groups.activate(altro_doc);
        let _ = app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert("libero".into()),
        ));
        assert!(app.workspace.get(altro_doc).unwrap().is_dirty());
        // Riproduce una vecchia richiesta già in coda arrivata dopo il preflight.
        app.queue_save(note_doc);
        let esito = risultato_task(worker);
        assert!(matches!(
            esito,
            Message::NotesDeleted(_, _, _, _, Ok(DeleteDisposition::Trashed))
        ));
        let duplicato = esito.clone();
        let _ = app.update(esito);
        assert!(
            !(app.scritture.contiene_salvataggio(note_doc)
                || app.saves.active_document() == Some(note_doc))
        );
        assert!(app.workspace.get(note_doc).is_none());
        assert!(app.workspace.get(alias_doc).is_none());
        assert!(!note.path().exists());
        let _ = app.update(Message::NotesAutosaveDue(note_doc, 0));
        assert!(!note.path().exists());
        let cestino = if cfg!(target_os = "macos") {
            PathBuf::from(std::env::var_os("HOME").unwrap()).join(".Trash")
        } else {
            PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap()).join("Trash/files")
        };
        assert!(std::fs::read_dir(cestino).unwrap().any(|entry| {
            entry
                .ok()
                .and_then(|entry| std::fs::read(entry.path()).ok())
                .as_deref()
                == Some(bytes.as_slice())
        }));
        let (_, seconda) = apri_nota(&mut app, &store, "Seconda");
        let successore = app.delete_note(seconda.note.header.id.clone());
        let lease = app.note_delete.as_ref().unwrap().lease;
        let _ = app.update(duplicato);
        assert_eq!(app.note_delete.as_ref().unwrap().lease, lease);
        assert_eq!(
            app.scritture.guarda(None).in_volo,
            Some(crate::scritture::Tipo::EliminaAppunto)
        );
        app.completa_salvataggi(successore);
        assert!(app.note_delete.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cambio_schede_prima_del_preflight_annulla_la_mutazione() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "cambio_schede_prima_del_preflight_annulla_la_mutazione",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, note) = apri_nota(&mut app, &store, "Nota");
        let altro = dir.path().join("altro.txt");
        std::fs::write(&altro, "prima").unwrap();
        app.open_file(&altro).unwrap();
        let doc = app.active_doc().unwrap();
        let task = app.delete_note(note.note.header.id.clone());
        app.chiudi_scheda(doc);
        assert!(app.workspace.get(doc).is_none());
        app.completa_salvataggi(task);
        assert!(note.path().is_file());
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("binding"))
        );
        assert_eq!(app.scritture.guarda(None).in_volo, None);
    }

    #[cfg(unix)]
    #[test]
    fn identita_cambiata_dopo_il_preflight_ferma_il_worker_reale() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "identita_cambiata_dopo_il_preflight_ferma_il_worker_reale",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, note) = apri_nota(&mut app, &store, "Nota");
        let preparato = risultato_task(app.delete_note(note.note.header.id.clone()));
        let worker = app.update(preparato);
        std::fs::write(note.path(), "modifica esterna reale").unwrap();
        let risultato = risultato_task(worker);
        assert!(matches!(
            risultato,
            Message::NotesDeleted(_, _, _, _, Err(_))
        ));
        let _ = app.update(risultato);
        assert_eq!(
            std::fs::read_to_string(note.path()).unwrap(),
            "modifica esterna reale"
        );
        assert_eq!(app.scritture.guarda(None).in_volo, None);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("cambiat"))
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn errore_del_cestino_reale_rilascia_il_lease_senza_eliminare() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "errore_del_cestino_reale_rilascia_il_lease_senza_eliminare",
            true,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (doc, note) = apri_nota(&mut app, &store, "Nota");
        let task = app.delete_note(note.note.header.id.clone());
        app.completa_salvataggi(task);
        assert!(note.path().is_file());
        assert!(app.workspace.get(doc).is_some());
        assert!(app.note_delete.is_none());
        assert_eq!(app.scritture.guarda(None).in_volo, None);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| matches!(n.kind, NoticeKind::Error))
        );
    }

    #[cfg(unix)]
    #[test]
    fn successo_di_scope_precedente_non_chiude_buffer_corrente() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "successo_di_scope_precedente_non_chiude_buffer_corrente",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (vecchio_doc, note) = apri_nota(&mut app, &store, "Nota");
        let preparato = risultato_task(app.delete_note(note.note.header.id.clone()));
        let worker = app.update(preparato);
        app.next_notes_scan_generation();
        let nuova_cartella = tempfile::tempdir().unwrap();
        let nuovo_store = NoteStore::open(nuova_cartella.path()).unwrap();
        let nuova_nota = nuovo_store.create("Nuova cartella").unwrap();
        let notes = app.notes.as_mut().unwrap();
        notes.store = Some(nuovo_store);
        notes.directory = Some(nuova_cartella.path().to_path_buf());
        let doc = app.open_loaded_note(nuova_nota.clone());
        let _ = app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert("buffer corrente".into()),
        ));
        let esito = risultato_task(worker);
        assert!(matches!(
            esito,
            Message::NotesDeleted(_, _, _, _, Ok(DeleteDisposition::Trashed))
        ));
        let _ = app.update(esito);
        assert!(
            app.workspace
                .get(doc)
                .unwrap()
                .rope()
                .to_string()
                .contains("buffer corrente")
        );
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert!(app.workspace.get(vecchio_doc).is_some());
        assert!(nuova_nota.path().is_file());
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("cartella precedente"))
        );
        assert_eq!(app.scritture.guarda(None).in_volo, None);
    }

    #[test]
    fn cancellazione_con_cestino_assente_serializza_save_e_non_usa_generazione_vista() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (note_doc, note) = apri_nota(&mut app, &store, "Nota");
        let other = dir.path().join("other.txt");
        std::fs::write(&other, "prima").unwrap();
        app.open_file(&other).unwrap();
        let other_doc = app.active_doc().unwrap();
        scrivi(&mut app, other_doc, "nuovo");
        let task = app.delete_note_with(note.note.header.id.clone(), cestino_assente);
        assert!(app.note_delete.is_some());
        let ticket = app.note_delete.as_ref().unwrap().request;
        let lease = app.note_delete.as_ref().unwrap().lease;
        let scope = app.note_delete.as_ref().unwrap().scope;
        let _ = app.update(Message::NotesDeleted(
            ticket + 1,
            note.note.header.id.clone(),
            scope,
            lease,
            Err("ticket estraneo".into()),
        ));
        assert!(app.note_delete.is_some());
        let _ = app.update(Message::SaveDocFixture(other_doc));
        assert_eq!(std::fs::read(&other).unwrap(), b"prima");
        app.next_notes_generation();
        app.completa_salvataggi(task);
        assert!(app.note_delete.is_none());
        assert_eq!(std::fs::read(other).unwrap(), b"prima");
        assert!(note.path().is_file());
        assert!(app.groups.contains(note_doc));
        assert!(
            app.persistence_error
                .as_ref()
                .unwrap()
                .contains("la nota non è stata eliminata")
        );
    }

    #[test]
    fn chiusura_non_salvare_aspetta_risposta_del_cestino() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (doc, note) = apri_nota(&mut app, &store, "Nota");
        let task = app.delete_note_with(note.note.header.id.clone(), cestino_assente);
        app.close_tab(doc);
        let _ = app.rispondi_chiusura(crate::conferma_chiusura::ConfermaMessage::NonSalvare);
        assert!(app.groups.contains(doc));
        app.completa_salvataggi(task);
        assert!(!app.groups.contains(doc));
        assert!(note.path().is_file());
        assert!(app.note_delete.is_none());
    }

    #[test]
    fn pulizia_dopo_rimozione_fs_non_scarta_le_battute_successive() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (doc, note) = apri_nota(&mut app, &store, "Nota");
        scrivi(&mut app, doc, "non perdere");
        // Postcondizione della pulizia, non test del Cestino Windows:
        // rimozione FS reale seguita dal codice che tratta il file assente.
        std::fs::remove_file(note.path()).unwrap();
        app.finish_note_delete(&note.note.header.id);
        assert!(app.groups.contains(doc));
        assert!(
            app.workspace
                .get(doc)
                .unwrap()
                .rope()
                .to_string()
                .contains("non perdere")
        );
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert!(
            app.notes
                .as_ref()
                .unwrap()
                .open_saves
                .contains_key(&note.note.header.id)
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellazione_rifiuta_anche_un_alias_aperto_sporco() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato("cancellazione_rifiuta_anche_un_alias_aperto_sporco", false) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(dir.path());
        let (_, note) = apri_nota(&mut app, &store, "Nota");
        let alias = dir.path().join("alias.md");
        std::os::unix::fs::symlink(note.path(), &alias).unwrap();
        app.open_file(&alias).unwrap();
        let doc = app.active_doc().unwrap();
        scrivi(&mut app, doc, "non perdere");
        let task = app.delete_note(note.note.header.id.clone());
        app.completa_salvataggi(task);
        assert!(note.path().is_file());
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert!(
            app.persistence_error
                .as_ref()
                .unwrap()
                .contains("anche tramite alias")
        );
    }

    /// Aggiunge testo in coda al documento, come una battuta dell'utente.
    fn scrivi(app: &mut Lume, doc: DocId, testo: &str) {
        let lunghezza = app.workspace.get(doc).unwrap().rope().len_chars();
        let cambi = lume_core::ChangeSet::replace_many(
            &[(lunghezza, lunghezza, testo.to_owned())],
            lunghezza,
        )
        .unwrap();
        app.workspace
            .get_mut(doc)
            .unwrap()
            .apply(lume_core::Transaction {
                changes: cambi,
                selection_after: None,
            });
    }

    fn corpo_su_disco(store: &NoteStore, nota: &LoadedNote) -> String {
        store.load(nota.path()).unwrap().note.markdown
    }

    #[test]
    fn scarico_cache_ma_salvo_la_scheda_aperta_con_guardia_di_identita() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo originale");
        scrivi(&mut app, doc, "testo aggiornato");
        assert!(app.workspace.get(doc).unwrap().is_dirty());

        app.unload_notes_view_cache();

        let notes = app.notes.as_ref().unwrap();
        assert!(notes.loaded.is_empty(), "le note caricate sono scaricate");
        assert!(notes.summaries.is_empty(), "i riepiloghi sono scaricati");
        assert_eq!(notes.loaded.capacity(), 0, "memoria restituita");
        assert!(notes.tabs.contains_key(&doc), "la scheda resta aperta");
        assert!(app.groups.contains(doc));
        assert!(app.workspace.get(doc).unwrap().is_dirty(), "resta sporca");

        app.write_doc(doc).unwrap();

        let riletta = store.load(nota.path()).unwrap();
        assert_eq!(riletta.note.markdown, "testo aggiornato");
        assert_eq!(riletta.note.header.id, nota.note.header.id, "stesso id");
        assert_eq!(riletta.note.header.title, "titolo originale");
        let su_disco = std::fs::read_to_string(nota.path()).unwrap();
        assert!(
            su_disco.starts_with("+++"),
            "l'intestazione sopravvive: {su_disco:?}"
        );
        assert!(!app.workspace.get(doc).unwrap().is_dirty());
        assert!(
            app.notes.as_ref().unwrap().loaded.is_empty(),
            "salvare a pannello chiuso non ripopola la cache"
        );
    }

    #[test]
    fn scarico_cache_salvataggi_ripetuti_aggiornano_la_guardia() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "ripetuto");
        scrivi(&mut app, doc, "uno");
        app.unload_notes_view_cache();
        app.write_doc(doc).unwrap();

        scrivi(&mut app, doc, " due");
        app.write_doc(doc)
            .expect("la seconda scrittura usa l'identita' aggiornata dalla prima");

        assert_eq!(corpo_su_disco(&store, &nota), "uno due");
    }

    #[test]
    fn scarico_cache_rifiuta_ancora_la_modifica_esterna() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "conteso");
        scrivi(&mut app, doc, "mia modifica");
        app.unload_notes_view_cache();

        // Un altro programma riscrive il file dopo lo scarico.
        let mut esterna = std::fs::read_to_string(nota.path()).unwrap();
        esterna.push_str("modifica esterna piu' lunga\n");
        std::fs::write(nota.path(), &esterna).unwrap();

        let errore = app.write_doc(doc).unwrap_err();
        assert!(errore.contains("fallito"), "{errore}");
        assert_eq!(
            std::fs::read_to_string(nota.path()).unwrap(),
            esterna,
            "il file dell'altro programma non e' stato schiacciato"
        );
        assert!(
            app.workspace.get(doc).unwrap().is_dirty(),
            "la mia modifica non e' andata persa"
        );
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "mia modifica"
        );
    }

    #[test]
    fn scarico_cache_non_scrive_una_nota_a_sola_lettura() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let percorso = cartella.path().join("rotta.md");
        let originale = b"+++\nid = [\n+++\ncorpo";
        std::fs::write(&percorso, originale).unwrap();
        let nota = store.load(&percorso).unwrap();
        assert!(nota.read_only_error.is_some(), "il file e' in sola lettura");
        let doc = app.open_loaded_note(nota);
        scrivi(&mut app, doc, " extra");
        app.unload_notes_view_cache();

        assert!(app.write_doc(doc).is_err());
        assert_eq!(
            std::fs::read(&percorso).unwrap(),
            originale,
            "il file illeggibile non viene toccato"
        );
    }

    #[test]
    fn scarico_cache_tiene_la_guardia_solo_per_le_schede_aperte() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (aperta, _) = apri_nota(&mut app, &store, "aperta");
        let (chiusa, _) = apri_nota(&mut app, &store, "chiusa");
        app.close_tab(chiusa);

        app.unload_notes_view_cache();

        let notes = app.notes.as_ref().unwrap();
        assert_eq!(
            notes.open_saves.len(),
            1,
            "una sola guardia: la scheda aperta"
        );
        assert!(notes.tabs.contains_key(&aperta));

        // chiudere la scheda libera anche la guardia
        app.close_tab(aperta);
        assert!(app.notes.as_ref().unwrap().open_saves.is_empty());
    }

    #[test]
    fn scarico_cache_senza_appunti_non_fa_nulla() {
        let (mut app, _) = Lume::new_without_store();
        app.unload_notes_view_cache();
        assert!(app.notes.is_none(), "nessuno stato allocato dallo scarico");
    }

    #[test]
    fn scarico_cache_la_scansione_in_volo_non_ripopola_a_pannello_chiuso() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, generation) = app_con_appunti(cartella.path());
        let _ = apri_nota(&mut app, &store, "nota");
        app.unload_notes_view_cache();

        // la scansione partita prima della chiusura arriva dopo
        let riepiloghi = store.scan().unwrap();
        let _ = app.update(Message::NotesScanned(
            generation,
            cartella.path().to_path_buf(),
            Ok((store.clone(), riepiloghi)),
        ));

        assert!(app.notes.as_ref().unwrap().summaries.is_empty());
    }

    #[test]
    fn scarico_cache_una_nota_aperta_a_pannello_chiuso_non_ripopola_la_cache() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        app.unload_notes_view_cache();
        let generation = app.next_notes_generation().unwrap();
        let nota = store.create("creata a pannello chiuso").unwrap();

        let _ = app.update(Message::NotesOpened(generation, Ok((nota.clone(), None))));

        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.tabs.len(), 1, "la scheda si apre");
        assert!(notes.loaded.is_empty(), "niente cache");
        assert!(notes.summaries.is_empty(), "niente riepiloghi");
        let doc = *notes.tabs.keys().next().unwrap();
        scrivi(&mut app, doc, "scritto");
        app.write_doc(doc).unwrap();
        assert_eq!(corpo_su_disco(&store, &nota), "scritto");
    }

    #[test]
    fn avviso_cartella_della_nota_creata_arriva_in_status_bar_e_la_nota_si_apre() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let generation = app.next_notes_generation().unwrap();
        let nota = store.create("nuova").unwrap();
        // Testo iniettato: un fsync di cartella non si fa fallire su richiesta.
        let avviso = crate::note_links::avviso_creazione("nuova", &"EIO");

        let _ = app.update(Message::NotesOpened(generation, Ok((nota, Some(avviso)))));

        assert_eq!(
            app.notes.as_ref().unwrap().tabs.len(),
            1,
            "la scheda si apre"
        );
        let (_, notice) = app.notice.as_ref().expect("l'avviso in status bar");
        assert!(matches!(notice.kind, NoticeKind::Error));
        assert!(
            notice
                .text
                .contains("«nuova» creato, ma la cartella non è stata sincronizzata"),
            "{}",
            notice.text
        );
    }

    #[test]
    fn reopen_note_la_riapertura_riscansiona_e_non_duplica_la_scheda() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "gia' aperta");
        let id = nota.note.header.id.clone();
        scrivi(&mut app, doc, "bozza");
        app.unload_notes_view_cache();
        assert!(app.notes.as_ref().unwrap().summaries.is_empty());

        // riapertura del pannello: i riepiloghi tornano dalla scansione vera
        let riapertura = app.registry.click(tools::notes::ID);
        let _task = app.apply_tool_requests(riapertura);
        let generation = app.notes.as_ref().unwrap().scan_request;
        let (store_nuovo, riepiloghi) = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(notes_scan_blocking(cartella.path().to_path_buf()))
            .unwrap();
        let _ = app.update(Message::NotesScanned(
            generation,
            cartella.path().to_path_buf(),
            Ok((store_nuovo, riepiloghi)),
        ));
        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.summaries.len(), 1, "i riepiloghi sono stati riletti");
        assert_eq!(notes.summaries[0].id, id);
        assert!(
            notes.loaded.is_empty(),
            "la riapertura non ricarica i corpi"
        );

        // cliccare il riepilogo della nota gia' aperta attiva la stessa scheda
        let _ = app.open_note(id.clone());
        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.tabs.len(), 1, "nessuna scheda duplicata");
        assert_eq!(notes.tabs.get(&doc), Some(&id));
        assert_eq!(app.groups.all().filter(|d| *d == doc).count(), 1);
        assert_eq!(app.groups.active(), Some(doc));
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "bozza",
            "la bozza non salvata non viene rimpiazzata dal disco"
        );
        assert!(
            app.notes.as_ref().unwrap().loaded.is_empty(),
            "attivare una scheda aperta non rilegge il file"
        );
    }

    #[test]
    fn reopen_note_il_risultato_di_un_apri_non_duplica_la_scheda() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "doppia");
        scrivi(&mut app, doc, "bozza");
        app.unload_notes_view_cache();
        let generation = app.next_notes_generation().unwrap();

        // un caricamento gia' in volo consegna la stessa nota
        let _ = app.update(Message::NotesOpened(
            generation,
            Ok((store.load(nota.path()).unwrap(), None)),
        ));

        assert_eq!(app.notes.as_ref().unwrap().tabs.len(), 1);
        assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), "bozza");
        scrivi(&mut app, doc, "!");
        app.write_doc(doc).unwrap();
        assert_eq!(corpo_su_disco(&store, &nota), "bozza!");
    }

    #[test]
    fn reopen_note_un_apri_in_volo_non_nasconde_una_modifica_esterna() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "esterna");
        scrivi(&mut app, doc, "mio");
        app.unload_notes_view_cache();

        // il file cambia sul disco e un apri in volo porta l'identita' nuova
        let mut esterna = std::fs::read_to_string(nota.path()).unwrap();
        esterna.push_str("altro programma\n");
        std::fs::write(nota.path(), &esterna).unwrap();
        let generation = app.next_notes_generation().unwrap();
        let _ = app.update(Message::NotesOpened(
            generation,
            Ok((store.load(nota.path()).unwrap(), None)),
        ));

        assert!(
            app.write_doc(doc).is_err(),
            "la guardia resta quella dell'apertura: il conflitto si vede ancora"
        );
        assert_eq!(std::fs::read_to_string(nota.path()).unwrap(), esterna);
    }

    #[test]
    fn reopen_note_chiudere_il_pannello_dal_registro_scarica_la_cache() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, _) = apri_nota(&mut app, &store, "dal registro");
        let aperture = app.registry.click(tools::notes::ID);
        assert!(
            aperture
                .iter()
                .any(|(_, r)| matches!(r, lume_ui::tool::ToolRequest::NotesLoad))
        );

        let chiusura = app.registry.click(tools::notes::ID);

        assert!(
            matches!(
                chiusura.as_slice(),
                [(id, lume_ui::tool::ToolRequest::NotesUnload)] if *id == tools::notes::ID
            ),
            "la chiusura chiede esattamente lo scarico: {chiusura:?}"
        );
        let _ = app.apply_tool_requests(chiusura);
        let notes = app.notes.as_ref().unwrap();
        assert!(notes.loaded.is_empty());
        assert!(notes.summaries.is_empty());
        assert!(notes.tabs.contains_key(&doc));
        assert!(
            notes.store.is_some(),
            "lo store resta finche' il lifecycle vive"
        );
        assert!(notes.directory.is_some());
    }

    #[test]
    fn scarico_cache_nuovo_appunto_a_pannello_chiuso_non_lascia_riepiloghi() {
        let cartella = tempfile::tempdir().unwrap();
        let store = NoteStore::open(cartella.path()).unwrap();
        let (mut app, _) = Lume::new_without_store();
        assert!(!app.registry.is_open(tools::notes::ID));

        // scorciatoia «Nuovo appunto» con Appunti mai aperti
        let _ = app.update(Message::NewNote);
        let generation = app.notes_generation().unwrap();
        let riepiloghi = store.scan().unwrap();
        let _ = app.update(Message::NotesScanned(
            generation,
            cartella.path().to_path_buf(),
            Ok((store.clone(), riepiloghi)),
        ));
        let notes = app.notes.as_ref().unwrap();
        assert!(notes.store.is_some(), "lo store serve a creare la nota");
        assert!(
            notes.summaries.is_empty(),
            "nessuna lista a pannello chiuso"
        );

        // la nota creata si apre in una scheda e si salva, senza cache
        let generation = app.notes_generation().unwrap();
        let nota = store.create("da scorciatoia").unwrap();
        let _ = app.update(Message::NotesOpened(generation, Ok((nota.clone(), None))));
        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.tabs.len(), 1);
        assert!(notes.loaded.is_empty());
        assert!(notes.summaries.is_empty());
        let doc = *notes.tabs.keys().next().unwrap();
        scrivi(&mut app, doc, "appena scritto");
        app.write_doc(doc).unwrap();
        assert_eq!(corpo_su_disco(&store, &nota), "appena scritto");
    }

    /// Fotografia di ciò che il tool Appunti sta mostrando (via `Event::Probe`).
    fn foto_tool(app: &mut Lume) -> tools::notes::Snapshot {
        let (invio, ricezione) = std::sync::mpsc::channel();
        let _ = app
            .registry
            .get_mut(tools::notes::ID)
            .expect("il tool Appunti e' registrato")
            .update(&ToolEvent::new(tools::notes::Event::Probe(invio)));
        ricezione.try_recv().expect("il tool risponde alla sonda")
    }

    /// Riapre il pannello come farebbe il clic sulla barra: il registro passa
    /// le richieste all'app, che avvia la riscansione (il task non si esegue:
    /// la consegna del risultato e' a mano, nell'ordine che il test vuole).
    fn riapri_pannello(app: &mut Lume) {
        let richieste = app.registry.click(tools::notes::ID);
        let _task = app.apply_tool_requests(richieste);
    }

    /// Consuma tutto il Task, anche quando non produce messaggi.
    fn consuma_task_appunti(task: Task<Message>) -> Vec<Message> {
        use futures::StreamExt;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut messaggi = Vec::new();
            if let Some(mut stream) = iced_runtime::task::into_stream(task) {
                while let Some(action) = stream.next().await {
                    if let iced_runtime::Action::Output(message) = action {
                        messaggi.push(message);
                    }
                }
            }
            messaggi
        })
    }

    /// Consegna i risultati reali e i seguiti Appunti; le scadenze degli
    /// avvisi non sono eventi della sequenza che si vuole intercalare.
    fn consegna_task_appunti(app: &mut Lume, task: Task<Message>) {
        let mut coda: VecDeque<Message> = consuma_task_appunti(task).into();
        while let Some(messaggio) = coda.pop_front() {
            if matches!(
                messaggio,
                Message::NotesPreferenceLoaded(..)
                    | Message::NotesScanned(..)
                    | Message::NotesOpened(..)
                    | Message::NotesReadFinished(..)
                    | Message::NotesDirectorySaved(..)
                    | Message::RestoredTabReadFinished(..)
                    | Message::RestoredTabAction(..)
            ) {
                let seguito = app.update(messaggio);
                coda.extend(consuma_task_appunti(seguito));
            }
        }
    }

    #[test]
    fn cambio_cartella_annulla_il_solo_intento_pendente_con_un_avviso() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(a.path());
        let store_a = NoteStore::open(a.path()).unwrap();
        let store_b = NoteStore::open(b.path()).unwrap();
        let task = app.update(Message::NewNote);
        let preferenza = consuma_task_appunti(task).pop().unwrap();
        let task = app.update(preferenza);
        let scansione_a = consuma_task_appunti(task).pop().unwrap();
        let vecchia = app.notes_scan_generation().unwrap();
        let task = app.scan_notes_directory(vecchia, a.path().to_path_buf());
        let altra_scansione_a = consuma_task_appunti(task).pop().unwrap();
        let prima = app.notice_seq;

        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        let avviso = app.notice.as_ref().map(|(_, n)| n.text.clone());
        let dopo_cambio = app.notice_seq;
        let task = app.update(scansione_a);
        consegna_task_appunti(&mut app, task);
        assert_eq!(app.notice_seq, dopo_cambio);
        consegna_task_appunti(&mut app, cambio);
        let numero_dopo_cambio = store_b.scan().unwrap().len();
        let task = app.update(altra_scansione_a);
        consegna_task_appunti(&mut app, task);
        let task = app.update(Message::NotesScanned(
            vecchia,
            PathBuf::new(),
            Err("vecchio".into()),
        ));
        consegna_task_appunti(&mut app, task);
        let dopo_vecchi = app.notice_seq;
        let notes = app.notes.as_ref().unwrap();
        let annullato = !notes.create_after_load;
        let cache_scarica =
            notes.view_unloaded && notes.summaries.is_empty() && notes.loaded.is_empty();
        let cartella = notes.directory.clone();
        let generazione = notes.scan_generation;

        assert_eq!(numero_dopo_cambio, 0, "l'intento annullato non crea in B");
        assert_eq!(store_a.scan().unwrap().len(), 0);
        assert!(annullato && cache_scarica);
        assert_eq!(cartella.as_deref(), Some(b.path()));
        assert_ne!(generazione, vecchia);
        assert_eq!(app.notes_directory, b.path());
        assert_eq!(dopo_cambio, prima + 1);
        assert_eq!(
            dopo_vecchi, dopo_cambio,
            "nessun avviso duplicato dai risultati vecchi"
        );
        assert!(
            avviso.is_some_and(|testo| testo.contains("annullato") && testo.contains("cartella"))
        );
        assert_ne!(app.persistence_error.as_deref(), Some("vecchio"));

        let task = app.update(Message::NewNote);
        consegna_task_appunti(&mut app, task);
        assert_eq!(
            store_b.scan().unwrap().len(),
            1,
            "la nuova azione crea in B"
        );
        assert_eq!(store_a.scan().unwrap().len(), 0);
        assert_eq!(app.notes.as_ref().unwrap().tabs.len(), 1);
        assert!(app.notes.as_ref().unwrap().view_unloaded);
        let richieste = app.registry.click(tools::notes::ID);
        let riapertura = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, riapertura);
        let foto = foto_tool(&mut app);
        assert!(!foto.loading);
        assert_eq!(foto.directory.as_deref(), Some(b.path()));
        assert_eq!(
            foto.summaries, 1,
            "la riapertura reale mostra la nuova nota"
        );
    }

    #[test]
    fn scelta_corrente_prevale_sulla_preferenza_non_ancora_salvata() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(a.path());
        let store_a = NoteStore::open(a.path()).unwrap();
        let store_b = NoteStore::open(b.path()).unwrap();
        app.begin_notes_load().unwrap();
        // Il Task del cambio non è ancora consumato: SQLite contiene davvero A.
        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        let nuova_azione = app.update(Message::NewNote);
        consegna_task_appunti(&mut app, nuova_azione);
        let conteggio_a = store_a.scan().unwrap().len();
        let conteggio_b = store_b.scan().unwrap().len();
        let corrente = app.notes_scan_generation();
        consegna_task_appunti(&mut app, cambio);

        assert_eq!(
            conteggio_a, 0,
            "non creare nella preferenza ancora vecchia A"
        );
        assert_eq!(conteggio_b, 1, "la nuova azione usa subito la scelta B");
        assert_eq!(store_a.scan().unwrap().len(), 0);
        assert_eq!(store_b.scan().unwrap().len(), 1);
        assert_eq!(app.notes_directory, b.path());
        assert_eq!(app.notes_scan_generation(), corrente);
        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.directory.as_deref(), Some(b.path()));
        assert_eq!(notes.store.as_ref().unwrap().scan().unwrap().len(), 1);
        assert!(notes.view_unloaded && notes.summaries.is_empty() && notes.loaded.is_empty());
        assert_eq!(notes.tabs.len(), 1);
    }

    #[test]
    fn riapertura_durante_persistenza_usa_la_scelta_corrente() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let store_b = NoteStore::open(b.path()).unwrap();
        let _ = store_b.create("presente in B").unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(a.path());
        app.begin_notes_load().unwrap();
        // La scrittura della preferenza e la prima scansione non sono avviate.
        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        let richieste = app.registry.click(tools::notes::ID);
        let riapertura = app.apply_tool_requests(richieste);
        let scansione = consuma_task_appunti(riapertura).pop().unwrap();
        assert!(matches!(&scansione, Message::NotesScanned(_, p, Ok(_)) if p == b.path()));
        let task = app.update(scansione);
        consegna_task_appunti(&mut app, task);
        let foto = foto_tool(&mut app);
        assert!(!foto.loading);
        assert_eq!(foto.directory.as_deref(), Some(b.path()));
        assert_eq!(foto.summaries, 1);
        let corrente = app.notes_scan_generation();
        consegna_task_appunti(&mut app, cambio);
        assert_eq!(app.notes_scan_generation(), corrente);
        assert_eq!(foto_tool(&mut app), foto);
        assert_eq!(app.notes_directory, b.path());
    }

    #[test]
    fn cambio_cartella_mette_subito_il_pannello_in_caricamento() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (mut app, store_a, _) = app_con_appunti(a.path());
        let nota = store_a.create("nota A").unwrap();
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        let task =
            app.scan_notes_directory(app.notes_scan_generation().unwrap(), a.path().to_path_buf());
        consegna_task_appunti(&mut app, task);
        let prima = foto_tool(&mut app);
        assert_eq!(prima.summaries, 1);
        assert!(!prima.loading);

        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        let durante = foto_tool(&mut app);
        consegna_task_appunti(&mut app, cambio);
        let dopo = foto_tool(&mut app);
        assert!(
            durante.loading,
            "la vista deve entrare in caricamento subito"
        );
        assert_eq!(durante.summaries, 0, "la nota A non è più offerta");
        assert_eq!(durante.directory.as_deref(), Some(b.path()));
        assert!(!dopo.loading);
        assert_eq!(dopo.summaries, 0);
        assert_eq!(dopo.directory.as_deref(), Some(b.path()));
        assert_eq!(store_a.scan().unwrap().len(), 1);
        assert!(nota.path().exists());
    }

    #[test]
    fn gesti_del_pannello_gia_in_viaggio_durante_il_cambio_avvisano() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (mut app, store_a, _) = app_con_appunti(a.path());
        let nota = store_a.create("nota A").unwrap();
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        let task =
            app.scan_notes_directory(app.notes_scan_generation().unwrap(), a.path().to_path_buf());
        consegna_task_appunti(&mut app, task);
        let origin = lume_ui::tool::NotesOrigin(app.notes_scan_generation().unwrap());
        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        let prima = app.notice_seq;
        let create = app.update(Message::Tool(
            tools::notes::ID,
            ToolEvent::new(tools::notes::Event::Create(origin)),
        ));
        assert_eq!(app.notice_seq, prima + 1);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("cambiata"))
        );
        consegna_task_appunti(&mut app, create);
        let open = app.update(Message::Tool(
            tools::notes::ID,
            ToolEvent::new(tools::notes::Event::Open(
                origin,
                nota.note.header.id.clone(),
            )),
        ));
        assert_eq!(app.notice_seq, prima + 2);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("cambiata"))
        );
        consegna_task_appunti(&mut app, open);
        consegna_task_appunti(&mut app, cambio);
        assert_eq!(store_a.scan().unwrap().len(), 1);
        assert_eq!(NoteStore::open(b.path()).unwrap().scan().unwrap().len(), 0);
        assert!(app.notes.as_ref().unwrap().tabs.is_empty());
    }

    #[test]
    fn cambio_rifiutato_per_nota_sporca_conserva_intento_e_generazioni() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let store_a = NoteStore::open(a.path()).unwrap();
        let esistente = store_a.create("da modificare").unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(a.path());
        let task = app.update(Message::NewNote);
        let preferenza = consuma_task_appunti(task).pop().unwrap();
        // Una nota reale è aperta mentre la preferenza è in viaggio.
        let doc = app.open_loaded_note(esistente);
        scrivi(&mut app, doc, "modifica");
        let scan = app.notes_scan_generation();
        let apertura = app.notes_generation();
        let directory = app.notes_directory.clone();
        let scelta = app.notes_directory_scelta.clone();
        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text == Lume::APPUNTI_SPORCHI)
        );
        assert_eq!(app.notes_scan_generation(), scan);
        assert_eq!(app.notes_generation(), apertura);
        assert_eq!(app.notes_directory, directory);
        assert_eq!(app.notes_directory_scelta, scelta);
        let notes = app.notes.as_ref().unwrap();
        assert!(notes.create_after_load && notes.store.is_none());
        assert!(notes.tabs.contains_key(&doc));
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        consegna_task_appunti(&mut app, cambio);
        let task = app.update(preferenza);
        consegna_task_appunti(&mut app, task);
        assert_eq!(
            store_a.scan().unwrap().len(),
            2,
            "il vecchio intento rimane valido in A"
        );
        assert_eq!(NoteStore::open(b.path()).unwrap().scan().unwrap().len(), 0);
        assert!(app.workspace.get(doc).unwrap().is_dirty());
    }

    #[test]
    fn scelta_esplicita_senza_db_usa_la_cartella_scelta() {
        // Solo caratterizzazione dopo la correzione: nessun Task apre il default.
        let b = tempfile::tempdir().unwrap();
        let (mut app, _) = Lume::new_without_store();
        app.begin_notes_load().unwrap();
        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        let task = app.update(Message::NewNote);
        consegna_task_appunti(&mut app, task);
        consegna_task_appunti(&mut app, cambio);
        assert_eq!(NoteStore::open(b.path()).unwrap().scan().unwrap().len(), 1);
        assert_eq!(app.notes_directory, b.path());
        assert_eq!(
            app.notes.as_ref().unwrap().directory.as_deref(),
            Some(b.path())
        );
        assert!(app.notes.as_ref().unwrap().view_unloaded);
    }

    /// Clicca il widget effettivamente costruito dal tool, trattenendo il
    /// messaggio Iced pubblicato prima che venga consegnato all'app.
    fn gesto_dalla_vista(app: &Lume, quale: &str) -> ToolEvent {
        use iced::advanced::{Layout, Shell, layout, widget::Tree};
        let theme = lume_theme::LumeTheme::default();
        let ctx = ToolCtx {
            theme: &theme,
            active_path: None,
            body_height: None,
            file_icons: None,
            open_diff: None,
        };
        let (x, y) = match quale {
            "crea" => (335.0, 14.0),
            "apri" => (40.0, 50.0),
            "elimina" => (345.0, 50.0),
            _ => panic!("gesto sconosciuto"),
        };
        let tool = app.registry.get(tools::notes::ID).unwrap();
        let mut view = tool.view(&ctx);
        let mut tree = Tree::new(&view);
        let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
            iced::Font::default(),
            iced::Pixels(16.0),
        ));
        let size = iced::Size::new(360.0, 500.0);
        let node = view.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, size),
        );
        let mut published = Vec::new();
        for event in [
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Left,
            )),
        ] {
            let mut shell = Shell::new(&mut published);
            view.as_widget_mut().update(
                &mut tree,
                &event,
                Layout::new(&node),
                iced::mouse::Cursor::Available(iced::Point::new(x, y)),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &iced::Rectangle::new(iced::Point::ORIGIN, size),
            );
        }
        published
            .into_iter()
            .find(|event| {
                matches!(
                    (quale, event.downcast::<tools::notes::Event>()),
                    ("crea", Some(tools::notes::Event::Create(_)))
                        | ("apri", Some(tools::notes::Event::Open(_, _)))
                        | ("elimina", Some(tools::notes::Event::Delete(_, _)))
                )
            })
            .unwrap_or_else(|| panic!("nessun gesto {quale} dal widget a {x},{y}; layout {node:?}"))
    }

    fn prepara_gesto_e_cambia_cartella(
        quale: &str,
    ) -> (
        Lume,
        tempfile::TempDir,
        tempfile::TempDir,
        ToolEvent,
        PathBuf,
    ) {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (mut app, store_a, _) = app_con_appunti(a.path());
        let nota = store_a.create("nota condivisa").unwrap();
        let copia = b.path().join(nota.path().file_name().unwrap());
        std::fs::copy(nota.path(), &copia).unwrap();
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        let task =
            app.scan_notes_directory(app.notes_scan_generation().unwrap(), a.path().to_path_buf());
        consegna_task_appunti(&mut app, task);
        let gesto = gesto_dalla_vista(&app, quale);
        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        consegna_task_appunti(&mut app, cambio);
        assert_eq!(foto_tool(&mut app).summaries, 1);
        (app, a, b, gesto, copia)
    }

    #[test]
    fn gesto_crea_della_vista_vecchia_non_crea_nella_cartella_nuova() {
        let (mut app, _a, b, gesto, copia) = prepara_gesto_e_cambia_cartella("crea");
        let prima = app.notice_seq;
        let task = app.update(Message::Tool(tools::notes::ID, gesto));
        consegna_task_appunti(&mut app, task);
        assert_eq!(NoteStore::open(b.path()).unwrap().scan().unwrap().len(), 1);
        assert!(copia.exists());
        assert_eq!(app.notice_seq, prima + 1);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.kind == NoticeKind::Error && n.text.contains("cambiata"))
        );
    }

    #[test]
    fn gesto_apri_della_vista_vecchia_non_apre_copia_con_stesso_id() {
        let (mut app, _a, b, gesto, copia) = prepara_gesto_e_cambia_cartella("apri");
        let prima = app.notice_seq;
        let task = app.update(Message::Tool(tools::notes::ID, gesto));
        consegna_task_appunti(&mut app, task);
        assert!(app.workspace.get_by_path(&copia).is_none());
        assert_eq!(NoteStore::open(b.path()).unwrap().scan().unwrap().len(), 1);
        assert_eq!(app.notice_seq, prima + 1);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.kind == NoticeKind::Error && n.text.contains("cambiata"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn gesto_elimina_della_vista_vecchia_non_elimina_copia_con_stesso_id() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "gesto_elimina_della_vista_vecchia_non_elimina_copia_con_stesso_id",
            false,
        ) {
            return;
        }
        let (mut app, _a, b, gesto, copia) = prepara_gesto_e_cambia_cartella("elimina");
        let prima = app.notice_seq;
        let task = app.update(Message::Tool(tools::notes::ID, gesto));
        let mut coda: VecDeque<Message> = consuma_task_appunti(task).into();
        while let Some(messaggio) = coda.pop_front() {
            if matches!(
                messaggio,
                Message::NotesDeletePrepared(..) | Message::NotesDeleted(..)
            ) {
                let seguito = app.update(messaggio);
                coda.extend(consuma_task_appunti(seguito));
            }
        }
        assert!(copia.exists(), "la copia in B non va eliminata");
        assert_eq!(NoteStore::open(b.path()).unwrap().scan().unwrap().len(), 1);
        assert_eq!(app.notice_seq, prima + 1);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.kind == NoticeKind::Error && n.text.contains("cambiata"))
        );
    }

    #[test]
    fn ripristino_tardivo_non_rimette_lo_store_vecchio_dopo_il_cambio() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let store_a = NoteStore::open(a.path()).unwrap();
        let nota = store_a.create("nota ripristinata").unwrap();
        let originali = std::fs::read(nota.path()).unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(a.path());
        app.begin_notes_load().unwrap();
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![nota.path().to_path_buf()];
        snapshot.active_tab = Some(nota.path().to_path_buf());
        let lettura = app.restore_snapshot_async(snapshot);
        let esito = consuma_task_appunti(lettura)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        let task = app.update(esito);
        consegna_task_appunti(&mut app, task);
        let nuova = app.update(Message::NewNote);
        consegna_task_appunti(&mut app, nuova);
        let conteggio_a = store_a.scan().unwrap().len();
        let conteggio_b = NoteStore::open(b.path()).unwrap().scan().unwrap().len();
        consegna_task_appunti(&mut app, cambio);
        assert_eq!(conteggio_a, 1, "il ripristino non deve creare in A");
        assert_eq!(conteggio_b, 1);
        assert_eq!(std::fs::read(nota.path()).unwrap(), originali);
        assert_eq!(app.notes_directory, b.path());
        assert_eq!(
            app.notes.as_ref().unwrap().directory.as_deref(),
            Some(b.path())
        );
        let doc = app
            .workspace
            .get_by_path(nota.path())
            .expect("placeholder protetto");
        assert!(app.pending_restored_tabs.contains(doc));
        let tentativo = app.activate_tab(doc);
        consegna_task_appunti(&mut app, tentativo);
        let apertura = app.open_file_task(
            nota.path().to_path_buf(),
            crate::file_read::DopoApertura::Niente,
        );
        let risposta = consuma_task_appunti(apertura)
            .into_iter()
            .find(|m| matches!(m, Message::FileReadFinished(..)))
            .unwrap();
        let task = app.update(risposta);
        consegna_task_appunti(&mut app, task);
        let salvataggio = app.update(Message::SaveActive);
        consegna_task_appunti(&mut app, salvataggio);
        assert!(
            app.pending_restored_tabs.contains(doc),
            "nessun reader generico materializza la nota"
        );
        assert_eq!(std::fs::read(nota.path()).unwrap(), originali);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, notice)| notice.text.contains("cartella"))
        );
        // Tornando ad A si può riprendere la lettura con la guardia NoteStore.
        let ritorno = app.update(Message::NotesDirectoryPicked(Some(a.path().to_path_buf())));
        consegna_task_appunti(&mut app, ritorno);
        let task = app.activate_tab(doc);
        let messaggio = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        let task = app.update(messaggio);
        consegna_task_appunti(&mut app, task);
        assert!(!app.pending_restored_tabs.contains(doc));
        assert!(matches!(
            app.editor_kind(doc),
            crate::editor_groups::EditorKind::Note(_)
        ));
        scrivi(&mut app, doc, "corpo nuovo");
        app.write_doc(doc).unwrap();
        assert!(
            std::fs::read_to_string(nota.path())
                .unwrap()
                .starts_with("+++\n")
        );
        assert!(
            store_a
                .load(nota.path())
                .unwrap()
                .note
                .markdown
                .contains("corpo nuovo")
        );
    }

    #[test]
    fn placeholder_inattivo_della_vecchia_cartella_non_diventa_markdown_ordinario() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let store = NoteStore::open(a.path()).unwrap();
        let nota = store.create("nota non ancora letta").unwrap();
        let originali = std::fs::read(nota.path()).unwrap();
        let codice = a.path().join("codice.rs");
        std::fs::write(&codice, "fn main() {}\n").unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(a.path());
        // Come l'avvio reale, la preferenza è già nota prima dei placeholder.
        app.notes_directory = a.path().to_path_buf();
        app.begin_notes_load().unwrap();
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![nota.path().to_path_buf(), codice.clone()];
        snapshot.active_tab = Some(codice);
        let task = app.restore_snapshot_async(snapshot);
        let lettura = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        let task = app.update(lettura);
        consegna_task_appunti(&mut app, task);
        let doc = app.workspace.get_by_path(nota.path()).unwrap();
        assert!(app.pending_restored_tabs.contains(doc));
        let task = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        consegna_task_appunti(&mut app, task);
        let task = app.activate_tab(doc);
        consegna_task_appunti(&mut app, task);
        let task = app.update(Message::SaveActive);
        consegna_task_appunti(&mut app, task);
        assert!(app.pending_restored_tabs.contains(doc));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, notice)| notice.text.contains("cartella"))
        );
        assert_eq!(std::fs::read(nota.path()).unwrap(), originali);
        assert_eq!(app.notes_directory, b.path());
    }

    #[cfg(unix)]
    #[test]
    fn placeholder_via_alias_della_vecchia_cartella_non_diventa_markdown() {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("a");
        let alias = radice.path().join("alias");
        let b = radice.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::os::unix::fs::symlink(&a, &alias).unwrap();
        let store = NoteStore::open(&a).unwrap();
        let nota = store.create("nota tramite alias").unwrap();
        let path_alias = alias.join(nota.path().file_name().unwrap());
        let originali = std::fs::read(nota.path()).unwrap();
        let codice = radice.path().join("codice.rs");
        std::fs::write(&codice, "fn main() {}\n").unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(&a);
        app.notes_directory = a.clone();
        app.begin_notes_load().unwrap();
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![path_alias.clone(), codice.clone()];
        snapshot.active_tab = Some(codice);
        let task = app.restore_snapshot_async(snapshot);
        let lettura = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        let task = app.update(lettura);
        consegna_task_appunti(&mut app, task);
        let doc = app.workspace.get_by_path(&path_alias).unwrap();
        let task = app.update(Message::NotesDirectoryPicked(Some(b.clone())));
        consegna_task_appunti(&mut app, task);
        // Anche una lettura generica già richiesta passa al reader dedicato.
        let task = app.open_file_task(path_alias.clone(), crate::file_read::DopoApertura::Niente);
        let risposta = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::FileReadFinished(..)))
            .unwrap();
        let task = app.update(risposta);
        for messaggio in consuma_task_appunti(task) {
            if matches!(messaggio, Message::RestoredTabReadFinished(..)) {
                let task = app.update(messaggio);
                consegna_task_appunti(&mut app, task);
            }
        }
        assert!(
            app.pending_restored_tabs.contains(doc),
            "la nota via alias non diventa un file ordinario"
        );
        assert_eq!(std::fs::read(nota.path()).unwrap(), originali);
        assert_eq!(app.notes_directory, b);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("cartella"))
        );
    }

    #[test]
    fn placeholder_markdown_estraneo_rimane_apribile_dopo_il_cambio() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let estranea = tempfile::tempdir().unwrap();
        let markdown = estranea.path().join("estraneo.md");
        let codice = estranea.path().join("codice.rs");
        std::fs::write(&markdown, "markdown esterno\n").unwrap();
        std::fs::write(&codice, "fn main() {}\n").unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(a.path());
        app.notes_directory = a.path().to_path_buf();
        app.begin_notes_load().unwrap();
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![markdown.clone(), codice.clone()];
        snapshot.active_tab = Some(codice);
        let task = app.restore_snapshot_async(snapshot);
        let lettura = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        let task = app.update(lettura);
        consegna_task_appunti(&mut app, task);
        let doc = app.workspace.get_by_path(&markdown).unwrap();
        let task = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        consegna_task_appunti(&mut app, task);
        let task = app.activate_tab(doc);
        let lettura = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        let task = app.update(lettura);
        consegna_task_appunti(&mut app, task);
        assert!(!app.pending_restored_tabs.contains(doc));
        assert!(matches!(
            app.editor_kind(doc),
            crate::editor_groups::EditorKind::Code
        ));
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "markdown esterno\n"
        );
    }

    /// Consegna anche i messaggi di lettura e persistenza usati nei percorsi
    /// reali di apertura, raccogliendo i salvataggi di sessione completati.
    fn completa_apertura_appunti(app: &mut Lume, task: Task<Message>) -> usize {
        let mut coda: VecDeque<Message> = consuma_task_appunti(task).into();
        let mut sessioni = 0;
        while let Some(messaggio) = coda.pop_front() {
            if matches!(messaggio, Message::SessionSaved(Ok(()))) {
                sessioni += 1;
            }
            if matches!(
                messaggio,
                Message::FileReadFinished(..)
                    | Message::RestoredTabReadFinished(..)
                    | Message::RestoredTabAction(..)
                    | Message::SessionSaved(..)
            ) {
                let seguito = app.update(messaggio);
                coda.extend(consuma_task_appunti(seguito));
            }
        }
        sessioni
    }

    fn app_con_placeholder_inattivo(
        cartella: &Path,
        file: &Path,
        attivo: &Path,
    ) -> (Lume, tempfile::TempDir, DocId) {
        let (mut app, db) = app_nuova_con_cartella_appunti_salvata(cartella);
        app.notes_directory = cartella.to_path_buf();
        app.begin_notes_load().unwrap();
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![file.to_path_buf(), attivo.to_path_buf()];
        snapshot.active_tab = Some(attivo.to_path_buf());
        let task = app.restore_snapshot_async(snapshot);
        completa_apertura_appunti(&mut app, task);
        let doc = app.workspace.get_by_path(file).unwrap();
        assert!(app.pending_restored_tabs.contains(doc));
        (app, db, doc)
    }

    fn riapertura_con_risultato(app: &mut Lume) -> Message {
        let richieste = app.registry.click(tools::notes::ID);
        let chiusura = app.apply_tool_requests(richieste);
        consegna_task_appunti(app, chiusura);
        let richieste = app.registry.click(tools::notes::ID);
        let apertura = app.apply_tool_requests(richieste);
        consuma_task_appunti(apertura)
            .into_iter()
            .find(|m| matches!(m, Message::NotesScanned(..)))
            .unwrap()
    }

    #[test]
    fn riscansioni_inverse_conservano_la_lista_corrente() {
        let radice = tempfile::tempdir().unwrap();
        let dir = radice.path().join("note");
        let (mut app, store, scope) = app_con_appunti(&dir);
        store.create("prima").unwrap();
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        let vecchia = riapertura_con_risultato(&mut app);
        assert!(matches!(vecchia, Message::NotesScanned(_, _, Ok(_))));
        store.create("seconda").unwrap();
        let nuova = riapertura_con_risultato(&mut app);
        let task = app.update(nuova);
        consegna_task_appunti(&mut app, task);
        assert_eq!(foto_tool(&mut app).summaries, 2);
        let task = app.update(vecchia);
        consegna_task_appunti(&mut app, task);
        assert_eq!(foto_tool(&mut app).summaries, 2);
        assert_eq!(app.notes_scan_generation(), Some(scope));
    }

    #[test]
    fn errore_reale_della_riscansione_vecchia_non_sovrascrive_successo_corrente() {
        let radice = tempfile::tempdir().unwrap();
        let dir = radice.path().join("note");
        let spostata = radice.path().join("spostata");
        let (mut app, store, _) = app_con_appunti(&dir);
        store.create("nota").unwrap();
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        std::fs::rename(&dir, &spostata).unwrap();
        std::fs::write(&dir, "non è una cartella").unwrap();
        let errore = riapertura_con_risultato(&mut app);
        assert!(matches!(errore, Message::NotesScanned(_, _, Err(_))));
        std::fs::remove_file(&dir).unwrap();
        std::fs::rename(&spostata, &dir).unwrap();
        let successo = riapertura_con_risultato(&mut app);
        let task = app.update(successo);
        consegna_task_appunti(&mut app, task);
        assert_eq!(foto_tool(&mut app).summaries, 1);
        let prima = app.persistence_error.clone();
        let task = app.update(errore);
        consegna_task_appunti(&mut app, task);
        assert_eq!(app.persistence_error, prima);
        let foto = foto_tool(&mut app);
        assert_eq!(foto.summaries, 1);
        assert!(!foto.loading);
    }

    #[test]
    fn riscansione_non_invalida_salvataggio_o_gesto_della_stessa_libreria() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, scope) = app_con_appunti(dir.path());
        let nota = store.create("nota").unwrap();
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        let scan = app.scan_notes_directory(scope, dir.path().to_path_buf());
        consegna_task_appunti(&mut app, scan);
        let gesto = gesto_dalla_vista(&app, "apri");
        let doc = app.open_loaded_note(nota.clone());
        scrivi(&mut app, doc, "testo salvato");
        use futures::StreamExt;
        let salvataggio = app.update(Message::SaveActive);
        let mut stream = iced_runtime::task::into_stream(salvataggio).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let pronto = runtime.block_on(async {
            loop {
                if let iced_runtime::Action::Output(m @ Message::SaveReady(_)) =
                    stream.next().await.expect("salvataggio senza Ready")
                {
                    break m;
                }
            }
        });
        let scansione = riapertura_con_risultato(&mut app);
        assert_eq!(app.notes_scan_generation(), Some(scope));
        let _ = app.update(pronto);
        let esito = runtime.block_on(async {
            loop {
                if let iced_runtime::Action::Output(m @ Message::SaveCollected(_)) =
                    stream.next().await.expect("salvataggio senza ricevuta")
                {
                    break m;
                }
            }
        });
        let task = app.update(esito);
        app.completa_salvataggi(task);
        let task = app.update(scansione);
        consegna_task_appunti(&mut app, task);
        assert!(
            store
                .load(nota.path())
                .unwrap()
                .note
                .markdown
                .contains("testo salvato")
        );
        assert!(!app.workspace.get(doc).unwrap().is_dirty());
        let prima = app.notice_seq;
        let task = app.update(Message::Tool(tools::notes::ID, gesto));
        consegna_task_appunti(&mut app, task);
        assert_eq!(app.active_doc(), Some(doc));
        assert_eq!(app.notice_seq, prima);
        assert_eq!(
            app.notes.as_ref().unwrap().tabs.get(&doc),
            Some(&nota.note.header.id)
        );
    }

    #[cfg(unix)]
    #[test]
    fn riscansione_non_invalida_eliminazione_della_stessa_libreria() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        if !cestino_isolato(
            "riscansione_non_invalida_eliminazione_della_stessa_libreria",
            false,
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut app, store, scope) = app_con_appunti(dir.path());
        let (doc, nota) = apri_nota(&mut app, &store, "da eliminare");
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        let preparato = risultato_task(app.delete_note(nota.note.header.id.clone()));
        let worker = app.update(preparato);
        let esito = risultato_task(worker);
        assert!(matches!(
            &esito,
            Message::NotesDeleted(_, _, _, _, Ok(DeleteDisposition::Trashed))
        ));
        let scansione = riapertura_con_risultato(&mut app);
        assert_eq!(app.notes_scan_generation(), Some(scope));
        let task = app.update(esito);
        consegna_task_appunti(&mut app, task);
        let task = app.update(scansione);
        consegna_task_appunti(&mut app, task);
        assert!(!nota.path().exists());
        assert!(app.workspace.get(doc).is_none());
        assert!(!app.notes.as_ref().unwrap().tabs.contains_key(&doc));
    }

    fn apertura_pannello_con_placeholder(reader_in_volo: bool) {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("a");
        let b = radice.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let store = NoteStore::open(&a).unwrap();
        let mut nota = store.create("nota").unwrap();
        nota.note.markdown = "prima\nseconda\nterza\n".into();
        let nota = store.save(&nota).unwrap();
        let attivo = radice.path().join("attivo.rs");
        std::fs::write(&attivo, "fn main() {}\n").unwrap();
        let (mut app, _db, doc) = app_con_placeholder_inattivo(&a, nota.path(), &attivo);
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        for dir in [&b, &a] {
            let task = app.update(Message::NotesDirectoryPicked(Some(dir.clone())));
            consegna_task_appunti(&mut app, task);
        }
        let lettura = if reader_in_volo {
            let task = app.open_file_task(
                nota.path().to_path_buf(),
                crate::file_read::DopoApertura::Ricerca { riga: Some(3) },
            );
            let generica = consuma_task_appunti(task)
                .into_iter()
                .find(|m| matches!(m, Message::FileReadFinished(..)))
                .unwrap();
            let task = app.update(generica);
            Some(
                consuma_task_appunti(task)
                    .into_iter()
                    .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
                    .unwrap(),
            )
        } else {
            None
        };
        let gesto = gesto_dalla_vista(&app, "apri");
        let task = app.update(Message::Tool(tools::notes::ID, gesto));
        // Anche il seguito di ricerca può produrre letture e persistenza.
        let mut coda: VecDeque<Message> = consuma_task_appunti(task).into();
        let mut sessioni = 0;
        while let Some(m) = coda.pop_front() {
            if matches!(m, Message::SessionSaved(Ok(()))) {
                sessioni += 1;
            }
            if matches!(
                m,
                Message::NotesReadFinished(..)
                    | Message::FileReadFinished(..)
                    | Message::RestoredTabAction(..)
                    | Message::SessionSaved(..)
            ) {
                coda.extend(consuma_task_appunti(app.update(m)));
            }
        }
        assert!(!app.pending_restored_tabs.contains(doc));
        assert!(!app.restored_notes_candidates.contains_key(&doc));
        assert!(!app.restored_notes_revoked.contains_key(&doc));
        assert!(app.notes.as_ref().unwrap().tabs.contains_key(&doc));
        if let Some(lettura) = lettura {
            let task = app.update(lettura);
            sessioni += completa_apertura_appunti(&mut app, task);
            assert_eq!(sessioni, 1);
        } else {
            let task = app.apply_tool_requests(vec![(
                tools::notes::ID,
                ToolRequest::OpenFileAt {
                    path: nota.path().to_path_buf(),
                    line: 3,
                },
            )]);
            assert_eq!(completa_apertura_appunti(&mut app, task), 0);
            assert_eq!(app.active_doc(), Some(doc));
            assert_eq!(
                app.workspace.get(doc).unwrap().selection(),
                Range::point(14)
            );
            let task = app.open_file_task(
                nota.path().to_path_buf(),
                crate::file_read::DopoApertura::Ricerca { riga: Some(3) },
            );
            assert_eq!(completa_apertura_appunti(&mut app, task), 1);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (snapshot, _) = runtime
            .block_on(app.store.as_ref().unwrap().load_session())
            .unwrap();
        assert_eq!(snapshot.active_tab.as_deref(), Some(nota.path()));
        assert_eq!(app.active_doc(), Some(doc));
        assert_eq!(
            app.workspace.get(doc).unwrap().selection(),
            Range::point(14)
        );
        assert!(
            app.restored_tab_actions
                .get(&doc)
                .is_none_or(VecDeque::is_empty)
        );
    }

    #[test]
    fn pannello_materializza_placeholder_e_apertura_successiva_salta() {
        apertura_pannello_con_placeholder(false);
    }

    #[test]
    fn pannello_materializza_placeholder_con_reader_e_seguito_in_volo() {
        apertura_pannello_con_placeholder(true);
    }

    #[cfg(unix)]
    #[test]
    fn errore_restore_superato_rivalida_prima_di_scartare_il_seguito() {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("a");
        let b = radice.path().join("b");
        let alias = radice.path().join("alias");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::os::unix::fs::symlink(&a, &alias).unwrap();
        let store = NoteStore::open(&a).unwrap();
        let mut nota = store.create("nota").unwrap();
        nota.note.markdown = "prima\nseconda\nterza\n".into();
        let nota = store.save(&nota).unwrap();
        let bytes = std::fs::read(nota.path()).unwrap();
        let attivo = radice.path().join("attivo.rs");
        std::fs::write(&attivo, "fn main() {}\n").unwrap();
        let (mut app, _db, doc) = app_con_placeholder_inattivo(&a, nota.path(), &attivo);
        for dir in [&b, &a] {
            let task = app.update(Message::NotesDirectoryPicked(Some(dir.clone())));
            consegna_task_appunti(&mut app, task);
        }
        let task = app.open_file_task(
            nota.path().to_path_buf(),
            crate::file_read::DopoApertura::Ricerca { riga: Some(3) },
        );
        let generica = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::FileReadFinished(..)))
            .unwrap();
        std::fs::remove_file(nota.path()).unwrap();
        let task = app.update(generica);
        let errore = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        assert!(
            matches!(&errore, Message::RestoredTabReadFinished(_, _, _, _, _, esito) if !esito.is_ok())
        );
        let duplicato = errore.clone();
        for dir in [&b, &alias] {
            let task = app.update(Message::NotesDirectoryPicked(Some(dir.clone())));
            consegna_task_appunti(&mut app, task);
        }
        std::fs::write(nota.path(), &bytes).unwrap();
        let avvisi_prima = app.notice_seq;
        let task = app.update(errore);
        let sessioni = completa_apertura_appunti(&mut app, task);
        assert_eq!(
            app.notice_seq, avvisi_prima,
            "l'errore superato non è mostrato"
        );
        assert!(!app.pending_restored_tabs.contains(doc));
        assert!(matches!(
            app.editor_kind(doc),
            crate::editor_groups::EditorKind::Note(_)
        ));
        assert_eq!(app.active_doc(), Some(doc));
        assert_eq!(
            app.workspace.get(doc).unwrap().selection(),
            Range::point(14)
        );
        assert_eq!(sessioni, 1);
        assert_eq!(
            app.notes.as_ref().unwrap().tabs.get(&doc),
            Some(&nota.note.header.id)
        );
        assert!(!app.restored_notes_candidates.contains_key(&doc));
        assert!(!app.restored_notes_revoked.contains_key(&doc));
        let prima = app.notice_seq;
        let task = app.update(duplicato);
        assert_eq!(completa_apertura_appunti(&mut app, task), 0);
        assert_eq!(app.notice_seq, prima);
        assert_eq!(std::fs::read(nota.path()).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn placeholder_alias_conserva_tutte_le_origini_x_a_b() {
        let radice = tempfile::tempdir().unwrap();
        let x = radice.path().join("x");
        let a = radice.path().join("a");
        let b = radice.path().join("b");
        let alias = radice.path().join("alias-a");
        for dir in [&x, &a, &b] {
            std::fs::create_dir(dir).unwrap();
        }
        std::os::unix::fs::symlink(&a, &alias).unwrap();
        let store = NoteStore::open(&a).unwrap();
        let nota = store.create("nota A").unwrap();
        let originali = std::fs::read(nota.path()).unwrap();
        assert!(originali.starts_with(b"+++\n"));
        let path = alias.join(nota.path().file_name().unwrap());
        let attivo = radice.path().join("attivo.rs");
        std::fs::write(&attivo, "fn main() {}\n").unwrap();
        let (mut app, _db, doc) = app_con_placeholder_inattivo(&x, &path, &attivo);
        for dir in [&a, &b] {
            let task = app.update(Message::NotesDirectoryPicked(Some(dir.to_path_buf())));
            consegna_task_appunti(&mut app, task);
        }
        let task = app.activate_tab(doc);
        completa_apertura_appunti(&mut app, task);
        let prima = app.notice_seq;
        let task = app.update(Message::SaveActive);
        completa_apertura_appunti(&mut app, task);
        assert!(app.notice_seq > prima, "il salvataggio rifiutato avvisa");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("cartella"))
        );
        assert!(!app.scritture.contiene_salvataggio(doc));
        assert_eq!(app.restored_notes_revoked.get(&doc), Some(&a));
        assert!(!app.notes.as_ref().unwrap().tabs.contains_key(&doc));
        assert!(
            app.pending_restored_tabs.contains(doc),
            "nota A protetta dopo X→A→B"
        );
        assert_eq!(std::fs::read(nota.path()).unwrap(), originali);
        assert_eq!(app.notes_directory, b);
        assert!(
            !app.restored_notes_candidates.contains_key(&doc),
            "i candidati si liberano dopo il riconoscimento"
        );
        app.close_tab(doc);
        assert!(!app.restored_notes_revoked.contains_key(&doc));
        assert!(!app.restored_notes_candidates.contains_key(&doc));
    }

    #[cfg(unix)]
    #[test]
    fn ritorno_a_tramite_alias_ripristina_e_salva_la_nota() {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("a");
        let b = radice.path().join("b");
        let alias = radice.path().join("alias-a");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::os::unix::fs::symlink(&a, &alias).unwrap();
        let store = NoteStore::open(&a).unwrap();
        let nota = store.create("nota A").unwrap();
        let attivo = radice.path().join("attivo.rs");
        std::fs::write(&attivo, "fn main() {}\n").unwrap();
        let (mut app, _db, doc) = app_con_placeholder_inattivo(&a, nota.path(), &attivo);
        for dir in [&b, &alias] {
            let task = app.update(Message::NotesDirectoryPicked(Some(dir.to_path_buf())));
            consegna_task_appunti(&mut app, task);
        }
        let task = app.activate_tab(doc);
        completa_apertura_appunti(&mut app, task);
        assert!(
            !app.pending_restored_tabs.contains(doc),
            "alias A è la libreria scelta"
        );
        assert!(matches!(
            app.editor_kind(doc),
            crate::editor_groups::EditorKind::Note(_)
        ));
        assert_eq!(app.notes_directory, alias);
        scrivi(&mut app, doc, "testo salvato");
        app.write_doc(doc).unwrap();
        assert!(
            std::fs::read_to_string(nota.path())
                .unwrap()
                .starts_with("+++\n")
        );
        assert!(
            store
                .load(nota.path())
                .unwrap()
                .note
                .markdown
                .contains("testo salvato")
        );
    }

    #[cfg(unix)]
    #[test]
    fn lettura_in_volo_rivalida_il_ritorno_ad_a_tramite_alias() {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("a");
        let b = radice.path().join("b");
        let alias = radice.path().join("alias-a");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::os::unix::fs::symlink(&a, &alias).unwrap();
        let store = NoteStore::open(&a).unwrap();
        let nota = store.create("nota in volo").unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(&a);
        app.notes_directory = a.clone();
        app.begin_notes_load().unwrap();
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![nota.path().to_path_buf()];
        snapshot.active_tab = Some(nota.path().to_path_buf());
        let task = app.restore_snapshot_async(snapshot);
        let vecchia = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        let duplicato = vecchia.clone();
        for dir in [&b, &alias] {
            let task = app.update(Message::NotesDirectoryPicked(Some(dir.to_path_buf())));
            consegna_task_appunti(&mut app, task);
        }
        let doc = app.workspace.get_by_path(nota.path()).unwrap();
        let task = app.update(vecchia);
        completa_apertura_appunti(&mut app, task);
        assert!(!app.pending_restored_tabs.contains(doc));
        assert!(matches!(
            app.editor_kind(doc),
            crate::editor_groups::EditorKind::Note(_)
        ));
        assert_eq!(app.notes_directory, alias);
        let task = app.update(duplicato);
        completa_apertura_appunti(&mut app, task);
        assert_eq!(store.scan().unwrap().len(), 1);
        assert_eq!(app.notes.as_ref().unwrap().tabs.len(), 1);
        assert!(!app.restored_notes_candidates.contains_key(&doc));
        assert!(!app.restored_notes_revoked.contains_key(&doc));
    }

    #[test]
    fn apertura_rifiutata_non_saltera_alla_riga_al_ritorno_in_a() {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("a");
        let b = radice.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let store = NoteStore::open(&a).unwrap();
        let mut nota = store.create("nota").unwrap();
        nota.note.markdown = "prima\nseconda\nterza\n".to_owned();
        let nota = store.save(&nota).unwrap();
        let attivo = radice.path().join("attivo.rs");
        std::fs::write(&attivo, "fn main() {}\n").unwrap();
        let (mut app, _db, doc) = app_con_placeholder_inattivo(&a, nota.path(), &attivo);
        let task = app.update(Message::NotesDirectoryPicked(Some(b)));
        consegna_task_appunti(&mut app, task);
        let task = app.apply_tool_requests(vec![(
            tools::notes::ID,
            ToolRequest::OpenFileAt {
                path: nota.path().to_path_buf(),
                line: 3,
            },
        )]);
        completa_apertura_appunti(&mut app, task);
        assert!(app.pending_restored_tabs.contains(doc));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("cartella"))
        );
        let task = app.update(Message::NotesDirectoryPicked(Some(a)));
        consegna_task_appunti(&mut app, task);
        let task = app.activate_tab(doc);
        completa_apertura_appunti(&mut app, task);
        assert_eq!(app.active_doc(), Some(doc));
        assert_eq!(app.workspace.get(doc).unwrap().selection(), Range::point(0));
    }

    #[test]
    fn ripristino_codice_non_attende_una_nuova_scansione_appunti() {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("a");
        let b = radice.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let codice = radice.path().join("codice.rs");
        std::fs::write(&codice, "fn main() {}\n").unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(&a);
        app.notes_directory = a;
        app.begin_notes_load().unwrap();
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![codice.clone()];
        snapshot.active_tab = Some(codice.clone());
        let task = app.restore_snapshot_async(snapshot);
        let lettura = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        let doc = app.workspace.get_by_path(&codice).unwrap();
        let task = app.update(Message::NotesDirectoryPicked(Some(b)));
        consegna_task_appunti(&mut app, task);
        let task = app.update(lettura);
        let immediato = !app.pending_restored_tabs.contains(doc);
        let _ = consuma_task_appunti(task);
        assert!(
            immediato,
            "il file di codice non richiede una seconda lettura per Appunti"
        );
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "fn main() {}\n"
        );
    }

    fn apertura_estranea_dopo_il_cambio(ricerca: bool) {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("a");
        let b = radice.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let file = radice.path().join("estraneo.md");
        let attivo = radice.path().join("attivo.rs");
        std::fs::write(&file, "prima\nseconda\nterza\nquarta\n").unwrap();
        std::fs::write(&attivo, "fn main() {}\n").unwrap();
        let (mut app, _db, doc) = app_con_placeholder_inattivo(&a, &file, &attivo);
        let task = app.update(Message::NotesDirectoryPicked(Some(b)));
        consegna_task_appunti(&mut app, task);
        let task = if ricerca {
            app.open_file_task(
                file.clone(),
                crate::file_read::DopoApertura::Ricerca { riga: Some(3) },
            )
        } else {
            app.apply_tool_requests(vec![(
                tools::notes::ID,
                ToolRequest::OpenFileAt {
                    path: file.clone(),
                    line: 3,
                },
            )])
        };
        let sessioni = completa_apertura_appunti(&mut app, task);
        assert!(!app.pending_restored_tabs.contains(doc));
        assert_eq!(app.active_doc(), Some(doc), "la scheda richiesta è attiva");
        assert_eq!(
            app.workspace.get(doc).unwrap().selection(),
            Range::point(14),
            "inizio della terza riga"
        );
        if ricerca {
            assert_eq!(sessioni, 1, "Cerca persiste la sessione una volta");
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let (snapshot, _) = rt
                .block_on(app.store.as_ref().unwrap().load_session())
                .unwrap();
            assert_eq!(snapshot.active_tab.as_deref(), Some(file.as_path()));
        }
    }

    #[test]
    fn open_file_at_su_placeholder_estraneo_attiva_e_salta_alla_riga() {
        apertura_estranea_dopo_il_cambio(false);
    }

    #[test]
    fn ricerca_su_placeholder_estraneo_attiva_salta_e_persiste() {
        apertura_estranea_dopo_il_cambio(true);
    }

    #[test]
    fn riattivazione_prima_del_salvataggio_usa_ancora_la_scelta_b() {
        use crate::features::{Feature, FeatureCommand};
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(a.path());
        app.begin_notes_load().unwrap();
        let cambio = app.update(Message::NotesDirectoryPicked(Some(b.path().to_path_buf())));
        let vecchia_generazione = app.notes_scan_generation().unwrap();
        let task = app.update(Message::Feature(FeatureCommand::SetEnabled(
            Feature::Notes,
            false,
        )));
        consegna_task_appunti(&mut app, task);
        assert!(app.notes.is_none());
        let task = app.update(Message::Feature(FeatureCommand::SetEnabled(
            Feature::Notes,
            true,
        )));
        consegna_task_appunti(&mut app, task);
        let task = app.update(Message::NewNote);
        let messaggio = consuma_task_appunti(task).into_iter().next().unwrap();
        assert!(matches!(&messaggio, Message::NotesScanned(_, path, Ok(_)) if path == b.path()));
        assert_ne!(app.notes_scan_generation(), Some(vecchia_generazione));
        let task = app.update(messaggio);
        consegna_task_appunti(&mut app, task);
        consegna_task_appunti(&mut app, cambio);
        assert_eq!(NoteStore::open(a.path()).unwrap().scan().unwrap().len(), 0);
        assert_eq!(NoteStore::open(b.path()).unwrap().scan().unwrap().len(), 1);
        assert_eq!(app.notes_directory, b.path());
        assert_eq!(
            app.notes.as_ref().unwrap().directory.as_deref(),
            Some(b.path())
        );
    }

    #[test]
    fn gesto_nuovo_della_vista_corrente_crea_una_nota_in_b() {
        let (mut app, _a, b, _vecchio, _) = prepara_gesto_e_cambia_cartella("crea");
        let gesto = gesto_dalla_vista(&app, "crea");
        let task = app.update(Message::Tool(tools::notes::ID, gesto));
        consegna_task_appunti(&mut app, task);
        assert_eq!(NoteStore::open(b.path()).unwrap().scan().unwrap().len(), 2);
        assert_eq!(app.notes.as_ref().unwrap().tabs.len(), 1);
    }

    /// Completa la scelta nel DB, poi attraversa il lifecycle della feature.
    fn cambia_cartella_e_riattiva(app: &mut Lume, nuova: &Path) {
        use crate::features::{Feature, FeatureCommand};
        let task = app.update(Message::NotesDirectoryPicked(Some(nuova.to_path_buf())));
        let messaggi = consuma_task_appunti(task);
        assert!(
            messaggi
                .iter()
                .any(|m| matches!(m, Message::NotesDirectorySaved(Ok(()))))
        );
        assert!(
            messaggi
                .iter()
                .any(|m| matches!(m, Message::NotesScanned(..)))
        );
        for messaggio in messaggi {
            if matches!(messaggio, Message::NotesDirectorySaved(..)) {
                let task = app.update(messaggio);
                let _ = consuma_task_appunti(task);
            }
        }
        let task = app.update(Message::Feature(FeatureCommand::SetEnabled(
            Feature::Notes,
            false,
        )));
        let _ = consuma_task_appunti(task);
        assert!(!app.feature_is_enabled(Feature::Notes));
        assert!(app.notes.is_none());
        let task = app.update(Message::Feature(FeatureCommand::SetEnabled(
            Feature::Notes,
            true,
        )));
        let _ = consuma_task_appunti(task);
        assert!(app.feature_is_enabled(Feature::Notes));
    }

    fn completa_nuovo_appunto(app: &mut Lume, primo_risultato: Message) {
        let scansione = match primo_risultato {
            Message::NotesScanned(..) => primo_risultato,
            preferenza => {
                let task = app.update(preferenza);
                consuma_task_appunti(task)
                    .into_iter()
                    .find(|m| matches!(m, Message::NotesScanned(..)))
                    .expect("scansione corrente")
            }
        };
        let task = app.update(scansione);
        let aperta = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::NotesOpened(..)))
            .expect("creazione corrente");
        let task = app.update(aperta);
        let _ = consuma_task_appunti(task);
    }

    /// La preferenza è richiesta prima del reader, ma consegnata solo dopo
    /// che una riapertura ha completato una scansione della stessa libreria.
    fn preferenza_iniziale_superata_dal_reader(errore_db: bool) {
        let radice = tempfile::tempdir().unwrap();
        let a = radice.path().join("appunti");
        let spostata = radice.path().join("appunti-spostati");
        let store = NoteStore::open(&a).unwrap();
        let nota = store.create("nota ripristinata").unwrap();
        let originali = std::fs::read(nota.path()).unwrap();
        let (mut app, db) = app_nuova_con_cartella_appunti_salvata(&a);
        // In caso di errore SQLite il reader usa il fallback esplicito e
        // temporaneo, senza interrogare il percorso predefinito dell'utente.
        app.notes_directory = a.clone();
        let task = app.update(Message::NewNote);
        if errore_db {
            // La connessione è già aperta: il backend SQLite rileva un file
            // non valido alla lettura, senza errori inventati nel messaggio.
            std::fs::write(db.path().join("session.sqlite3"), b"database non valido").unwrap();
        }
        let preferenza = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::NotesPreferenceLoaded(..)))
            .unwrap();
        if errore_db {
            assert!(matches!(
                &preferenza,
                Message::NotesPreferenceLoaded(_, Err(_))
            ));
        } else {
            assert!(
                matches!(&preferenza, Message::NotesPreferenceLoaded(_, Ok(Some(p))) if p == &a)
            );
        }
        let scope = app.notes_scan_generation().unwrap();
        assert!(app.notes.as_ref().unwrap().create_after_load);
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![nota.path().to_path_buf()];
        snapshot.active_tab = Some(nota.path().to_path_buf());
        let task = app.restore_snapshot_async(snapshot);
        let lettura = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .unwrap();
        assert!(
            matches!(&lettura, Message::RestoredTabReadFinished(_, _, _, _, _, esito) if esito.is_ok())
        );
        let task = app.update(lettura);
        completa_apertura_appunti(&mut app, task);
        let doc = app.workspace.get_by_path(nota.path()).unwrap();
        assert!(!app.pending_restored_tabs.contains(doc));
        assert!(app.notes.as_ref().unwrap().store.is_some());
        assert_eq!(
            app.notes.as_ref().unwrap().directory.as_deref(),
            Some(a.as_path())
        );
        assert_eq!(app.notes_scan_generation(), Some(scope));
        // Il pannello viene aperto, chiuso e riaperto dal registro reale.
        let richieste = app.registry.click(tools::notes::ID);
        let task = app.apply_tool_requests(richieste);
        consegna_task_appunti(&mut app, task);
        let scansione = riapertura_con_risultato(&mut app);
        assert!(matches!(&scansione, Message::NotesScanned(_, p, Ok(_)) if p == &a));
        let task = app.update(scansione);
        consegna_task_appunti(&mut app, task);
        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.scan_generation, scope);
        assert_ne!(notes.scan_request, scope);
        assert!(!notes.create_after_load);
        let richiesta = notes.scan_request;
        let riepiloghi = id_dei_riepiloghi(&app);
        let avviso = app.notice_seq;
        let persistence_error = app.persistence_error.clone();
        let immagine = foto_tool(&mut app);
        let file_nota = nota.path().file_name().unwrap().to_os_string();
        std::fs::rename(&a, &spostata).unwrap();
        let task = app.update(preferenza);
        let risultati = consuma_task_appunti(task);
        assert!(
            !a.exists(),
            "la preferenza superata non deve ricreare la cartella A"
        );
        assert!(
            !risultati
                .iter()
                .any(|m| matches!(m, Message::NotesScanned(..))),
            "la preferenza superata non deve avviare una scansione"
        );
        assert_eq!(std::fs::read(spostata.join(file_nota)).unwrap(), originali);
        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.scan_generation, scope);
        assert_eq!(notes.scan_request, richiesta);
        assert_eq!(notes.directory.as_deref(), Some(a.as_path()));
        assert!(notes.store.is_some());
        assert!(!notes.create_after_load);
        assert!(notes.tabs.contains_key(&doc));
        assert_eq!(app.notes_directory, a);
        assert_eq!(id_dei_riepiloghi(&app), riepiloghi);
        assert_eq!(foto_tool(&mut app).summaries, immagine.summaries);
        assert_eq!(app.notice_seq, avviso);
        assert_eq!(app.persistence_error, persistence_error);
    }

    #[test]
    fn preferenza_iniziale_ok_superata_dal_reader_non_avvia_io() {
        preferenza_iniziale_superata_dal_reader(false);
    }

    #[cfg(unix)]
    #[test]
    fn preferenza_iniziale_err_superata_dal_reader_non_cambia_stato() {
        preferenza_iniziale_superata_dal_reader(true);
    }

    #[test]
    fn preferenza_corrente_avvia_scansione_e_crea_una_nota() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(cartella.path());
        let task = app.update(Message::NewNote);
        let preferenza = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::NotesPreferenceLoaded(..)))
            .unwrap();
        assert!(
            matches!(&preferenza, Message::NotesPreferenceLoaded(_, Ok(Some(p))) if p == cartella.path())
        );
        completa_nuovo_appunto(&mut app, preferenza);
        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.directory.as_deref(), Some(cartella.path()));
        assert!(!notes.create_after_load);
        assert_eq!(notes.tabs.len(), 1);
        assert_eq!(
            NoteStore::open(cartella.path())
                .unwrap()
                .scan()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn preferenza_di_istanza_disattivata_non_avvia_io_dopo_riattivazione() {
        let radice = tempfile::tempdir().unwrap();
        let vecchia = radice.path().join("assente");
        let nuova = tempfile::tempdir().unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(&vecchia);
        let task = app.update(Message::NewNote);
        let preferenza_vecchia = consuma_task_appunti(task).pop().unwrap();
        assert!(
            matches!(&preferenza_vecchia, Message::NotesPreferenceLoaded(_, Ok(Some(p))) if p == &vecchia)
        );
        cambia_cartella_e_riattiva(&mut app, nuova.path());

        let task = app.update(Message::NewNote);
        let preferenza_corrente = consuma_task_appunti(task).pop().unwrap();
        assert!(
            matches!(&preferenza_corrente, Message::NotesScanned(_, p, Ok(_)) if p == nuova.path())
        );
        let task = app.update(preferenza_vecchia);
        let _ = consuma_task_appunti(task);
        assert!(
            !vecchia.exists(),
            "la preferenza vecchia non deve creare la cartella"
        );
        assert_eq!(app.notes_directory, nuova.path());
        assert!(app.notes.as_ref().unwrap().create_after_load);
        assert!(app.notes.as_ref().unwrap().store.is_none());

        completa_nuovo_appunto(&mut app, preferenza_corrente);
        let store = NoteStore::open(nuova.path()).unwrap();
        assert_eq!(store.scan().unwrap().len(), 1);
        assert!(!vecchia.exists());
        assert!(!app.notes.as_ref().unwrap().create_after_load);
    }

    #[test]
    fn scansione_di_istanza_disattivata_non_crea_nella_cartella_vecchia() {
        let vecchia = tempfile::tempdir().unwrap();
        let nuova = tempfile::tempdir().unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(vecchia.path());
        let task = app.update(Message::NewNote);
        let preferenza = consuma_task_appunti(task).pop().unwrap();
        let task = app.update(preferenza);
        let scansione_vecchia = consuma_task_appunti(task).pop().unwrap();
        assert!(
            matches!(&scansione_vecchia, Message::NotesScanned(_, p, Ok(_)) if p == vecchia.path())
        );
        cambia_cartella_e_riattiva(&mut app, nuova.path());

        let task = app.update(Message::NewNote);
        let preferenza_corrente = consuma_task_appunti(task).pop().unwrap();
        assert!(
            matches!(&preferenza_corrente, Message::NotesScanned(_, p, Ok(_)) if p == nuova.path())
        );
        let task = app.update(scansione_vecchia);
        let _ = consuma_task_appunti(task);
        let store_vecchio = NoteStore::open(vecchia.path()).unwrap();
        assert_eq!(
            store_vecchio.scan().unwrap().len(),
            0,
            "la scansione vecchia non deve creare in A"
        );
        assert_eq!(app.notes_directory, nuova.path());
        assert!(app.notes.as_ref().unwrap().create_after_load);
        assert!(app.notes.as_ref().unwrap().store.is_none());
        assert!(app.notes.as_ref().unwrap().summaries.is_empty());

        completa_nuovo_appunto(&mut app, preferenza_corrente);
        let store_nuovo = NoteStore::open(nuova.path()).unwrap();
        assert_eq!(store_nuovo.scan().unwrap().len(), 1);
        assert_eq!(store_vecchio.scan().unwrap().len(), 0);
        assert_eq!(app.notes.as_ref().unwrap().tabs.len(), 1);
        assert!(!app.notes.as_ref().unwrap().create_after_load);
    }

    #[test]
    fn scansione_di_istanza_disattivata_non_sovrascrive_la_scheda_ripristinata() {
        let vecchia = tempfile::tempdir().unwrap();
        let nuova = tempfile::tempdir().unwrap();
        let store_nuovo = NoteStore::open(nuova.path()).unwrap();
        let nota = store_nuovo.create("da ripristinare").unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(vecchia.path());
        let task = app.update(Message::NewNote);
        let preferenza = consuma_task_appunti(task).pop().unwrap();
        let task = app.update(preferenza);
        let scansione_vecchia = consuma_task_appunti(task).pop().unwrap();
        cambia_cartella_e_riattiva(&mut app, nuova.path());

        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![nota.path().to_path_buf()];
        snapshot.active_tab = Some(nota.path().to_path_buf());
        let task = app.restore_snapshot_async(snapshot);
        let messaggio = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::RestoredTabReadFinished(..)))
            .expect("scheda ripristinata");
        let task = app.update(messaggio);
        let _ = consuma_task_appunti(task);
        let doc = app.workspace.get_by_path(nota.path()).unwrap();
        assert!(
            app.notes
                .as_ref()
                .unwrap()
                .open_saves
                .contains_key(&nota.note.header.id)
        );

        let task = app.update(scansione_vecchia);
        let _ = consuma_task_appunti(task);
        assert_eq!(app.notes_directory, nuova.path());
        let notes = app.notes.as_ref().unwrap();
        assert_eq!(notes.directory.as_deref(), Some(nuova.path()));
        assert_eq!(notes.store.as_ref().unwrap().scan().unwrap().len(), 1);
        assert_eq!(notes.tabs.get(&doc), Some(&nota.note.header.id));
        assert!(notes.open_saves.contains_key(&nota.note.header.id));
        assert!(notes.view_unloaded);
    }

    #[test]
    fn preferenza_obsoleta_non_cambia_cartella_ne_avvia_scansione() {
        let vecchia = tempfile::tempdir().unwrap();
        let nuova = tempfile::tempdir().unwrap();
        let assente = vecchia.path().join("non_creare");
        let (mut app, _, generazione_vecchia) = app_con_appunti(vecchia.path());
        let cambio = app.update(Message::NotesDirectoryPicked(Some(
            nuova.path().to_path_buf(),
        )));
        let corrente = consuma_task_appunti(cambio)
            .into_iter()
            .find(|m| matches!(m, Message::NotesScanned(..)))
            .unwrap();
        let generazione_corrente = app.notes_scan_generation();

        let task = app.update(Message::NotesPreferenceLoaded(
            generazione_vecchia,
            Ok(Some(assente.clone())),
        ));
        let _ = consuma_task_appunti(task);
        assert!(
            !assente.exists(),
            "la preferenza vecchia non deve creare la cartella"
        );
        assert_eq!(app.notes_directory, nuova.path());
        assert_eq!(app.notes_scan_generation(), generazione_corrente);

        let task = app.update(corrente);
        let _ = consuma_task_appunti(task);
        let task = app.update(Message::NotesPreferenceLoaded(
            generazione_vecchia,
            Ok(Some(assente.clone())),
        ));
        let _ = consuma_task_appunti(task);
        assert!(!assente.exists());
        assert_eq!(app.notes_directory, nuova.path());
        assert_eq!(
            app.notes.as_ref().unwrap().directory.as_deref(),
            Some(nuova.path())
        );
        assert_eq!(app.notes_scan_generation(), generazione_corrente);
        let task = app.update(Message::NotesPreferenceLoaded(
            generazione_vecchia,
            Err("errore obsoleto".into()),
        ));
        let _ = consuma_task_appunti(task);
        assert_ne!(app.persistence_error.as_deref(), Some("errore obsoleto"));
    }

    #[test]
    fn scansione_obsoleta_non_scarica_cache_di_altri_dati() {
        let cartella = tempfile::tempdir().unwrap();
        let estranea = tempfile::tempdir().unwrap();
        let (mut app, store, vecchia) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "da conservare");
        assert!(!app.registry.is_open(tools::notes::ID));
        let store_estraneo = NoteStore::open(estranea.path()).unwrap();
        let _ = store_estraneo.create("estranea").unwrap();
        let task = app.scan_notes_directory(vecchia, estranea.path().to_path_buf());
        let risposta = consuma_task_appunti(task).pop().unwrap();
        let corrente = app.next_notes_scan_generation().unwrap();
        let task = app.update(risposta);
        let _ = consuma_task_appunti(task);

        let notes = app.notes.as_ref().unwrap();
        assert!(
            !notes.view_unloaded,
            "la risposta vecchia non scarica la cache"
        );
        assert!(notes.loaded.contains_key(&nota.note.header.id));
        assert!(notes.summaries.iter().any(|s| s.id == nota.note.header.id));
        assert_eq!(app.notes_directory, cartella.path());
        assert_eq!(notes.directory.as_deref(), Some(cartella.path()));
        assert_eq!(notes.scan_generation, corrente);
        assert!(notes.tabs.contains_key(&doc));
        let riepiloghi_store = notes.store.as_ref().unwrap().scan().unwrap();
        assert_eq!(riepiloghi_store.len(), 1);
        assert_eq!(riepiloghi_store[0].id, nota.note.header.id);
        assert_eq!(store.scan().unwrap().len(), 1);
        assert_eq!(store_estraneo.scan().unwrap().len(), 1);
    }

    #[test]
    fn scansione_obsoleta_conserva_intento_e_cache_scarica_fino_alla_corrente() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, _db) = app_nuova_con_cartella_appunti_salvata(cartella.path());
        assert!(!app.registry.is_open(tools::notes::ID));

        let task = app.update(Message::NewNote);
        let preferenza_vecchia = consuma_task_appunti(task).pop().unwrap();
        let task = app.update(preferenza_vecchia);
        let scansione_vecchia = consuma_task_appunti(task).pop().unwrap();
        let vecchia = app.notes_scan_generation().unwrap();

        let task = app.update(Message::NewNote);
        let preferenza_corrente = consuma_task_appunti(task).pop().unwrap();
        let task = app.update(preferenza_corrente);
        let scansione_corrente = consuma_task_appunti(task).pop().unwrap();
        let corrente = app.notes_scan_generation().unwrap();
        assert_ne!(vecchia, corrente);
        app.unload_notes_view_cache();

        let task = app.update(scansione_vecchia);
        let _ = consuma_task_appunti(task);
        let notes = app.notes.as_ref().unwrap();
        assert!(
            notes.create_after_load,
            "la risposta vecchia non consuma l'intento"
        );
        assert!(notes.view_unloaded);
        assert!(notes.store.is_none());
        assert!(notes.summaries.is_empty());
        assert!(notes.loaded.is_empty());
        let store = NoteStore::open(cartella.path()).unwrap();
        assert_eq!(store.scan().unwrap().len(), 0);

        let task = app.update(scansione_corrente);
        let aperta = consuma_task_appunti(task)
            .into_iter()
            .find(|m| matches!(m, Message::NotesOpened(..)))
            .expect("la risposta corrente deve creare la nota");
        let task = app.update(aperta);
        let _ = consuma_task_appunti(task);
        let notes = app.notes.as_ref().unwrap();
        assert!(!notes.create_after_load);
        assert!(notes.view_unloaded);
        assert!(notes.store.is_some());
        assert!(notes.summaries.is_empty());
        assert!(notes.loaded.is_empty());
        assert_eq!(notes.tabs.len(), 1);
        assert_eq!(notes.open_saves.len(), 1);
        let note = store.scan().unwrap();
        assert_eq!(note.len(), 1);
        // Lo store conserva la radice canonica: il confronto usa la stessa
        // forma, non il percorso della TMPDIR eventualmente dietro un link.
        let cartella_canonica = lume_core::canonical_path(cartella.path()).unwrap();
        assert!(note[0].path.starts_with(&cartella_canonica));

        let task = app.update(Message::NotesScanned(
            vecchia,
            cartella.path().to_path_buf(),
            Ok((store.clone(), store.scan().unwrap())),
        ));
        let _ = consuma_task_appunti(task);
        let task = app.update(Message::NotesScanned(
            vecchia,
            PathBuf::new(),
            Err("errore scansione obsoleto".into()),
        ));
        let _ = consuma_task_appunti(task);
        assert_ne!(
            app.persistence_error.as_deref(),
            Some("errore scansione obsoleto")
        );
        assert_eq!(store.scan().unwrap().len(), 1);
        assert_eq!(app.notes.as_ref().unwrap().tabs.len(), 1);
        assert!(app.notes.as_ref().unwrap().view_unloaded);
    }

    fn id_dei_riepiloghi(app: &Lume) -> Vec<String> {
        let mut id: Vec<String> = app
            .notes
            .as_ref()
            .unwrap()
            .summaries
            .iter()
            .map(|s| s.id.as_str().to_owned())
            .collect();
        id.sort();
        id
    }

    /// Riapertura con la riscansione in volo e una creazione nel frattempo
    /// (il «+» o Ctrl+Alt+N): la scansione non deve andare persa, in qualunque
    /// ordine arrivino i due risultati.
    fn riapri_con_creazione_in_volo(scansione_prima: bool) {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let altra = store.create("altra").unwrap();
        app.unload_notes_view_cache();
        riapri_pannello(&mut app);
        let scan_generation = app.notes.as_ref().unwrap().scan_request;
        // la scansione e' partita prima della creazione: non vede la nota nuova
        let scansione = store.scan().unwrap();

        let _ = app.create_note(); // fa avanzare la generazione di apertura
        let generation = app.notes_generation().unwrap();
        assert_ne!(generation, app.notes_scan_generation().unwrap());
        let nuova = store.create("nuova").unwrap();

        let scansionata = Message::NotesScanned(
            scan_generation,
            cartella.path().to_path_buf(),
            Ok((store.clone(), scansione)),
        );
        let aperta = Message::NotesOpened(generation, Ok((nuova.clone(), None)));
        if scansione_prima {
            let _ = app.update(scansionata);
            let _ = app.update(aperta);
        } else {
            let _ = app.update(aperta);
            let _ = app.update(scansionata);
        }

        let mut attesi = vec![
            altra.note.header.id.as_str().to_owned(),
            nuova.note.header.id.as_str().to_owned(),
        ];
        attesi.sort();
        assert_eq!(
            id_dei_riepiloghi(&app),
            attesi,
            "la riscansione non e' persa: ci sono l'altra nota e la nuova"
        );
        assert_eq!(app.notes.as_ref().unwrap().tabs.len(), 1);
        let foto = foto_tool(&mut app);
        assert_eq!(foto.summaries, 2, "il pannello mostra tutto");
        assert!(!foto.loading);
    }

    #[test]
    fn reopen_note_la_riscansione_sopravvive_a_una_creazione_arrivata_prima() {
        riapri_con_creazione_in_volo(false);
    }

    #[test]
    fn reopen_note_la_riscansione_sopravvive_a_una_creazione_arrivata_dopo() {
        riapri_con_creazione_in_volo(true);
    }

    #[test]
    fn reopen_note_il_cambio_cartella_scarta_ancora_la_scansione_della_vecchia() {
        let vecchia = tempfile::tempdir().unwrap();
        let nuova = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(vecchia.path());
        let _ = store.create("della vecchia").unwrap();
        let scansione_vecchia = store.scan().unwrap();
        let scan_generation = app.notes_scan_generation().unwrap();

        let cambio = app.switch_notes_directory(nuova.path().to_path_buf());
        let _ = consuma_task_appunti(cambio);
        let task = app.update(Message::NotesScanned(
            scan_generation,
            vecchia.path().to_path_buf(),
            Ok((store.clone(), scansione_vecchia)),
        ));
        let _ = consuma_task_appunti(task);

        let notes = app.notes.as_ref().unwrap();
        assert!(notes.summaries.is_empty(), "scansione obsoleta scartata");
        assert!(
            notes.store.is_none(),
            "lo store vecchio non viene pubblicato"
        );
        assert_ne!(notes.scan_generation, scan_generation);
        assert_eq!(app.notes_directory, nuova.path());
    }

    #[test]
    fn reopen_note_il_pannello_riaperto_e_in_caricamento_fino_alla_scansione() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let nota = store.create("esistente").unwrap();
        app.unload_notes_view_cache();
        riapri_pannello(&mut app);

        let foto = foto_tool(&mut app);
        assert!(
            foto.loading,
            "niente «Nessun appunto» prima della scansione"
        );
        assert_eq!(foto.summaries, 0);
        assert!(foto.directory.is_some());

        let scan_generation = app.notes.as_ref().unwrap().scan_request;
        let _ = app.update(Message::NotesScanned(
            scan_generation,
            cartella.path().to_path_buf(),
            Ok((store.clone(), store.scan().unwrap())),
        ));
        let foto = foto_tool(&mut app);
        assert!(!foto.loading);
        assert_eq!(foto.summaries, 1);
        assert_eq!(
            id_dei_riepiloghi(&app),
            vec![nota.note.header.id.as_str().to_owned()]
        );
    }

    #[test]
    fn scarico_cache_il_clear_arriva_al_tool_sul_percorso_nuovo_appunto() {
        let cartella = tempfile::tempdir().unwrap();
        let store = NoteStore::open(cartella.path()).unwrap();
        let _ = store.create("gia' su disco").unwrap();
        let (mut app, _) = Lume::new_without_store();
        assert!(!app.registry.is_open(tools::notes::ID));

        // «Nuovo appunto» con il pannello mai aperto: la scansione copia i
        // riepiloghi nel tool, poi lo scarico li deve togliere anche da li'
        let _ = app.update(Message::NewNote);
        let scan_generation = app.notes_scan_generation().unwrap();
        let _ = app.update(Message::NotesScanned(
            scan_generation,
            cartella.path().to_path_buf(),
            Ok((store.clone(), store.scan().unwrap())),
        ));

        let foto = foto_tool(&mut app);
        assert_eq!(foto.summaries, 0, "il tool non tiene la lista copiata");
        assert!(app.notes.as_ref().unwrap().summaries.is_empty());
    }

    #[test]
    fn scarico_cache_la_guardia_della_scansione_regge_a_registro_aperto() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let _ = store.create("presente").unwrap();
        // clic di apertura, ma la richiesta NotesLoad non e' ancora drenata
        let _richieste_non_drenate = app.registry.click(tools::notes::ID);
        assert!(app.registry.is_open(tools::notes::ID));
        app.unload_notes_view_cache();
        let scan_generation = app.notes_scan_generation().unwrap();

        // una scansione partita prima arriva: il gestore non scarica (il
        // registro e' aperto), quindi e' la guardia a impedire il ripopolamento
        let _ = app.update(Message::NotesScanned(
            scan_generation,
            cartella.path().to_path_buf(),
            Ok((store.clone(), store.scan().unwrap())),
        ));

        assert!(app.notes.as_ref().unwrap().summaries.is_empty());
        assert_eq!(foto_tool(&mut app).summaries, 0);
    }

    #[test]
    fn reopen_note_la_riscansione_parte_con_la_generazione_delle_scansioni() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, _store, _) = app_con_appunti(cartella.path());
        app.unload_notes_view_cache();
        // due creazioni in volo fanno avanzare solo la generazione di apertura
        let _ = app.create_note();
        let _ = app.create_note();

        let (generation, directory) = app.take_notes_rescan().expect("riscansione dovuta");

        assert_eq!(generation, app.notes.as_ref().unwrap().scan_request);
        assert_ne!(Some(generation), app.notes_scan_generation());
        assert_ne!(app.notes_scan_generation(), app.notes_generation());
        assert_eq!(directory, cartella.path());
        assert!(
            app.take_notes_rescan().is_none(),
            "la riscansione si chiede una volta sola"
        );
    }

    #[test]
    fn opening_settings_scans_external_icon_themes_once() {
        use crate::appearance::fixtures::root_with_packs;

        let config = root_with_packs(&["alpha", "night"]);
        let (mut app, _) = Lume::new_without_store();
        app.icon_themes = IconThemeState::from_config_dir(Some(config.clone()));
        assert_eq!(
            app.icon_themes.scan_runs(),
            0,
            "nessuna scansione prima di Aspetto"
        );

        app.apply_activity_intent(ActivityBarIntent::ToggleSettings);
        assert!(app.settings_panel.is_open());
        assert_eq!(app.icon_themes.scan_runs(), 1);
        let ids = app
            .icon_themes
            .rows()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        assert!(ids.contains(&"external:alpha".to_owned()));
        assert!(ids.contains(&"external:night".to_owned()));

        app.apply_activity_intent(ActivityBarIntent::ToggleSettings);
        app.apply_activity_intent(ActivityBarIntent::ToggleSettings);
        assert_eq!(
            app.icon_themes.scan_runs(),
            1,
            "le aperture successive non riscansionano"
        );
        std::fs::remove_dir_all(config).unwrap();
    }

    #[test]
    fn restoring_a_saved_external_icon_theme_activates_it_without_a_full_scan() {
        use crate::appearance::fixtures::root_with_packs;
        use lume_store::SessionSnapshot;

        let config = root_with_packs(&["alpha", "night"]);
        let (mut app, _) = Lume::new_without_store();
        app.icon_themes = IconThemeState::from_config_dir(Some(config.clone()));
        let mut snapshot = SessionSnapshot::empty();
        snapshot.appearance.icon_theme_id = "external:night".into();

        app.restore_snapshot(snapshot);

        assert_eq!(app.icon_themes.active_id(), "external:night");
        assert_eq!(app.icon_themes.scan_runs(), 0);
        std::fs::remove_dir_all(config).unwrap();
    }

    /// Primo caricamento fallito (per esempio cartella su un'unita' scollegata),
    /// poi pannello chiuso e riaperto: lo stato di partenza dei test sotto.
    fn app_con_primo_caricamento_fallito() -> Lume {
        let (mut app, _) = Lume::new_without_store();
        riapri_pannello(&mut app);
        let generation = app.notes_scan_generation().unwrap();
        let _ = app.update(Message::NotesScanned(
            generation,
            PathBuf::new(),
            Err("unita' scollegata".into()),
        ));
        let foto = foto_tool(&mut app);
        assert!(!foto.loading, "l'errore e' in vista, non c'e' attesa");
        assert!(app.notes.as_ref().unwrap().store.is_none());

        let chiusura = app.registry.click(tools::notes::ID);
        let _ = app.apply_tool_requests(chiusura);
        assert!(foto_tool(&mut app).loading, "la chiusura scarica la vista");
        app
    }

    #[test]
    fn reopen_note_dopo_un_primo_caricamento_fallito_la_riapertura_ritenta() {
        let cartella = tempfile::tempdir().unwrap();
        let store = NoteStore::open(cartella.path()).unwrap();
        let nota = store.create("esiste").unwrap();
        let mut app = app_con_primo_caricamento_fallito();
        let prima = app.notes_scan_generation().unwrap();

        riapri_pannello(&mut app);

        // senza un caricamento in volo il pannello resterebbe «Caricamento…»
        // per sempre: la riapertura deve far ripartire il caricamento
        let ora = app.notes_scan_generation().unwrap();
        assert_ne!(ora, prima, "e' partito un nuovo caricamento");
        let _ = app.update(Message::NotesScanned(
            ora,
            cartella.path().to_path_buf(),
            Ok((store.clone(), store.scan().unwrap())),
        ));
        let foto = foto_tool(&mut app);
        assert!(!foto.loading);
        assert_eq!(foto.summaries, 1);
        assert_eq!(
            id_dei_riepiloghi(&app),
            vec![nota.note.header.id.as_str().to_owned()]
        );
    }

    #[test]
    fn reopen_note_un_secondo_fallimento_mostra_ancora_l_errore_e_non_l_attesa() {
        let mut app = app_con_primo_caricamento_fallito();
        riapri_pannello(&mut app);
        let ora = app.notes_scan_generation().unwrap();

        let _ = app.update(Message::NotesScanned(
            ora,
            PathBuf::new(),
            Err("ancora scollegata".into()),
        ));

        assert!(!foto_tool(&mut app).loading);
        // e si puo' riprovare ancora: chiudi e riapri ritenta di nuovo
        let chiusura = app.registry.click(tools::notes::ID);
        let _ = app.apply_tool_requests(chiusura);
        riapri_pannello(&mut app);
        assert_ne!(app.notes_scan_generation().unwrap(), ora);
    }

    #[test]
    fn reopen_note_il_risultato_del_caricamento_scartato_non_tocca_il_nuovo() {
        let cartella = tempfile::tempdir().unwrap();
        let store = NoteStore::open(cartella.path()).unwrap();
        let (mut app, _) = Lume::new_without_store();
        riapri_pannello(&mut app);
        let vecchia = app.notes_scan_generation().unwrap();
        // chiudi e riapri mentre il primo caricamento e' ancora in volo
        let chiusura = app.registry.click(tools::notes::ID);
        let _ = app.apply_tool_requests(chiusura);
        riapri_pannello(&mut app);
        assert_ne!(app.notes_scan_generation().unwrap(), vecchia);

        let _ = app.update(Message::NotesScanned(
            vecchia,
            cartella.path().to_path_buf(),
            Ok((store.clone(), store.scan().unwrap())),
        ));

        assert!(
            app.notes.as_ref().unwrap().store.is_none(),
            "il risultato del caricamento superato e' scartato"
        );
    }

    // --- Rilettura dal disco di una scheda Appunti (difetto 15/10) ---------
    // Il watcher consegna il file serializzato: nell'editor deve entrare solo
    // il corpo, l'identita' della scheda resta e l'eco del nostro salvataggio
    // non crea revisioni undo finte.

    /// Esegue il Task del watcher fino al primo `DocRicaricato` consegnato.
    fn watcher_consegna_ricaricamento(task: iced::Task<Message>) -> Message {
        use futures::StreamExt;
        fn runtime() -> &'static tokio::runtime::Runtime {
            static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> =
                std::sync::OnceLock::new();
            RUNTIME.get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .unwrap()
            })
        }
        runtime().block_on(async {
            let mut stream = iced_runtime::task::into_stream(task).expect("flusso del Task");
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(message) = action
                    && matches!(message, Message::DocRicaricato(..))
                {
                    return message;
                }
            }
            panic!("il watcher non ha consegnato alcuna rilettura");
        })
    }

    /// Un evento del watcher, completato fino alla consegna all'app.
    fn rileggi_dal_watcher(app: &mut Lume, paths: &[&Path]) {
        let percorso: Vec<PathBuf> = paths.iter().map(|p| p.to_path_buf()).collect();
        let task = app.update(Message::FsChanged(percorso));
        let messaggio = watcher_consegna_ricaricamento(task);
        let _ = app.update(messaggio);
    }

    /// Un altro programma riscrive il corpo della nota, stesso id.
    fn riscrivi_corpo_esterno(store: &NoteStore, nota: &LoadedNote, corpo: &str) {
        let mut esterna = store.load(nota.path()).unwrap();
        esterna.note.markdown = corpo.to_owned();
        store.save(&esterna).unwrap();
    }

    /// Sostituisce l'intero contenuto della scheda, come un incolla totale.
    fn riscrivi_documento(app: &mut Lume, doc: DocId, testo: &str) {
        let lunghezza = app.workspace.get(doc).unwrap().rope().len_chars();
        let cambi =
            lume_core::ChangeSet::replace_many(&[(0, lunghezza, testo.to_owned())], lunghezza)
                .unwrap();
        app.workspace
            .get_mut(doc)
            .unwrap()
            .apply(lume_core::Transaction {
                changes: cambi,
                selection_after: None,
            });
    }

    #[test]
    fn rilettura_dal_watcher_solo_corpo_nel_buffer_della_nota() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");

        riscrivi_corpo_esterno(&store, &nota, "corpo nuovo\n");
        rileggi_dal_watcher(&mut app, &[nota.path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "corpo nuovo\n",
            "nell'editor entra solo il Markdown, mai il file serializzato"
        );
        assert!(!app.workspace.get(doc).unwrap().is_dirty());
        assert_eq!(corpo_su_disco(&store, &nota), "corpo nuovo\n");
    }

    #[test]
    fn dopo_la_rilettura_esterna_il_salvataggio_segue_il_disco() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");

        riscrivi_corpo_esterno(&store, &nota, "esterno");
        rileggi_dal_watcher(&mut app, &[nota.path()]);
        scrivi(&mut app, doc, " e mio");
        app.write_doc(doc)
            .expect("la guardia segue l'identita' appena riletta");

        assert_eq!(corpo_su_disco(&store, &nota), "esterno e mio");
    }

    #[test]
    fn eco_del_salvataggio_della_nota_senza_revisioni_ne_frontmatter_nel_buffer() {
        use sha2::{Digest, Sha256};
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");
        scrivi(&mut app, doc, "corpo");
        app.write_doc(doc).unwrap();

        let atteso: [u8; 32] = Sha256::digest(std::fs::read(nota.path()).unwrap()).into();
        assert_eq!(
            app.disk_fingerprints.get(&doc),
            Some(&atteso),
            "la guardia registra i byte esatti del file serializzato"
        );

        let versione = app.workspace.get(doc).unwrap().version();
        rileggi_dal_watcher(&mut app, &[nota.path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().version(),
            versione,
            "l'eco del watcher non crea una revisione undo finta"
        );
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "corpo",
            "il file serializzato non entra nel buffer"
        );
    }

    #[test]
    fn eco_piu_autosave_non_duplica_l_intestazione() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");
        scrivi(&mut app, doc, "# titolo\ncorpo");
        app.write_doc(doc).unwrap();

        // il watcher consegna l'eco del nostro stesso salvataggio
        rileggi_dal_watcher(&mut app, &[nota.path()]);

        // l'utente continua a scrivere e il salvataggio riparte
        scrivi(&mut app, doc, " in coda");
        app.write_doc(doc).unwrap();

        let riletta = store.load(nota.path()).unwrap();
        assert_eq!(
            riletta.note.markdown, "# titolo\ncorpo in coda",
            "il corpo non assorbe mai l'intestazione"
        );
        let grezzo = std::fs::read_to_string(nota.path()).unwrap();
        assert_eq!(
            grezzo.matches("+++").count(),
            2,
            "una sola coppia di delimitatori: {grezzo:?}"
        );
    }

    #[test]
    fn eco_della_rinomina_da_titolo_senza_frontmatter_nel_buffer() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");
        let vecchio = nota.path().to_path_buf();
        scrivi(&mut app, doc, "# titolo\ncorpo");
        app.write_doc(doc).unwrap();

        // il titolo nel corpo cambia: il salvataggio rinomina il file
        riscrivi_documento(&mut app, doc, "# nuovo titolo\ncorpo");
        app.write_doc(doc).unwrap();
        let nuovo = app
            .workspace
            .get(doc)
            .unwrap()
            .path()
            .unwrap()
            .to_path_buf();
        assert_ne!(nuovo, vecchio, "il file e' stato rinominato");
        assert!(nuovo.is_file());

        let versione = app.workspace.get(doc).unwrap().version();
        rileggi_dal_watcher(&mut app, &[vecchio.as_path(), nuovo.as_path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().version(),
            versione,
            "l'eco della rinomina non crea una revisione undo finta"
        );
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "# nuovo titolo\ncorpo"
        );
        assert_eq!(
            store.load(&nuovo).unwrap().note.markdown,
            "# nuovo titolo\ncorpo"
        );
    }

    #[test]
    fn nota_sporca_piu_modifica_esterna_blocca_e_conserva() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");
        scrivi(&mut app, doc, "mia bozza");
        riscrivi_corpo_esterno(&store, &nota, "esterno\n");

        rileggi_dal_watcher(&mut app, &[nota.path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "mia bozza",
            "il testo locale non si perde"
        );
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert!(
            app.external_conflicts.contains(&doc),
            "conflitto registrato"
        );
        assert!(app.write_doc(doc).is_err(), "il salvataggio resta bloccato");
        assert_eq!(
            corpo_su_disco(&store, &nota),
            "esterno\n",
            "il file dell'altro programma non e' schiacciato"
        );
    }

    #[test]
    fn nota_con_identita_diversa_non_viene_adottata() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");

        let estranea_cartella = tempfile::tempdir().unwrap();
        let estranea = NoteStore::open(estranea_cartella.path()).unwrap();
        let salvata = estranea.create("estranea").unwrap();
        assert_ne!(salvata.note.header.id, nota.note.header.id);
        std::fs::write(nota.path(), std::fs::read(salvata.path()).unwrap()).unwrap();
        rileggi_dal_watcher(&mut app, &[nota.path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "",
            "il buffer resta il corpo della scheda, non il file estraneo"
        );
        let avviso = app
            .notice
            .as_ref()
            .expect("un avviso in status bar")
            .1
            .text
            .clone();
        assert!(avviso.contains("identità diversa"), "{avviso}");
    }

    #[test]
    fn nota_senza_intestazione_esterna_viene_adottata_con_l_identita_della_scheda() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");

        std::fs::write(nota.path(), "# esterna\nsenza intestazione\n").unwrap();
        rileggi_dal_watcher(&mut app, &[nota.path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "# esterna\nsenza intestazione\n",
            "un Markdown senza intestazione resta il corpo della scheda"
        );
        assert_eq!(
            app.notes
                .as_ref()
                .unwrap()
                .titles
                .get(&nota.note.header.id)
                .map(String::as_str),
            Some("esterna"),
            "il titolo del pannello segue il corpo appena letto"
        );

        scrivi(&mut app, doc, " in coda");
        app.write_doc(doc)
            .expect("il salvataggio riparte con l'identita' della scheda");
        let riletta = store.load(nota.path()).unwrap();
        assert_eq!(
            riletta.note.header.id, nota.note.header.id,
            "la scheda conserva la propria identita'"
        );
        assert_eq!(
            riletta.note.markdown,
            "# esterna\nsenza intestazione\n in coda"
        );
        let grezzo = std::fs::read_to_string(nota.path()).unwrap();
        assert_eq!(
            grezzo.matches("+++").count(),
            2,
            "il salvataggio riacquista l'intestazione: {grezzo:?}"
        );
    }

    #[test]
    fn nota_illeggibile_non_viene_adottata() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");

        let rotta = b"+++\nid = [\n+++\ncorpo";
        std::fs::write(nota.path(), rotta).unwrap();
        rileggi_dal_watcher(&mut app, &[nota.path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "",
            "il file illeggibile non entra nel buffer"
        );
        let avviso = app
            .notice
            .as_ref()
            .expect("un avviso in status bar")
            .1
            .text
            .clone();
        assert!(avviso.contains("non è leggibile"), "{avviso}");
        assert_eq!(std::fs::read(nota.path()).unwrap(), rotta);
    }

    #[test]
    fn senza_libreria_caricata_la_nota_non_si_rilegge() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");
        riscrivi_corpo_esterno(&store, &nota, "esterno");
        app.notes.as_mut().unwrap().store = None;

        rileggi_dal_watcher(&mut app, &[nota.path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "",
            "senza NoteStore la scheda resta com'e'"
        );
        let avviso = app
            .notice
            .as_ref()
            .expect("un avviso in status bar")
            .1
            .text
            .clone();
        assert!(avviso.contains("libreria Appunti"), "{avviso}");
    }

    #[test]
    fn dopo_lo_scarico_della_vista_la_rilettura_esterna_aggiorna_la_guardia() {
        let cartella = tempfile::tempdir().unwrap();
        let (mut app, store, _) = app_con_appunti(cartella.path());
        let (doc, nota) = apri_nota(&mut app, &store, "titolo");
        app.unload_notes_view_cache();
        riscrivi_corpo_esterno(&store, &nota, "esterno dopo lo scarico\n");

        rileggi_dal_watcher(&mut app, &[nota.path()]);

        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "esterno dopo lo scarico\n"
        );
        let notes = app.notes.as_ref().unwrap();
        assert!(notes.loaded.is_empty(), "la cache vista resta scarica");
        assert!(
            notes.open_saves.contains_key(&nota.note.header.id),
            "la guardia minima e' registrata per il prossimo salvataggio"
        );

        scrivi(&mut app, doc, " coda");
        app.write_doc(doc)
            .expect("il salvataggio usa la guardia anche a vista scarica");
        assert_eq!(
            corpo_su_disco(&store, &nota),
            "esterno dopo lo scarico\n coda"
        );
    }

    // --- Riapertura di sessione di una scheda Appunti (06-A/B) -------------

    fn esegui_task_fino_a(
        task: iced::Task<Message>,
        predicato: impl Fn(&Message) -> bool,
    ) -> Message {
        use futures::StreamExt;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut stream = iced_runtime::task::into_stream(task).expect("flusso del Task");
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(message) = action
                    && predicato(&message)
                {
                    return message;
                }
            }
            panic!("il Task non ha prodotto il messaggio atteso");
        })
    }

    fn app_nuova_con_cartella_appunti_salvata(
        cartella_appunti: &Path,
    ) -> (Lume, tempfile::TempDir) {
        let dir_db = tempfile::tempdir().unwrap();
        let db_path = dir_db.path().join("session.sqlite3");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let db = rt.block_on(async {
            let handle = lume_store::StoreHandle::open_at(db_path).await.unwrap();
            handle
                .save_notes_directory(cartella_appunti.to_path_buf())
                .await
                .unwrap();
            handle
        });
        let (mut app, _) = Lume::new_without_store();
        app.store = Some(db);
        (app, dir_db)
    }

    #[test]
    fn ripristino_sessione_asincrono_apre_la_nota_senza_frontmatter_e_salva() {
        let cartella = tempfile::tempdir().unwrap();
        let store = NoteStore::open(cartella.path()).unwrap();
        let mut nota = store.create("Titolo sessione").unwrap();
        nota.note.markdown = "# Titolo sessione\ncorpo della nota\n".to_owned();
        let nota = store.save(&nota).unwrap();
        let grezzo_iniziale = std::fs::read_to_string(nota.path()).unwrap();
        assert!(
            grezzo_iniziale.starts_with("+++\n"),
            "il file su disco ha il frontmatter TOML: {grezzo_iniziale:?}"
        );

        let (mut app, _dir_db) = app_nuova_con_cartella_appunti_salvata(cartella.path());
        assert!(app.notes.is_none());
        assert!(!app.registry.is_open(tools::notes::ID));

        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![nota.path().to_path_buf()];
        snapshot.active_tab = Some(nota.path().to_path_buf());

        let task = app.restore_snapshot_async(snapshot);
        let msg = esegui_task_fino_a(task, |m| matches!(m, Message::RestoredTabReadFinished(..)));
        let _ = app.update(msg);

        let doc = app.workspace.get_by_path(nota.path()).unwrap();
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "# Titolo sessione\ncorpo della nota\n",
            "nell'editor entra solo il corpo Markdown, mai il frontmatter +++"
        );
        assert!(!app.workspace.get(doc).unwrap().is_dirty());
        assert_eq!(
            app.editor_kind(doc),
            crate::editor_groups::EditorKind::Note(nota.note.header.id.clone())
        );
        assert_eq!(app.editor_tab_name(doc).as_deref(), Some("Titolo sessione"));
        let notes = app.notes.as_ref().expect("stato Appunti registrato");
        assert!(
            notes.loaded.is_empty(),
            "a pannello chiuso la cache della vista resta scarica"
        );
        assert!(notes.open_saves.contains_key(&nota.note.header.id));

        scrivi(&mut app, doc, "seconda riga\n");
        app.write_doc(doc)
            .expect("il salvataggio dopo il ripristino usa NoteStore");

        let riletta = store.load(nota.path()).unwrap();
        assert_eq!(riletta.note.header.id, nota.note.header.id);
        assert_eq!(
            riletta.note.markdown,
            "# Titolo sessione\ncorpo della nota\nseconda riga\n"
        );
        let grezzo_finale = std::fs::read_to_string(nota.path()).unwrap();
        assert_eq!(
            grezzo_finale.matches("+++").count(),
            2,
            "una sola intestazione sul disco dopo il salvataggio: {grezzo_finale:?}"
        );
    }

    #[test]
    fn ripristino_sessione_sincrono_e_watcher_conservano_corpo_e_identita() {
        let cartella = tempfile::tempdir().unwrap();
        let store = NoteStore::open(cartella.path()).unwrap();
        let mut nota = store.create("Titolo").unwrap();
        nota.note.markdown = "# Titolo\ncorpo iniziale\n".to_owned();
        let nota = store.save(&nota).unwrap();

        let (mut app, _dir_db) = app_nuova_con_cartella_appunti_salvata(cartella.path());
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![nota.path().to_path_buf()];
        snapshot.active_tab = Some(nota.path().to_path_buf());

        app.restore_snapshot(snapshot);

        let doc = app.workspace.get_by_path(nota.path()).unwrap();
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "# Titolo\ncorpo iniziale\n"
        );
        assert_eq!(
            app.editor_kind(doc),
            crate::editor_groups::EditorKind::Note(nota.note.header.id.clone())
        );

        riscrivi_corpo_esterno(&store, &nota, "# Titolo\nda fuori\n");
        rileggi_dal_watcher(&mut app, &[nota.path()]);
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "# Titolo\nda fuori\n",
            "il watcher continua a rileggere solo il corpo dopo il ripristino"
        );

        scrivi(&mut app, doc, "e locale\n");
        app.write_doc(doc).unwrap();
        assert_eq!(
            corpo_su_disco(&store, &nota),
            "# Titolo\nda fuori\ne locale\n"
        );
    }

    #[test]
    fn aprire_nota_dal_pannello_su_scheda_ripristinata_in_attesa_adotta_il_corpo_senza_duplicare() {
        let cartella = tempfile::tempdir().unwrap();
        let radice = tempfile::tempdir().unwrap();
        let codice = radice.path().join("attivo.rs");
        std::fs::write(&codice, "fn main() {}\n").unwrap();

        let store = NoteStore::open(cartella.path()).unwrap();
        let mut nota = store.create("Pigra").unwrap();
        nota.note.markdown = "# Pigra\ncorpo pigro\n".to_owned();
        let nota = store.save(&nota).unwrap();

        let (mut app, _dir_db) = app_nuova_con_cartella_appunti_salvata(cartella.path());
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.workspace_root = Some(radice.path().to_path_buf());
        snapshot.tabs = vec![codice.clone(), nota.path().to_path_buf()];
        snapshot.active_tab = Some(codice);

        app.restore_snapshot(snapshot);
        let doc_nota = app.workspace.get_by_path(nota.path()).unwrap();
        assert!(
            app.pending_restored_tabs.contains(doc_nota),
            "la scheda non attiva resta in attesa di lettura"
        );

        riapri_pannello(&mut app);
        let scan_gen = app.notes_scan_generation().unwrap();
        let _ = app.update(Message::NotesScanned(
            scan_gen,
            cartella.path().to_path_buf(),
            Ok((store.clone(), store.scan().unwrap())),
        ));

        let task = app.open_note(nota.note.header.id.clone());
        let msg = esegui_task_fino_a(task, |m| matches!(m, Message::NotesReadFinished(..)));
        let _ = app.update(msg);

        assert_eq!(
            app.groups.all().filter(|&d| d == doc_nota).count(),
            1,
            "nessuna scheda duplicata"
        );
        assert!(!app.pending_restored_tabs.contains(doc_nota));
        assert_eq!(
            app.workspace.get(doc_nota).unwrap().rope().to_string(),
            "# Pigra\ncorpo pigro\n",
            "il placeholder viene popolato con il solo corpo della nota"
        );
        assert!(!app.workspace.get(doc_nota).unwrap().is_dirty());
        assert_eq!(
            app.editor_kind(doc_nota),
            crate::editor_groups::EditorKind::Note(nota.note.header.id.clone())
        );

        scrivi(&mut app, doc_nota, "aggiunta\n");
        app.write_doc(doc_nota).unwrap();
        assert_eq!(
            corpo_su_disco(&store, &nota),
            "# Pigra\ncorpo pigro\naggiunta\n"
        );
    }

    #[test]
    fn file_markdown_fuori_dagli_appunti_resta_documento_codice_al_ripristino() {
        let cartella = tempfile::tempdir().unwrap();
        let radice = tempfile::tempdir().unwrap();
        let md_progetto = radice.path().join("README.md");
        let grezzo = "+++\ntitle = \"Sito statico\"\n+++\n# Pagina\n";
        std::fs::write(&md_progetto, grezzo).unwrap();

        let (mut app, _dir_db) = app_nuova_con_cartella_appunti_salvata(cartella.path());
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.workspace_root = Some(radice.path().to_path_buf());
        snapshot.tabs = vec![md_progetto.clone()];
        snapshot.active_tab = Some(md_progetto.clone());

        app.restore_snapshot(snapshot);

        let doc = app.workspace.get_by_path(&md_progetto).unwrap();
        assert_eq!(app.editor_kind(doc), crate::editor_groups::EditorKind::Code);
        assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), grezzo);
    }

    #[test]
    fn ripristino_sessione_poi_apertura_pannello_appunti_scansiona_la_libreria_e_non_duplica_la_scheda()
     {
        let cartella = tempfile::tempdir().unwrap();
        let store = NoteStore::open(cartella.path()).unwrap();
        let mut aperta = store.create("Aperta").unwrap();
        aperta.note.markdown = "# Aperta\ncorpo aperto\n".to_owned();
        let aperta = store.save(&aperta).unwrap();
        let altra = store.create("Altra").unwrap();

        let (mut app, _dir_db) = app_nuova_con_cartella_appunti_salvata(cartella.path());
        let mut snapshot = lume_store::SessionSnapshot::empty();
        snapshot.tabs = vec![aperta.path().to_path_buf()];
        snapshot.active_tab = Some(aperta.path().to_path_buf());

        app.restore_snapshot(snapshot);
        let doc = app.workspace.get_by_path(aperta.path()).unwrap();
        assert_eq!(
            app.workspace.get(doc).unwrap().rope().to_string(),
            "# Aperta\ncorpo aperto\n"
        );

        riapri_pannello(&mut app);
        let scan_gen = app.notes.as_ref().unwrap().scan_request;
        let _ = app.update(Message::NotesScanned(
            scan_gen,
            cartella.path().to_path_buf(),
            Ok((store.clone(), store.scan().unwrap())),
        ));

        let foto = foto_tool(&mut app);
        assert!(!foto.loading);
        assert_eq!(
            foto.summaries, 2,
            "la libreria mostra tutte le note della cartella"
        );
        let mut attesi = vec![
            aperta.note.header.id.as_str().to_owned(),
            altra.note.header.id.as_str().to_owned(),
        ];
        attesi.sort();
        assert_eq!(id_dei_riepiloghi(&app), attesi);

        let _ = app.open_note(aperta.note.header.id.clone());
        assert_eq!(app.notes.as_ref().unwrap().tabs.len(), 1);
        assert_eq!(app.groups.all().count(), 1);
    }

    #[cfg(feature = "mcp")]
    mod mcp {
        use super::*;
        use crate::consenso_mcp::ConsensoMessage;
        use crate::features::{Feature, FeatureCommand};
        use crate::mcp_card::CardMessage;
        use crate::overlay_manager::OverlayId;
        use lume_mcp::McpEvent;
        use lume_mcp::{AppAnswer, AppQuery, Availability, McpCommand};

        fn app() -> Lume {
            Lume::new_without_store().0
        }

        fn app_con_cartella(nome: &str) -> (Lume, std::path::PathBuf) {
            let root = tempdir(nome);
            let mut app = app();
            app.set_project_root(root.clone());
            let _ = app.registry.set_feature_enabled("mcp", true);
            (app, root)
        }

        fn tempdir(nome: &str) -> std::path::PathBuf {
            let path =
                std::env::temp_dir().join(format!("lume_mcp_app_{nome}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            // Il server MCP usa la radice canonica: la fixture fa lo stesso,
            // altrimenti un antenato symlink (TMPDIR su macOS) crea due forme
            // dello stesso progetto e i confronti dell'app non le uniscono.
            lume_core::canonical_path(&path).unwrap()
        }

        /// Crack privata 0700 sotto `$HOME`: la scoperta Unix rifiuta
        /// `/tmp`, che è scrivibile da altri.
        #[cfg(unix)]
        fn cartella_privata(nome: &str) -> std::path::PathBuf {
            use std::os::unix::fs::DirBuilderExt;
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .expect("HOME per la cartella dati privata");
            let path = home.join(format!(".lume_mcp_app_{nome}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&path).unwrap();
            path
        }

        fn con_mailbox(
            app: &mut Lume,
            epoca: u64,
        ) -> tokio::sync::mpsc::UnboundedReceiver<McpCommand> {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = app.update(Message::Mcp(epoca, McpEvent::Ready(tx)));
            rx
        }

        #[test]
        fn card_accodata_revocata_da_askgone_stop_e_cambio_radice() {
            for caso in ["gone", "stop", "root"] {
                let (mut app, root, epoch, mut posta) = con_client(&format!("queue_{caso}"));
                let _ = app.update(Message::MenuAction(MenuAct::About));
                let _ = app.update(Message::Mcp(
                    epoch,
                    McpEvent::Ask {
                        id: 700,
                        conn: 1,
                        query: scrivi("mai.txt", "vietato"),
                    },
                ));
                assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
                assert_eq!(app.mcp_pending(), 1);
                let seq = app.notice_seq;
                let _ = app.update(Message::Mcp(
                    epoch,
                    McpEvent::ClientConnected {
                        conn: 2,
                        name: "secondo".into(),
                    },
                ));
                assert_eq!(app.notice_seq, seq + 1); // solo la notifica di connessione
                match caso {
                    "gone" => {
                        let _ = app.update(Message::Mcp(epoch, McpEvent::AskGone(700)));
                    }
                    "stop" => {
                        let _ = app.apply_feature_command(FeatureCommand::Stop(Feature::Mcp));
                    }
                    _ => {
                        app.set_project_root(root.join("altra"));
                    }
                }
                let risposte = risposte(&mut posta);
                assert!(risposte.len() <= 1);
                let _ = app.update(Message::CloseAll);
                assert_eq!(app.shell.active_overlay(), None);
                assert!(!root.join("mai.txt").exists());
                let _ = std::fs::remove_dir_all(&root);
            }
        }

        #[test]
        fn card_accodata_mostra_la_front_corrente_dopo_askgone() {
            let (mut app, root, epoch, mut posta) = con_client("queue_front_corrente");
            let _ = app.update(Message::MenuAction(MenuAct::About));
            for id in [11, 12] {
                let _ = app.update(Message::Mcp(
                    epoch,
                    McpEvent::Ask {
                        id,
                        conn: 1,
                        query: scrivi(&format!("{id}.txt"), "vietato"),
                    },
                ));
            }
            let _ = app.update(Message::Mcp(epoch, McpEvent::AskGone(11)));
            assert_eq!(app.mcp_pending(), 1);
            let _ = app.update(Message::CloseAll);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            let card = app.mcp_card_view().unwrap();
            assert_eq!(card.position, (1, 1));
            assert_eq!(card.title, "vuole scrivere 12.txt");
            let _ = app.update(Message::CloseAll);
            assert!(matches!(risposte(&mut posta).as_slice(), [(12, Err(_))]));
            assert!(!root.join("11.txt").exists());
            assert!(!root.join("12.txt").exists());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn due_attese_promosse_da_esc_preservano_il_gate() {
            let (mut app, root, epoch, mut posta) = con_client("queue_fifo");
            let _ = app.update(Message::MenuAction(MenuAct::About));
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 701,
                    conn: 1,
                    query: scrivi("mai.txt", "vietato"),
                },
            ));
            // Solo la seconda vista statica usa il seam manager: la card è prodotta dal gate.
            app.apri_overlay(OverlayId::Scorciatoie);
            let _ = app.update(Message::CloseAll);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert_eq!(app.mcp_pending(), 1);
            let _ = app.update(Message::CloseAll);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::Scorciatoie));
            assert!(matches!(risposte(&mut posta).as_slice(), [(701, Err(_))]));
            assert!(!root.join("mai.txt").exists());
            let _ = app.update(Message::CloseAll);
            assert_eq!(app.shell.active_overlay(), None);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn x_su_card_non_nega_la_richiesta_e_annulla_la_promuove() {
            let (mut app, root, epoch, mut posta) = con_client("queue_x_card");
            let file = root.join("sporco.txt");
            std::fs::write(&file, "originale").unwrap();
            app.open_file(&file).unwrap();
            let _ = app.update(Message::EditorIn(
                app.groups.focus(),
                EditorCommand::Insert("modifica".into()),
            ));
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 702,
                    conn: 1,
                    query: scrivi("vietato.txt", "no"),
                },
            ));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
            assert_eq!(
                app.shell.active_overlay(),
                Some(OverlayId::ConfermaChiusura)
            );
            assert_eq!(app.mcp_pending(), 1);
            assert!(risposte(&mut posta).is_empty());
            let _ = app.update(Message::ConfermaChiusura(
                crate::conferma_chiusura::ConfermaMessage::Annulla,
            ));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert_eq!(app.mcp_pending(), 1);
            assert!(risposte(&mut posta).is_empty());
            let _ = app.update(Message::CloseAll);
            assert!(matches!(risposte(&mut posta).as_slice(), [(702, Err(_))]));
            assert!(!root.join("vietato.txt").exists());
            assert_eq!(std::fs::read_to_string(file).unwrap(), "originale");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn x_su_consenso_con_documento_sporco_rifiuta_e_non_avvia() {
            let (mut app, root) = app_con_cartella("queue_consenso_x");
            let file = root.join("sporco.txt");
            std::fs::write(&file, "originale").unwrap();
            app.open_file(&file).unwrap();
            let _ = app.update(Message::EditorIn(
                app.groups.focus(),
                EditorCommand::Insert("modifica".into()),
            ));
            assert!(
                app.workspace
                    .get(app.active_doc().unwrap())
                    .unwrap()
                    .is_dirty()
            );
            let _ = app.update(Message::Feature(FeatureCommand::Start(Feature::Mcp)));
            let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
            assert_eq!(
                app.shell.active_overlay(),
                Some(OverlayId::ConfermaChiusura)
            );
            assert!(app.mcp_consenso.is_none());
            assert!(app.notice.as_ref().is_some_and(|(_, n)| n.kind == NoticeKind::Info && n.text.contains("Consenso")));
            let _ = app.update(Message::ConfermaChiusura(
                crate::conferma_chiusura::ConfermaMessage::Annulla,
            ));
            let _ = app.update(Message::ConsensoMcp(ConsensoMessage::Avvia));
            assert!(app.mcp.is_none());
            assert_eq!(app.shell.active_overlay(), None);
            assert_eq!(std::fs::read_to_string(file).unwrap(), "originale");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn un_comando_in_non_brave_attende_il_si_prima_di_granted() {
            let (mut app, root, epoca) = app_con_mcp("mcp_gate_command");
            let mut posta = con_mailbox(&mut app, epoca);
            let _ = app.update(Message::Mcp(
                epoca,
                McpEvent::ClientConnected {
                    conn: 1,
                    name: "claude-code".into(),
                },
            ));
            let query = AppQuery::RunCommand {
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: PathBuf::new(),
                timeout_secs: 30,
            };
            let _ = app.update(Message::Mcp(
                epoca,
                McpEvent::Ask {
                    id: 1,
                    conn: 1,
                    query,
                },
            ));
            assert_eq!(app.mcp_mode(), Some(lume_mcp::Mode::NonBrave));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert!(app.mcp_card_view().is_some());
            assert!(posta.try_recv().is_err());
            let _ = app.update(Message::McpCard(CardMessage::Allow));
            assert!(matches!(
                posta.try_recv(),
                Ok(McpCommand::Answer {
                    id: 1,
                    answer: Ok(AppAnswer::Granted)
                })
            ));
            let _ = std::fs::remove_dir_all(&root);
        }

        fn con_client(
            nome: &str,
        ) -> (
            Lume,
            std::path::PathBuf,
            u64,
            tokio::sync::mpsc::UnboundedReceiver<McpCommand>,
        ) {
            let (mut app, root) = app_con_cartella(nome);
            let _ = app.mcp_accendi(lume_mcp::confine::canonical(&root).unwrap());
            let epoch = app.mcp.as_ref().unwrap().epoch;
            let posta = con_mailbox(&mut app, epoch);
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::ClientConnected {
                    conn: 1,
                    name: "test-agent".into(),
                },
            ));
            (app, root, epoch, posta)
        }

        #[test]
        fn il_selettore_mcp_esiste_solo_a_server_acceso_e_cambia_la_modalita() {
            let (mut app, root) = app_con_cartella("mcp_mode_statusbar");
            assert_eq!(app.mcp_mode_index(), None);
            let _ = app.mcp_accendi(lume_mcp::confine::canonical(&root).unwrap());
            let epoch = app.mcp.as_ref().unwrap().epoch;
            assert_eq!(app.mcp_mode_index(), Some(0));
            let _ = app.update(Message::Statusbar(StatusbarMessage::McpMode(2)));
            assert_eq!(app.mcp_mode(), Some(lume_mcp::Mode::Brave));
            assert_eq!(app.mcp_mode_index(), Some(2));
            let _ = app.update(Message::Statusbar(StatusbarMessage::McpMode(1)));
            assert_eq!(app.mcp_mode(), Some(lume_mcp::Mode::Auto));
            let _ = app.update(Message::Statusbar(StatusbarMessage::McpMode(200)));
            assert_eq!(app.mcp_mode(), Some(lume_mcp::Mode::NonBrave));
            assert_eq!(app.mcp.as_ref().unwrap().epoch, epoch);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn contatore_mcp_porta_la_card_davanti_all_overlay_corrente() {
            let (mut app, root, epoch, _posta) = con_client("mcp_pending_statusbar");
            app.apri_overlay(OverlayId::About);
            let command = |program: &str| AppQuery::RunCommand {
                program: program.into(),
                args: Vec::new(),
                cwd: PathBuf::new(),
                timeout_secs: 10,
            };
            for (id, program) in [(1, "one"), (2, "two")] {
                let _ = app.update(Message::Mcp(
                    epoch,
                    McpEvent::Ask {
                        id,
                        conn: 1,
                        query: command(program),
                    },
                ));
            }
            assert_eq!(app.mcp_pending(), 2);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
            let _ = app.update(Message::CloseAll);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert_eq!(app.mcp_card_view().unwrap().position, (1, 2));
            let _ = app.update(Message::Statusbar(StatusbarMessage::McpCards));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert_eq!(app.mcp_pending(), 2);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn esc_sulla_card_nega_solo_la_prima_e_riconcilia_la_successiva() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_esc_due_card");
            for id in [1, 2] {
                let _ = app.update(Message::Mcp(
                    epoch,
                    McpEvent::Ask {
                        id,
                        conn: 1,
                        query: scrivi(&format!("{id}.txt"), "testo"),
                    },
                ));
            }
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert_eq!(app.mcp_pending(), 2);
            let _ = app.update(Message::CloseAll);
            assert!(matches!(risposte(&mut posta).as_slice(), [(1, Err(_))]));
            assert_eq!(app.mcp_pending(), 1);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert_eq!(app.mcp_card_view().unwrap().position, (1, 1));
            assert!(!root.join("1.txt").exists());
            assert!(!root.join("2.txt").exists());
            let _ = app.update(Message::McpCard(CardMessage::Allow));
            assert!(matches!(
                risposte(&mut posta).as_slice(),
                [(2, Ok(AppAnswer::Json(_)))]
            ));
            assert_eq!(
                std::fs::read_to_string(root.join("2.txt")).unwrap(),
                "testo"
            );
            assert!(risposte(&mut posta).is_empty());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn gate_vuoto_chiude_solo_la_card_e_non_un_overlay_estraneo() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_gate_vuoto_overlay");
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 1,
                    conn: 1,
                    query: scrivi("uno.txt", "testo"),
                },
            ));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            let _ = app.update(Message::McpCard(CardMessage::Deny));
            assert_eq!(app.shell.active_overlay(), None);
            assert!(matches!(risposte(&mut posta).as_slice(), [(1, Err(_))]));
            app.apri_overlay(OverlayId::About);
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 2,
                    conn: 1,
                    query: scrivi("due.txt", "testo"),
                },
            ));
            assert_eq!(app.mcp_pending(), 1);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
            let _ = app.update(Message::Mcp(epoch, McpEvent::AskGone(2)));
            assert_eq!(app.mcp_pending(), 0);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
            assert!(risposte(&mut posta).is_empty());
            let _ = std::fs::remove_dir_all(&root);
        }

        fn scrivi(percorso: &str, contenuto: &str) -> AppQuery {
            AppQuery::WriteFile {
                relative: PathBuf::from(percorso),
                content: contenuto.to_owned(),
            }
        }

        fn sostituisci(percorso: &str, vecchio: &str, nuovo: &str, tutte: bool) -> AppQuery {
            AppQuery::ReplaceText {
                relative: PathBuf::from(percorso),
                old_text: vecchio.to_owned(),
                new_text: nuovo.to_owned(),
                all: tutte,
            }
        }

        fn risposte(
            posta: &mut tokio::sync::mpsc::UnboundedReceiver<McpCommand>,
        ) -> Vec<(u64, Result<AppAnswer, String>)> {
            let mut answers = Vec::new();
            while let Ok(command) = posta.try_recv() {
                if let McpCommand::Answer { id, answer } = command {
                    answers.push((id, answer));
                }
            }
            answers
        }

        fn chiedi(
            app: &mut Lume,
            epoch: u64,
            posta: &mut tokio::sync::mpsc::UnboundedReceiver<McpCommand>,
            id: u64,
            query: AppQuery,
        ) -> Result<AppAnswer, String> {
            let _ = app.update(Message::Mcp(epoch, McpEvent::Ask { id, conn: 1, query }));
            let mut answers = risposte(posta);
            assert_eq!(answers.len(), 1, "attesa una risposta: {answers:?}");
            let (answered_id, answer) = answers.remove(0);
            assert_eq!(answered_id, id);
            answer
        }

        fn testo_del_buffer(app: &Lume) -> String {
            let id = app.active_doc().unwrap();
            app.workspace.get(id).unwrap().rope().to_string()
        }

        #[test]
        fn write_file_crea_e_registra_senza_memorizzare_il_contenuto() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_write_create");
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let value = json(chiedi(
                &mut app,
                epoch,
                &mut posta,
                1,
                scrivi("nuovo.txt", "segreto-123\n"),
            ));
            assert_eq!(value["path"], "nuovo.txt");
            assert_eq!(value["bytes"], 12);
            assert_eq!(value["created"], true);
            assert_eq!(value["open_in_editor"], false);
            assert_eq!(
                std::fs::read_to_string(root.join("nuovo.txt")).unwrap(),
                "segreto-123\n"
            );
            let entry = app
                .history
                .iter()
                .find(|entry| entry.action == lume_store::HistoryAction::AgentWrote)
                .unwrap();
            assert_eq!(entry.label, "nuovo.txt");
            assert!(
                app.history
                    .iter()
                    .all(|entry| !entry.label.contains("segreto"))
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn write_file_attende_il_consenso_e_il_diniego_non_crea_file() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_write_consent");
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 1,
                    conn: 1,
                    query: scrivi("a.txt", "no"),
                },
            ));
            assert!(risposte(&mut posta).is_empty());
            assert!(!root.join("a.txt").exists());
            let _ = app.update(Message::McpCard(CardMessage::Deny));
            assert!(matches!(risposte(&mut posta).as_slice(), [(1, Err(_))]));
            assert!(!root.join("a.txt").exists());
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 2,
                    conn: 1,
                    query: scrivi("b.txt", "si"),
                },
            ));
            let _ = app.update(Message::McpCard(CardMessage::Allow));
            assert!(matches!(
                risposte(&mut posta).as_slice(),
                [(2, Ok(AppAnswer::Json(_)))]
            ));
            assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "si");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn write_file_rifiuta_buffer_dirty_e_non_modifica_il_disco() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_write_dirty");
            std::fs::write(root.join("a.txt"), "originale\n").unwrap();
            app.open_file(&root.join("a.txt")).unwrap();
            let _ = app.apply_editor_command(EditorCommand::Insert("mio ".to_owned()));
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let error =
                chiedi(&mut app, epoch, &mut posta, 1, scrivi("a.txt", "agente\n")).unwrap_err();
            assert!(error.contains("modifiche non salvate"), "{error}");
            assert_eq!(
                std::fs::read_to_string(root.join("a.txt")).unwrap(),
                "originale\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn write_file_auto_scrive_dentro_radice_ma_non_crea_cartelle_o_supera_il_tetto() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_write_limits");
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Auto));
            assert!(chiedi(&mut app, epoch, &mut posta, 1, scrivi("ok.txt", "x")).is_ok());
            let nested = chiedi(&mut app, epoch, &mut posta, 2, scrivi("manca/x.txt", "x"));
            assert!(nested.is_err());
            assert!(!root.join("manca").exists());
            let oversized = "x".repeat(lume_mcp::edit::MAX_WRITE_BYTES + 1);
            assert!(
                chiedi(
                    &mut app,
                    epoch,
                    &mut posta,
                    3,
                    scrivi("grande.txt", &oversized)
                )
                .is_err()
            );
            assert!(!root.join("grande.txt").exists());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn replace_text_ambiguo_o_assente_non_scrive_e_non_registra() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_replace_errors");
            std::fs::write(root.join("a.txt"), "x x\n").unwrap();
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let ambiguous = chiedi(
                &mut app,
                epoch,
                &mut posta,
                1,
                sostituisci("a.txt", "x", "y", false),
            )
            .unwrap_err();
            assert!(ambiguous.contains('2'), "{ambiguous}");
            assert!(
                chiedi(
                    &mut app,
                    epoch,
                    &mut posta,
                    2,
                    sostituisci("a.txt", "missing", "y", false)
                )
                .is_err()
            );
            assert_eq!(
                std::fs::read_to_string(root.join("a.txt")).unwrap(),
                "x x\n"
            );
            assert!(
                app.history
                    .iter()
                    .all(|entry| entry.action != lume_store::HistoryAction::AgentWrote)
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn replace_text_conserva_crlf_su_file_non_aperto() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_replace_crlf");
            std::fs::write(root.join("a.txt"), "uno\r\ndue\r\ntre\r\n").unwrap();
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let answer = chiedi(
                &mut app,
                epoch,
                &mut posta,
                1,
                sostituisci("a.txt", "uno\ndue", "1\n2", false),
            );
            assert!(answer.is_ok(), "{answer:?}");
            assert_eq!(
                std::fs::read(root.join("a.txt")).unwrap(),
                b"1\r\n2\r\ntre\r\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn replace_text_su_buffer_aperto_lo_ricarica_pulito() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_replace_open");
            std::fs::write(root.join("a.txt"), "ciao mondo\n").unwrap();
            app.open_file(&root.join("a.txt")).unwrap();
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let task = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 1,
                    conn: 1,
                    query: sostituisci("a.txt", "mondo", "Lume", false),
                },
            ));
            assert!(matches!(
                risposte(&mut posta).as_slice(),
                [(1, Ok(AppAnswer::Json(_)))]
            ));
            let message = task_message(task);
            let _ = app.update(message);
            assert_eq!(testo_del_buffer(&app), "ciao Lume\n");
            assert!(
                !app.workspace
                    .get(app.active_doc().unwrap())
                    .unwrap()
                    .is_dirty()
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn write_file_su_buffer_aperto_lo_ricarica_pulito() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_write_open");
            std::fs::write(root.join("a.txt"), "prima\n").unwrap();
            app.open_file(&root.join("a.txt")).unwrap();
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let task = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 1,
                    conn: 1,
                    query: scrivi("a.txt", "dopo\n"),
                },
            ));
            assert!(matches!(
                risposte(&mut posta).as_slice(),
                [(1, Ok(AppAnswer::Json(_)))]
            ));
            let message = task_message(task);
            let _ = app.update(message);
            assert_eq!(testo_del_buffer(&app), "dopo\n");
            let doc = app.workspace.get(app.active_doc().unwrap()).unwrap();
            assert!(!doc.is_dirty());
            assert!(
                app.disk_fingerprints
                    .contains_key(&app.active_doc().unwrap())
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn run_command_consentito_registra_solo_il_programma() {
            let (mut app, root, epoch, mut posta) = con_client("mcp_run_history");
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let query = AppQuery::RunCommand {
                program: "cargo".into(),
                args: vec!["--token".into(), "secret".into()],
                cwd: PathBuf::new(),
                timeout_secs: 5,
            };
            assert!(matches!(
                chiedi(&mut app, epoch, &mut posta, 1, query),
                Ok(AppAnswer::Granted)
            ));
            let entry = app
                .history
                .iter()
                .find(|entry| entry.action == lume_store::HistoryAction::AgentRan)
                .unwrap();
            assert_eq!(entry.label, "cargo");
            assert!(entry.path.is_none());
            let _ = std::fs::remove_dir_all(&root);
        }

        fn cambi(
            posta: &mut tokio::sync::mpsc::UnboundedReceiver<McpCommand>,
        ) -> Vec<Availability> {
            let mut values = Vec::new();
            while let Ok(command) = posta.try_recv() {
                if let McpCommand::ToolsChanged(availability) = command {
                    values.push(availability);
                }
            }
            values
        }

        fn json(answer: Result<AppAnswer, String>) -> serde_json::Value {
            match answer {
                Ok(AppAnswer::Json(value)) => value,
                other => panic!("atteso JSON, ricevuto {other:?}"),
            }
        }

        fn task_message(task: iced::Task<Message>) -> Message {
            use futures::StreamExt;
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let mut stream = iced_runtime::task::into_stream(task).unwrap();
                    while let Some(action) = stream.next().await {
                        if let iced_runtime::Action::Output(message) = action {
                            return message;
                        }
                    }
                    panic!("il Task non ha prodotto un messaggio")
                })
        }

        /// Un comando LSP atteso dalla fixture: la scadenza impedisce che un
        /// instradamento rotto faccia restare la suite appesa per sempre.
        fn comando_lsp(ricevitore: &mut lume_lsp::LspCommandReceiver) -> lume_lsp::LspCommand {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(5), ricevitore.recv())
                        .await
                        .expect("comando LSP entro la scadenza")
                        .expect("mailbox LSP chiusa")
                })
        }

        fn lsp_app_con_mcp(nome: &str) -> (Lume, std::path::PathBuf, u64, u64) {
            use crate::lsp_mode::{LspState, LspWorkspace};
            let root = tempdir(nome);
            std::fs::write(root.join("main.rs"), "fn mian() {}\n").unwrap();
            let mut app = app();
            app.set_project_root(root.clone());
            let spec = lume_language_servers::ServerCatalog::embedded()
                .unwrap()
                .get("rust-analyzer")
                .unwrap();
            let mut lsp = LspWorkspace::new(spec, root.clone());
            lsp.command = Some(lume_lsp::ServerCommand {
                program: "rust-analyzer".into(),
                args: Vec::new(),
                cwd: None,
                env: Vec::new(),
                source: lume_lsp::CommandSource::SystemPath,
            });
            lsp.state = LspState::Avvio;
            let lsp_epoch = lsp.bump();
            app.lsp.insert(lsp);
            let _ = app.registry.set_feature_enabled("mcp", true);
            let _ = app.mcp_accendi(lume_mcp::confine::canonical(&root).unwrap());
            let mcp_epoch = app.mcp.as_ref().unwrap().epoch;
            let (mailbox, _receiver) = lume_lsp::LspCommandMailbox::channel();
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                crate::lsp_mode::LspEvent::Ready(mailbox),
            ));
            let ready = lume_lsp::SessionReady {
                capabilities: lume_lsp::NegotiatedCapabilities {
                    server: lsp_types::ServerCapabilities::default(),
                    sync_kind: lsp_types::TextDocumentSyncKind::FULL,
                },
                peak_memory_bytes: None,
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(ready))),
            ));
            (app, root, mcp_epoch, lsp_epoch)
        }

        fn app_con_mcp_e_rinomina(
            nome: &str,
        ) -> (
            Lume,
            std::path::PathBuf,
            u64,
            u64,
            lume_lsp::LspCommandReceiver,
            tokio::sync::mpsc::UnboundedReceiver<McpCommand>,
        ) {
            let (mut app, root, mcp_epoch, lsp_epoch) = lsp_app_con_mcp(nome);
            app.lsp
                .get_mut("rust-analyzer")
                .unwrap()
                .availability
                .rename = true;
            let (mailbox, lsp_commands) = lume_lsp::LspCommandMailbox::channel();
            app.lsp.get_mut("rust-analyzer").unwrap().mailbox = Some(mailbox);
            app.open_file(&root.join("main.rs")).unwrap();
            let mut mcp_commands = con_mailbox(&mut app, mcp_epoch);
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::ClientConnected {
                    conn: 1,
                    name: "test-agent".into(),
                },
            ));
            while mcp_commands.try_recv().is_ok() {}
            (app, root, mcp_epoch, lsp_epoch, lsp_commands, mcp_commands)
        }

        fn diagnostic(root: &std::path::Path) -> lsp_types::PublishDiagnosticsParams {
            lsp_types::PublishDiagnosticsParams {
                uri: crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                diagnostics: vec![lsp_types::Diagnostic {
                    range: lsp_types::Range::new(
                        lsp_types::Position::new(0, 3),
                        lsp_types::Position::new(0, 7),
                    ),
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    message: "cannot find function `mian`".into(),
                    ..lsp_types::Diagnostic::default()
                }],
                version: None,
            }
        }

        pub(super) fn app_con_mcp(nome: &str) -> (Lume, std::path::PathBuf, u64) {
            let root = tempdir(nome);
            let mut app = app();
            app.set_project_root(root.clone());
            let _ = app.registry.set_feature_enabled("mcp", true);
            let _ = app.mcp_accendi(lume_mcp::confine::canonical(&root).unwrap());
            let epoca = app.mcp.as_ref().expect("acceso").epoch;
            (app, root, epoca)
        }

        #[test]
        fn spento_non_c_e_il_ramo_acceso_c_e_e_conta_uno() {
            let spento = app();
            assert!(spento.mcp_spawn().is_none());
            assert_eq!(spento.feature_live_count(Feature::Mcp), Some(0));
            let (acceso, root, _) = app_con_mcp("mcp_ramo");
            assert!(acceso.mcp_spawn().is_some());
            assert_eq!(acceso.feature_live_count(Feature::Mcp), Some(1));
            assert!(!acceso.feature_can_start(Feature::Mcp));
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn ferma_toglie_il_ramo_e_lo_dice() {
            let (mut app, root, _) = app_con_mcp("mcp_ferma");
            let _ = app.apply_feature_command(FeatureCommand::Stop(Feature::Mcp));
            assert!(app.mcp.is_none());
            assert!(app.mcp_spawn().is_none());
            assert_eq!(
                app.notice.as_ref().map(|(_, n)| n.text.as_str()),
                Some("Server MCP fermato")
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn cambiare_radice_spegne_il_server_precedente() {
            let (mut app, root, _) = app_con_mcp("mcp_radice");
            let altra = tempdir("mcp_radice_2");
            app.set_project_root(altra.clone());
            assert!(app.mcp.is_none());
            assert!(app.mcp_spawn().is_none());
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&altra);
        }

        #[test]
        fn eventi_tardivi_della_vecchia_epoca_si_ignorano() {
            let (mut app, root, vecchia) = app_con_mcp("mcp_epoca");
            let token_vecchio = app.mcp.as_ref().unwrap().token.clone();
            let _ = app.apply_feature_command(FeatureCommand::Stop(Feature::Mcp));
            let _ = app.mcp_accendi(lume_mcp::confine::canonical(&root).unwrap());
            let nuova = app.mcp.as_ref().unwrap();
            assert_ne!(nuova.epoch, vecchia);
            assert_ne!(nuova.token, token_vecchio);
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = app.update(Message::Mcp(vecchia, McpEvent::Ready(tx)));
            let _ = app.update(Message::Mcp(
                vecchia,
                McpEvent::ClientConnected {
                    conn: 1,
                    name: "fantasma".into(),
                },
            ));
            let _ = app.update(Message::Mcp(vecchia, McpEvent::Failed("vecchio".into())));
            let viva = app
                .mcp
                .as_ref()
                .expect("l'epoca vecchia non spegne quella nuova");
            assert!(viva.mailbox.is_none());
            assert!(viva.clients.is_empty());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn clienti_conteggiati_e_failed_spegne_il_server() {
            let (mut app, root, epoca) = app_con_mcp("mcp_client");
            let _ = app.update(Message::Mcp(
                epoca,
                McpEvent::ClientConnected {
                    conn: 1,
                    name: "claude-code".into(),
                },
            ));
            let _ = app.update(Message::Mcp(
                epoca,
                McpEvent::ClientConnected {
                    conn: 2,
                    name: "codex".into(),
                },
            ));
            assert_eq!(app.mcp.as_ref().unwrap().clients.len(), 2);
            let _ = app.update(Message::Mcp(epoca, McpEvent::ClientGone(1)));
            assert_eq!(
                app.mcp
                    .as_ref()
                    .unwrap()
                    .clients
                    .values()
                    .collect::<Vec<_>>(),
                ["codex"]
            );
            let _ = app.update(Message::Mcp(
                epoca,
                McpEvent::Failed("pipe occupata".into()),
            ));
            assert!(app.mcp.is_none());
            assert!(
                app.notice
                    .as_ref()
                    .is_some_and(|(_, n)| n.text.contains("pipe occupata"))
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn avvia_senza_radice_mostra_un_avviso() {
            let mut app = app();
            let _ = app.registry.set_feature_enabled("mcp", true);
            assert!(!app.feature_can_start(Feature::Mcp));
            let _ = app.apply_feature_command(FeatureCommand::Start(Feature::Mcp));
            assert!(app.mcp.is_none());
            assert!(
                app.notice
                    .as_ref()
                    .is_some_and(|(_, n)| n.text.contains("Apri una cartella"))
            );
        }

        #[test]
        fn esci_rimuove_il_record_di_scoperta() {
            let (mut app, root, _) = app_con_mcp("mcp_esci");
            #[cfg(windows)]
            let (dati, path) = {
                let dati = tempdir("mcp_esci_dati");
                let nonce = lume_mcp::discovery::new_nonce().unwrap();
                app.mcp.as_mut().unwrap().endpoint = lume_mcp::server::Endpoint::Pipe {
                    name: lume_mcp::discovery::pipe_name(std::process::id(), &nonce),
                    discovery_dir: Some(dati.clone()),
                };
                let path = lume_mcp::discovery::record_path(&dati, std::process::id());
                std::fs::write(&path, "{}").unwrap();
                (dati, path)
            };
            #[cfg(unix)]
            let (dati, path) = {
                let dati = cartella_privata("mcp_esci");
                let endpoint = lume_mcp::unix::Endpoint::with_identity(
                    lume_mcp::unix::discovery_dir(&dati),
                    lume_mcp::unix::Identity::generate().unwrap(),
                )
                .unwrap();
                lume_mcp::unix::ensure_private_dir(endpoint.dir()).unwrap();
                let token = app.mcp.as_ref().unwrap().token.clone();
                let record = lume_mcp::unix::Record::new(
                    &endpoint,
                    root.clone(),
                    token,
                    "0.0.0-prova".to_owned(),
                    &std::fs::symlink_metadata(&root).unwrap(),
                );
                let guard = lume_mcp::unix::DiscoveryFile::publish(&endpoint, &record).unwrap();
                std::mem::forget(guard);
                let path = endpoint.record_path();
                app.mcp.as_mut().unwrap().endpoint = lume_mcp::server::Endpoint::Unix(endpoint);
                (dati, path)
            };

            let _ = app.esci(None);

            assert!(
                !path.exists(),
                "l'uscita pulisce il record anche se lo stream resta vivo"
            );
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&dati);
        }

        #[tokio::test]
        async fn tracker_mcp_rimuove_il_server_della_recipe() {
            use futures::StreamExt;
            use iced_runtime::futures::subscription::{Tracker, into_recipes};

            let (mut app, root, _) = app_con_mcp("mcp_tracker_teardown");
            #[cfg(windows)]
            let (discovery, record) = {
                let discovery = tempfile::tempdir().unwrap();
                let nonce = lume_mcp::discovery::new_nonce().unwrap();
                let endpoint = lume_mcp::server::Endpoint::Pipe {
                    name: lume_mcp::discovery::pipe_name(std::process::id(), &nonce),
                    discovery_dir: Some(discovery.path().to_path_buf()),
                };
                app.mcp.as_mut().unwrap().endpoint = endpoint;
                let record = lume_mcp::discovery::record_path(discovery.path(), std::process::id());
                (discovery, record)
            };
            #[cfg(unix)]
            let (discovery, record) = {
                let discovery = cartella_privata("mcp_tracker_teardown");
                let endpoint = lume_mcp::unix::Endpoint::with_identity(
                    lume_mcp::unix::discovery_dir(&discovery),
                    lume_mcp::unix::Identity::generate().unwrap(),
                )
                .unwrap();
                let record = endpoint.record_path();
                app.mcp.as_mut().unwrap().endpoint = lume_mcp::server::Endpoint::Unix(endpoint);
                (discovery, record)
            };
            let recipe = into_recipes(mcp_subscription(app.mcp_spawn().unwrap()));
            assert_eq!(recipe.len(), 1);

            let mut tracker = Tracker::new();
            let (sender, mut receiver) = futures::channel::mpsc::channel::<Message>(16);
            let mut executions = tracker.update(recipe.into_iter(), sender.clone());
            assert_eq!(executions.len(), 1);
            let execution = tokio::spawn(executions.pop().unwrap());

            let evento = tokio::time::timeout(std::time::Duration::from_secs(10), receiver.next())
                .await
                .expect("la recipe MCP produce il primo evento")
                .expect("il receiver resta collegato");
            #[cfg(windows)]
            {
                assert!(matches!(&evento, Message::Mcp(_, McpEvent::Ready(_))));
                assert!(record.exists(), "Ready segue la scrittura del discovery");
            }
            #[cfg(unix)]
            {
                assert!(
                    matches!(&evento, Message::Mcp(_, McpEvent::Ready(_))),
                    "il server Unix parte: {evento:?}"
                );
                assert!(record.exists(), "Ready segue la scrittura del discovery");
            }

            assert!(tracker.update(std::iter::empty(), sender).is_empty());
            tokio::time::timeout(std::time::Duration::from_secs(10), execution)
                .await
                .expect("la recipe termina quando Tracker la rimuove")
                .unwrap();
            assert!(
                !record.exists(),
                "il file discovery viene rimosso al teardown"
            );
            let _ = std::fs::remove_dir_all(root);
            let _ = std::fs::remove_dir_all(&discovery);
        }

        #[test]
        fn list_open_files_indica_percorsi_dirty_e_file_attivo() {
            let (mut app, root, _) = app_con_mcp("mcp_aperti");
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
            std::fs::write(root.join("b.txt"), "bi\n").unwrap();
            app.open_file(&root.join("src/a.rs")).unwrap();
            let _ = app.apply_editor_command(EditorCommand::Insert("x".into()));
            app.open_file(&root.join("b.txt")).unwrap();
            let value = json(app.mcp_answer(AppQuery::OpenFiles));
            let files = value["files"].as_array().unwrap();
            assert_eq!(files.len(), 2);
            let a = files
                .iter()
                .find(|file| file["path"] == "src/a.rs")
                .unwrap();
            assert_eq!(a["dirty"], true);
            assert_eq!(a["active"], false);
            assert_eq!(
                files.iter().find(|file| file["path"] == "b.txt").unwrap()["active"],
                true
            );
            assert_eq!(value["outside_root"], 0);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn get_active_file_e_buffer_restituiscono_il_testo_sporco() {
            let (mut app, root, _) = app_con_mcp("mcp_attivo");
            std::fs::write(root.join("c.txt"), "uno\ndue\n").unwrap();
            assert!(app.mcp_answer(AppQuery::ActiveFile).is_err());
            app.open_file(&root.join("c.txt")).unwrap();
            let _ = app.apply_editor_command(EditorCommand::Insert("zero\n".into()));
            let active = json(app.mcp_answer(AppQuery::ActiveFile));
            assert_eq!(active["path"], "c.txt");
            assert_eq!(active["dirty"], true);
            let Ok(AppAnswer::Buffer(Some(buffer))) = app.mcp_answer(AppQuery::Buffer {
                relative: "c.txt".into(),
            }) else {
                panic!("il file aperto deve restituire il buffer")
            };
            assert_eq!(buffer.text, "zero\nuno\ndue\n");
            assert!(buffer.dirty);
            assert!(matches!(
                app.mcp_answer(AppQuery::Buffer {
                    relative: "mai-aperto.txt".into()
                }),
                Ok(AppAnswer::Buffer(None))
            ));
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn get_diagnostics_usa_solo_lsp_attivi_e_filtra_la_gravita() {
            let (mut app, root, _, lsp_epoch) = lsp_app_con_mcp("mcp_diagnostiche");
            assert!(
                app.mcp_answer(AppQuery::Diagnostics {
                    relative: None,
                    max_severity: 4,
                })
                .is_ok()
            );
            let query = AppQuery::Diagnostics {
                relative: None,
                max_severity: 4,
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Diagnostics(Box::new(
                    diagnostic(&root),
                ))),
            ));
            let value = json(app.mcp_answer(query));
            let first = &value["diagnostics"][0];
            assert_eq!(first["path"], "main.rs");
            assert_eq!(first["line"], 1);
            assert_eq!(first["column_utf16"], 4);
            assert_eq!(first["severity"], "error");
            assert!(first["message"].as_str().unwrap().contains("mian"));
            let hints = json(app.mcp_answer(AppQuery::Diagnostics {
                relative: None,
                max_severity: 0,
            }));
            assert_eq!(hints["diagnostics"], serde_json::json!([]));
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn diagnostiche_uniscono_server_e_restringono_dopo_ferma() {
            use crate::lsp_mode::{LspState, LspWorkspace};
            let (mut app, root, _, ra_epoch) = lsp_app_con_mcp("mcp_diagnostiche_due");
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                ra_epoch,
                crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Diagnostics(Box::new(
                    diagnostic(&root),
                ))),
            ));
            let tombi = lume_language_servers::ServerCatalog::embedded()
                .unwrap()
                .get("tombi")
                .unwrap();
            let mut second = LspWorkspace::new(tombi, root.clone());
            second.state = LspState::Attivo {
                version: String::new(),
                memory_mb: None,
            };
            let tombi_epoch = second.bump();
            app.lsp.insert(second);
            let mut params = diagnostic(&root);
            params.uri = crate::lsp_uri::path_to_uri(&root.join("Cargo.toml")).unwrap();
            let _ = app.update(Message::Lsp(
                "tombi".into(),
                tombi_epoch,
                crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Diagnostics(Box::new(
                    params,
                ))),
            ));
            let query = AppQuery::Diagnostics {
                relative: None,
                max_severity: 4,
            };
            let value = json(app.mcp_answer(query.clone()));
            let diagnostics = value["diagnostics"].as_array().unwrap();
            assert_eq!(diagnostics.len(), 2);
            assert!(diagnostics.iter().any(|item| item["path"] == "main.rs"));
            assert!(diagnostics.iter().any(|item| item["path"] == "Cargo.toml"));

            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                ra_epoch,
                crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Stopped),
            ));
            assert!(app.mcp_availability().diagnostics);
            let remaining = json(app.mcp_answer(query));
            assert_eq!(remaining["diagnostics"].as_array().unwrap().len(), 1);
            assert_eq!(remaining["diagnostics"][0]["path"], "Cargo.toml");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn ask_tardiva_dopo_ferma_non_apre_un_file() {
            let (mut app, root, epoch) = app_con_mcp("mcp_ask_stale");
            std::fs::write(root.join("stale.txt"), "no\n").unwrap();
            let _ = app.apply_feature_command(FeatureCommand::Stop(Feature::Mcp));
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 99,
                    conn: 1,
                    query: AppQuery::OpenInEditor {
                        relative: "stale.txt".into(),
                        line: None,
                    },
                },
            ));
            assert!(app.active_path().is_none());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn avvia_chiede_consenso_e_nomina_la_radice() {
            let (mut app, root) = app_con_cartella("mcp_consenso");
            let _ = app.apply_feature_command(FeatureCommand::Start(Feature::Mcp));
            assert!(app.mcp.is_none(), "il server non parte prima del consenso");
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConsensoMcp));
            let dialog = app.riquadro_consenso_mcp().unwrap();
            let canonical = lume_mcp::confine::canonical(&root).unwrap();
            assert_eq!(dialog.radice, canonical.display().to_string());
            assert!(crate::consenso_mcp::titolo(&dialog).contains(&dialog.radice));
            let _ = app.update(Message::ConsensoMcp(ConsensoMessage::Avvia));
            assert!(app.mcp_spawn().is_some());
            assert_eq!(app.shell.active_overlay(), None);
            assert!(app.riquadro_consenso_mcp().is_none());
            let _ = std::fs::remove_dir_all(&root);
        }

        fn azione_aiuto_rifiutata_sotto_consenso(nome: &str, azione: MenuAct) {
            let (mut app, root) = app_con_cartella(nome);
            let _ = app.update(Message::Feature(FeatureCommand::Start(Feature::Mcp)));
            assert!(app.riquadro_consenso_mcp().is_some());
            let _ = app.update(Message::MenuAction(azione));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConsensoMcp));
            assert!(app.mcp_consenso.is_some());
            assert!(
                app.notice
                    .as_ref()
                    .is_some_and(|(_, n)| n.text.contains("Rispondi prima"))
            );
            // Il gesto rifiutato non è accodato; Esc revoca il consenso.
            let _ = app.update(Message::CloseAll);
            assert!(app.mcp_consenso.is_none());
            assert_eq!(app.shell.active_overlay(), None);
            let _ = app.update(Message::ConsensoMcp(ConsensoMessage::Avvia));
            assert!(app.mcp.is_none(), "la risposta scaduta non avvia il server");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn informazioni_rifiutate_sotto_consenso() {
            azione_aiuto_rifiutata_sotto_consenso("about", MenuAct::About);
        }

        #[test]
        fn scorciatoie_rifiutate_sotto_consenso() {
            azione_aiuto_rifiutata_sotto_consenso("scorciatoie", MenuAct::Shortcuts);
        }

        #[test]
        fn avvio_mcp_pulisce_gli_stati_precedenti_prima_del_consenso() {
            let (mut app, root) = app_con_cartella("mcp_pulizia_avvio");
            std::fs::write(root.join("file.txt"), "testo\n").unwrap();
            app.open_file(&root.join("file.txt")).unwrap();
            let id = app.active_doc().unwrap();
            let _ = app.open_search(SearchTab::Tutto);
            let _ricerca = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
                "file".into(),
            )));
            assert!(app.global_search.is_file_searching());
            assert!(app.global_file_search_worker.is_some());
            let _ = app.update(Message::MenuAction(MenuAct::Find));
            assert!(app.find.is_some());
            app.menu_bar.update(MenuBarMessage::ToggleRoot);
            app.environment_picker
                .update(EnvironmentPickerMessage::Toggle);
            app.settings_panel.update(SettingsMessage::Open);
            app.rename = Some(crate::rename_box::RenameBox::new(
                id,
                lsp_types::Position::new(0, 0),
                0,
                "testo".into(),
            ));
            app.goto_line = Some("3".into());
            app.context_menu = Some(crate::context_menu::ContextMenu {
                x: 0.0,
                y: 0.0,
                soggetto: crate::context_menu::Soggetto::Editor {
                    pos: 0,
                    selezione: None,
                },
            });
            let generazione = app
                .global_search_gen
                .load(std::sync::atomic::Ordering::SeqCst);

            let _ = app.update(Message::Feature(FeatureCommand::Start(Feature::Mcp)));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConsensoMcp));
            assert!(app.riquadro_consenso_mcp().is_some());
            assert!(!app.menu_bar.is_open());
            assert!(!app.environment_picker.is_open());
            assert!(!app.settings_panel.is_open());
            assert!(!app.global_search.is_open());
            assert!(!app.global_search.is_file_searching());
            assert!(
                app.global_search_gen
                    .load(std::sync::atomic::Ordering::SeqCst)
                    > generazione
            );
            assert!(app.rename.is_none());
            assert!(app.goto_line.is_none());
            assert!(app.find.is_none());
            assert!(app.context_menu.is_none());
            assert!(app.mcp.is_none());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn esc_e_annulla_non_accendono_il_server() {
            let (mut app, root) = app_con_cartella("mcp_annulla");
            let _ = app.apply_feature_command(FeatureCommand::Start(Feature::Mcp));
            let _ = app.update(Message::CloseAll);
            assert!(app.mcp.is_none());
            assert!(app.riquadro_consenso_mcp().is_none());
            let _ = app.update(Message::ConsensoMcp(ConsensoMessage::Avvia));
            assert!(app.mcp.is_none(), "una risposta scaduta non avvia MCP");
            let _ = app.apply_feature_command(FeatureCommand::Start(Feature::Mcp));
            let _ = app.update(Message::ConsensoMcp(ConsensoMessage::Annulla));
            assert!(app.mcp.is_none());
            assert_eq!(app.shell.active_overlay(), None);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn overlay_consenso_impedisce_che_invio_modifichi_il_file() {
            let (mut app, root) = app_con_cartella("mcp_tastiera");
            std::fs::write(root.join("f.txt"), "ciao\n").unwrap();
            app.open_file(&root.join("f.txt")).unwrap();
            let id = app.active_doc().unwrap();
            let _ = app.apply_feature_command(FeatureCommand::Start(Feature::Mcp));
            let _ = app.update(Message::EditorIn(
                crate::editor_groups::GroupId::Primary,
                EditorCommand::Insert("\n".into()),
            ));
            assert!(!app.workspace.get(id).unwrap().is_dirty());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn cambio_radice_revoca_il_consenso_in_sospeso() {
            let (mut app, root) = app_con_cartella("mcp_consenso_radice");
            let _ = app.apply_feature_command(FeatureCommand::Start(Feature::Mcp));
            let other = tempdir("mcp_consenso_radice_2");
            app.set_project_root(other.clone());
            assert!(app.riquadro_consenso_mcp().is_none());
            assert_eq!(app.shell.active_overlay(), None);
            let _ = app.update(Message::ConsensoMcp(ConsensoMessage::Avvia));
            assert!(app.mcp.is_none());
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&other);
        }

        #[test]
        fn apertura_non_modale_rifiutata_conserva_il_consenso() {
            let (mut app, root) = app_con_cartella("mcp_overlay_sostituito");
            let _ = app.apply_feature_command(FeatureCommand::Start(Feature::Mcp));
            app.apri_overlay(OverlayId::Search);
            assert!(app.riquadro_consenso_mcp().is_some());
            assert!(
                app.notice
                    .as_ref()
                    .is_some_and(|(_, n)| n.kind == NoticeKind::Info)
            );
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConsensoMcp));
            assert!(app.riquadro_consenso_mcp().is_some());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn open_in_editor_apre_sulla_riga_e_aggiunge_una_cronologia() {
            let (mut app, root, epoch) = app_con_mcp("mcp_apri");
            std::fs::write(root.join("d.txt"), "uno\ndue\ntre\n").unwrap();
            let mut mailbox = con_mailbox(&mut app, epoch);
            let before = app.history.len();
            let task = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 81,
                    conn: 1,
                    query: AppQuery::OpenInEditor {
                        relative: "d.txt".into(),
                        line: Some(3),
                    },
                },
            ));
            assert!(
                app.active_path().is_none(),
                "l'apertura aspetta la lettura worker"
            );
            assert!(
                mailbox.try_recv().is_err(),
                "il client aspetta l'esito reale dell'apertura"
            );
            let reading = task_message(task);
            let _ = app.update(reading);
            let value = mailbox.try_recv().unwrap();
            let McpCommand::Answer { id, answer } = value else {
                panic!("attesa risposta alla domanda MCP")
            };
            assert_eq!(id, 81);
            let value = json(answer);
            assert_eq!(value["opened"], "d.txt");
            assert_eq!(app.active_path(), Some(root.join("d.txt").as_path()));
            let doc = app.workspace.get(app.active_doc().unwrap()).unwrap();
            assert_eq!(doc.rope().char_to_line(doc.selection().head), 2);
            assert_eq!(app.history.len(), before + 1);
            assert_eq!(app.history[0].action, lume_store::HistoryAction::Opened);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn open_sql_file_mcp_lo_apre_nell_editor_senza_eseguirlo() {
            let (mut app, root, epoch) = app_con_mcp("mcp_apri_sql");
            std::fs::create_dir_all(root.join("schema")).unwrap();
            let sql = "SELECT 1;\n";
            std::fs::write(root.join("schema/procedura.sql"), sql).unwrap();
            let mut mailbox = con_mailbox(&mut app, epoch);
            let task = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 82,
                    conn: 1,
                    query: AppQuery::OpenInEditor {
                        relative: "schema/procedura.sql".into(),
                        line: Some(1),
                    },
                },
            ));
            let reading = task_message(task);
            let _ = app.update(reading);
            let answer = mailbox.try_recv().unwrap();
            let McpCommand::Answer { id, answer } = answer else {
                panic!("attesa risposta all'apertura MCP del file SQL")
            };
            assert_eq!(id, 82);
            assert_eq!(json(answer)["opened"], "schema/procedura.sql");
            assert_eq!(
                app.active_path(),
                Some(root.join("schema/procedura.sql").as_path())
            );
            let document = app.workspace.get(app.active_doc().unwrap()).unwrap();
            assert_eq!(document.rope().to_string(), sql);
            #[cfg(feature = "sql")]
            assert!(
                app.sql_workspace().is_none(),
                "aprire il file non avvia SQL"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn ask_risponde_sulla_mailbox_dello_stream() {
            let (mut app, root, epoch) = app_con_mcp("mcp_ask");
            let mut mailbox = con_mailbox(&mut app, epoch);
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 7,
                    conn: 1,
                    query: AppQuery::OpenFiles,
                },
            ));
            let (id, answer) = mailbox
                .try_recv()
                .ok()
                .and_then(|command| match command {
                    McpCommand::Answer { id, answer } => Some((id, answer)),
                    _ => None,
                })
                .expect("risposta MCP");
            assert_eq!(id, 7);
            assert!(matches!(answer, Ok(AppAnswer::Json(_))));
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn tools_changed_esce_solo_quando_cambia_git_o_lsp() {
            let (mut app, root, mcp_epoch, lsp_epoch) = lsp_app_con_mcp("mcp_tools_changed");
            let mut mailbox = con_mailbox(&mut app, mcp_epoch);
            assert_eq!(
                cambi(&mut mailbox),
                [Availability {
                    git: true,
                    diagnostics: true,
                    ..Availability::default()
                }],
                "Ready aggiorna la fotografia cambiata dallo spawn"
            );
            assert!(app.mcp_availability().diagnostics);
            let _ = app.apply_feature_command(FeatureCommand::SetEnabled(Feature::Git, false));
            assert_eq!(
                cambi(&mut mailbox),
                [Availability {
                    git: false,
                    diagnostics: true,
                    ..Availability::default()
                }]
            );
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Stopped),
            ));
            assert_eq!(
                cambi(&mut mailbox),
                [Availability {
                    git: false,
                    diagnostics: false,
                    ..Availability::default()
                }]
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn richiesta_lsp_mcp_torna_al_client_senza_navigare_l_editor() {
            use crate::lsp_mode::LspEvent;
            use lsp_types::{GotoDefinitionResponse, Location, Position, Range};

            let (mut app, root, mcp_epoch, lsp_epoch) = lsp_app_con_mcp("mcp_lsp_routing");
            let (mailbox_lsp, mut ricevitore_lsp) = lume_lsp::LspCommandMailbox::channel();
            let lsp = app.lsp.get_mut("rust-analyzer").unwrap();
            lsp.mailbox = Some(mailbox_lsp);
            lsp.availability.definition = true;
            app.open_file(&root.join("main.rs")).unwrap();
            let lume_lsp::LspCommand::Open { .. } = comando_lsp(&mut ricevitore_lsp) else {
                panic!("atteso didOpen")
            };

            let mut risposte = con_mailbox(&mut app, mcp_epoch);
            while risposte.try_recv().is_ok() {}
            let query = AppQuery::Definition {
                relative: PathBuf::from("main.rs"),
                line: 1,
                column: 4,
            };
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 71,
                    conn: 1,
                    query,
                },
            ));
            let lume_lsp::LspCommand::Ask { id, request } = comando_lsp(&mut ricevitore_lsp) else {
                panic!("atteso textDocument/definition")
            };
            assert!(matches!(request, lume_lsp::LspRequest::Definition { .. }));
            let uri = crate::lsp_uri::path_to_uri(&root.join("altro.rs")).unwrap();
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    id,
                    Box::new(Ok(lume_lsp::LspAnswer::Definition(Some(
                        GotoDefinitionResponse::Scalar(Location {
                            uri,
                            range: Range::new(Position::new(0, 0), Position::new(0, 5)),
                        }),
                    )))),
                )),
            ));

            let result = risposte.try_recv().expect("risposta MCP");
            let McpCommand::Answer {
                id: answer_id,
                answer,
            } = result
            else {
                panic!("attesa risposta, ricevuto {result:?}")
            };
            assert_eq!(answer_id, 71);
            assert_eq!(json(answer)["definitions"][0]["path"], "altro.rs");
            assert_eq!(app.active_path(), Some(root.join("main.rs").as_path()));
            assert_eq!(app.workspace.len(), 1, "la definizione MCP non apre schede");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn rinomina_mcp_valida_prima_di_lsp_e_attende_consenso_per_applicare() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_two_phase");
            assert!(matches!(
                lsp_commands.try_recv(),
                Ok(lume_lsp::LspCommand::Open { .. })
            ));
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 91,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "nome non valido".into(),
                    },
                },
            ));
            assert!(
                lsp_commands.try_recv().is_err(),
                "il nome invalido non raggiunge LSP"
            );
            let invalid = mcp_commands.try_recv().expect("errore immediato");
            assert!(matches!(
                invalid,
                McpCommand::Answer {
                    id: 91,
                    answer: Err(_)
                }
            ));

            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 92,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask {
                id: request,
                request: lume_lsp::LspRequest::Rename { .. },
            } = lsp_commands.try_recv().expect("richiesta rename a LSP")
            else {
                panic!("attesa richiesta LSP rename")
            };
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            assert!(mcp_commands.try_recv().is_err(), "attende consenso utente");
            assert_eq!(
                app.workspace
                    .get_by_path(&root.join("main.rs"))
                    .and_then(|doc| app.workspace.get(doc))
                    .unwrap()
                    .rope()
                    .to_string(),
                "fn mian() {}\n"
            );
            assert_eq!(
                app.mcp_card_view().unwrap().title,
                "vuole rinominare in «main»: 1 occorrenza in 1 file"
            );
            let _ = app.update(Message::McpCard(CardMessage::Allow));
            let McpCommand::Answer {
                id: 92,
                answer: Ok(AppAnswer::Json(result)),
            } = mcp_commands.try_recv().expect("risposta dopo il consenso")
            else {
                panic!("risposta MCP inattesa")
            };
            assert_eq!(result["saved"], false);
            assert_eq!(result["files"][0]["path"], "main.rs");
            assert_eq!(
                app.workspace
                    .get_by_path(&root.join("main.rs"))
                    .and_then(|doc| app.workspace.get(doc))
                    .unwrap()
                    .rope()
                    .to_string(),
                "fn main() {}\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn rinomina_mcp_fuori_radice_e_abbandono_non_modificano_buffer() {
            use crate::lsp_mode::LspEvent;

            for mode in lume_mcp::Mode::ALL {
                let nome = format!("mcp_rename_outside_{}", mode.as_str());
                let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                    app_con_mcp_e_rinomina(&nome);
                let _ = lsp_commands.try_recv().expect("didOpen");
                let outside = tempdir(&format!("mcp_rename_outside_target_{}", mode.as_str()));
                let outside_file = outside.join("other.rs");
                std::fs::write(&outside_file, "fn mian() {}\n").unwrap();
                let _ = app.update(Message::McpSetMode(mode));
                let _ = app.update(Message::Mcp(
                    mcp_epoch,
                    McpEvent::Ask {
                        id: 100,
                        conn: 1,
                        query: AppQuery::Rename {
                            relative: PathBuf::from("main.rs"),
                            line: 1,
                            column: 4,
                            new_name: "main".into(),
                        },
                    },
                ));
                let lume_lsp::LspCommand::Ask { id: request, .. } =
                    lsp_commands.try_recv().expect("richiesta LSP")
                else {
                    panic!("attesa richiesta LSP")
                };
                let edit = lsp_types::WorkspaceEdit {
                    changes: Some(
                        [root.join("main.rs"), outside_file.clone()]
                            .into_iter()
                            .map(|path| {
                                (
                                    crate::lsp_uri::path_to_uri(&path).unwrap(),
                                    vec![lsp_types::TextEdit {
                                        range: lsp_types::Range::new(
                                            lsp_types::Position::new(0, 3),
                                            lsp_types::Position::new(0, 7),
                                        ),
                                        new_text: "main".into(),
                                    }],
                                )
                            })
                            .collect(),
                    ),
                    ..lsp_types::WorkspaceEdit::default()
                };
                let _ = app.update(Message::Lsp(
                    "rust-analyzer".into(),
                    lsp_epoch,
                    LspEvent::From(lume_lsp::ActorEvent::Answer(
                        request,
                        Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                    )),
                ));
                let McpCommand::Answer {
                    id: 100,
                    answer: Err(error),
                } = mcp_commands.try_recv().expect("rifiuto immediato")
                else {
                    panic!("la proposta fuori radice va rifiutata")
                };
                assert!(error.contains("fuori dalla radice"), "{error}");
                assert!(
                    !error.contains(&*outside.to_string_lossy()),
                    "non rivela path assoluto"
                );
                let open_doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
                assert_eq!(
                    app.workspace.get(open_doc).unwrap().rope().to_string(),
                    "fn mian() {}\n"
                );
                assert!(app.workspace.get_by_path(&outside_file).is_none());
                assert!(app.mcp_card_view().is_none());
                assert_eq!(
                    std::fs::read_to_string(outside_file).unwrap(),
                    "fn mian() {}\n"
                );
                let _ = std::fs::remove_dir_all(&root);
                let _ = std::fs::remove_dir_all(&outside);
            }

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_abandoned");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 101,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let _ = app.update(Message::Mcp(mcp_epoch, McpEvent::AskGone(101)));
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            assert!(
                mcp_commands.try_recv().is_err(),
                "il client ha abbandonato la richiesta"
            );
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn mian() {}\n"
            );
            assert!(app.mcp_card_view().is_none());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn rinomina_mcp_sotto_git_e_testo_stale_rifiuta_tutto() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_git_reject");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = std::fs::create_dir(root.join(".git"));
            let git_config = root.join(".git").join("config");
            std::fs::write(&git_config, "[core]\n").unwrap();
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 110,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [root.join("main.rs"), git_config.clone()]
                        .into_iter()
                        .map(|path| {
                            (
                                crate::lsp_uri::path_to_uri(&path).unwrap(),
                                vec![lsp_types::TextEdit {
                                    range: lsp_types::Range::new(
                                        lsp_types::Position::new(0, 0),
                                        lsp_types::Position::new(0, 2),
                                    ),
                                    new_text: "ok".into(),
                                }],
                            )
                        })
                        .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            let McpCommand::Answer {
                id: 110,
                answer: Err(error),
            } = mcp_commands
                .try_recv()
                .expect(".git si rifiuta prima di applicare")
            else {
                panic!("la proposta che tocca .git va rifiutata")
            };
            assert!(error.contains(".git"), "{error}");
            let main = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(main).unwrap().rope().to_string(),
                "fn mian() {}\n"
            );
            assert_eq!(std::fs::read_to_string(&git_config).unwrap(), "[core]\n");
            assert!(app.workspace.get_by_path(&git_config).is_none());
            assert!(app.mcp_card_view().is_none());
            let _ = std::fs::remove_dir_all(&root);

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_stale_card");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 111,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            app.workspace
                .get_by_path(&root.join("main.rs"))
                .and_then(|id| app.workspace.get_mut(id))
                .unwrap()
                .reload_from_str("// mio testo\nfn mian() {}\n");
            let _ = app.update(Message::McpCard(CardMessage::Allow));
            let McpCommand::Answer {
                id: 111,
                answer: Err(error),
            } = mcp_commands
                .try_recv()
                .expect("il piano stale risponde con errore")
            else {
                panic!("la rinomina stale non deve applicarsi")
            };
            assert!(error.contains("cambiato"), "{error}");
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "// mio testo\nfn mian() {}\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn rinomine_mcp_in_coda_non_applicano_un_piano_diventato_stale() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_fifo");
            let _ = lsp_commands.try_recv().expect("didOpen");
            for (id, new_name) in [(120, "main"), (121, "principale")] {
                let _ = app.update(Message::Mcp(
                    mcp_epoch,
                    McpEvent::Ask {
                        id,
                        conn: 1,
                        query: AppQuery::Rename {
                            relative: PathBuf::from("main.rs"),
                            line: 1,
                            column: 4,
                            new_name: new_name.into(),
                        },
                    },
                ));
            }
            let request_one = match lsp_commands.try_recv().expect("prima richiesta LSP") {
                lume_lsp::LspCommand::Ask { id, .. } => id,
                other => panic!("attesa Ask, ricevuto {other:?}"),
            };
            let request_two = match lsp_commands.try_recv().expect("seconda richiesta LSP") {
                lume_lsp::LspCommand::Ask { id, .. } => id,
                other => panic!("attesa Ask, ricevuto {other:?}"),
            };
            for (request, name) in [(request_one, "main"), (request_two, "principale")] {
                let edit = lsp_types::WorkspaceEdit {
                    changes: Some(
                        [(
                            crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                            vec![lsp_types::TextEdit {
                                range: lsp_types::Range::new(
                                    lsp_types::Position::new(0, 3),
                                    lsp_types::Position::new(0, 7),
                                ),
                                new_text: name.into(),
                            }],
                        )]
                        .into_iter()
                        .collect(),
                    ),
                    ..lsp_types::WorkspaceEdit::default()
                };
                let _ = app.update(Message::Lsp(
                    "rust-analyzer".into(),
                    lsp_epoch,
                    LspEvent::From(lume_lsp::ActorEvent::Answer(
                        request,
                        Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                    )),
                ));
            }
            assert_eq!(app.mcp_card_view().unwrap().position, (1, 2));
            assert!(mcp_commands.try_recv().is_err());
            let _ = app.update(Message::McpCard(CardMessage::Allow));
            assert!(matches!(
                mcp_commands.try_recv(),
                Ok(McpCommand::Answer {
                    id: 120,
                    answer: Ok(AppAnswer::Json(_))
                })
            ));
            assert_eq!(app.mcp_card_view().unwrap().position, (1, 1));
            let _ = app.update(Message::McpCard(CardMessage::Allow));
            let McpCommand::Answer {
                id: 121,
                answer: Err(error),
            } = mcp_commands
                .try_recv()
                .expect("la seconda proposta è stale")
            else {
                panic!("la seconda rinomina non deve sovrascrivere la prima")
            };
            assert!(error.contains("cambiato"), "{error}");
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn main() {}\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn cambio_a_brave_mentre_lsp_risponde_applica_senza_card() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_mode_inflight");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 130,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let _ = app.update(Message::Statusbar(StatusbarMessage::McpMode(2)));
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            assert!(matches!(
                mcp_commands.try_recv(),
                Ok(McpCommand::Answer {
                    id: 130,
                    answer: Ok(AppAnswer::Json(_))
                })
            ));
            assert!(app.mcp_card_view().is_none());
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn main() {}\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn ferma_mcp_durante_rename_rifiuta_e_scartala_risposta_tardiva() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_stop_mcp");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 139,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };

            let _ = app.apply_feature_command(FeatureCommand::Stop(Feature::Mcp));
            assert!(app.mcp.is_none());
            assert!(matches!(
                mcp_commands.try_recv(),
                Ok(McpCommand::Answer {
                    id: 139,
                    answer: Err(_)
                })
            ));

            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));

            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn mian() {}\n"
            );
            assert!(app.rename.is_none(), "non deve aprire la rinomina UI");
            assert!(mcp_commands.try_recv().is_err());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn rinomina_mcp_clientgone_errore_lsp_e_stop_lsp_non_lasciano_modifiche() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_clientgone");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 140,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let _ = app.update(Message::Mcp(mcp_epoch, McpEvent::ClientGone(1)));
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            assert!(
                mcp_commands.try_recv().is_err(),
                "il client disconnesso non riceve reply"
            );
            assert!(app.mcp_card_view().is_none());
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn mian() {}\n"
            );
            assert!(app.mcp.as_ref().unwrap().renames.is_empty());
            let _ = std::fs::remove_dir_all(&root);

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_lsp_error");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 141,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Err("server failure".into())),
                )),
            ));
            let McpCommand::Answer {
                id: 141,
                answer: Err(error),
            } = mcp_commands.try_recv().expect("errore LSP verso MCP")
            else {
                panic!("atteso errore MCP")
            };
            assert!(error.contains("language server") && error.contains("server failure"));
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn mian() {}\n"
            );
            assert!(app.mcp.as_ref().unwrap().renames.is_empty());
            let _ = std::fs::remove_dir_all(&root);

            let (mut app, root, mcp_epoch, _lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_lsp_stopped");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 142,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let _ = lsp_commands.try_recv().expect("richiesta LSP");
            let _ = app.lsp_forget("rust-analyzer", crate::lsp_mode::LspState::Ferma);
            let McpCommand::Answer {
                id: 142,
                answer: Err(error),
            } = mcp_commands.try_recv().expect("stop LSP verso MCP")
            else {
                panic!("atteso errore MCP al fermarsi di LSP")
            };
            assert!(error.contains("si è fermato"), "{error}");
            assert!(app.mcp.as_ref().unwrap().renames.is_empty());
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn mian() {}\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn rinomina_mcp_in_coda_applica_quando_la_modalita_diventa_auto() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_queued_auto");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 170,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            assert_eq!(app.mcp_pending(), 1);
            assert!(mcp_commands.try_recv().is_err());
            let _ = app.update(Message::Statusbar(StatusbarMessage::McpMode(1)));
            assert_eq!(app.mcp_mode(), Some(lume_mcp::Mode::NonBrave));
            assert!(
                app.notice
                    .as_ref()
                    .is_some_and(|(_, n)| n.text.contains("Rispondi prima"))
            );
            // La riconsiderazione della coda resta verificata sul percorso interno.
            let _ = app.mcp_apply_mode(lume_mcp::Mode::Auto);
            app.mcp_sync_card();
            assert_eq!(app.mcp_mode(), Some(lume_mcp::Mode::Auto));
            let McpCommand::Answer {
                id: 170,
                answer: Ok(AppAnswer::Json(result)),
            } = mcp_commands.try_recv().expect("la coda segue Auto")
            else {
                panic!("attesa risposta della rinomina")
            };
            assert_eq!(result["saved"], false);
            assert_eq!(app.mcp_pending(), 0);
            assert!(app.mcp_card_view().is_none());
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn main() {}\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn askgone_durante_la_card_rimuove_piano_e_una_conferma_tardiva_non_applica() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_gone_card");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 180,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            assert!(app.mcp_card_view().is_some());
            let _ = app.update(Message::Mcp(mcp_epoch, McpEvent::AskGone(180)));
            assert!(app.mcp_card_view().is_none());
            let _ = app.update(Message::McpCard(CardMessage::Allow));
            assert!(mcp_commands.try_recv().is_err());
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn mian() {}\n"
            );
            assert!(app.mcp.as_ref().unwrap().gate.is_empty());
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn diniego_della_card_rename_non_modifica_il_buffer() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_denied");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 190,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            assert!(app.mcp_card_view().is_some());
            let _ = app.update(Message::McpCard(CardMessage::Deny));
            let McpCommand::Answer {
                id: 190,
                answer: Err(error),
            } = mcp_commands
                .try_recv()
                .expect("il diniego viene riferito all'agente")
            else {
                panic!("atteso rifiuto MCP")
            };
            assert!(error.contains("negat"), "{error}");
            assert!(app.mcp_card_view().is_none());
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                "fn mian() {}\n"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn risposta_rename_mcp_orfana_non_chiude_il_riquadro_dell_utente() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, _mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_orphan_ui");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 150,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } = lsp_commands
                .try_recv()
                .expect("richiesta rename silenziosa")
            else {
                panic!("attesa richiesta LSP")
            };
            let doc = app.workspace.get_by_path(&root.join("main.rs")).unwrap();
            let _ = app.open_rename_box(
                doc,
                lsp_types::Position::new(0, 3),
                app.workspace.get(doc).unwrap().version(),
                "mian".into(),
            );
            app.mcp = None;
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(
                        crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, 3),
                                lsp_types::Position::new(0, 7),
                            ),
                            new_text: "main".into(),
                        }],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            assert!(
                app.rename.is_some(),
                "la risposta MCP non chiude la UI rename"
            );
            let doc = app.workspace.get(doc).unwrap();
            assert_eq!(doc.rope().to_string(), "fn mian() {}\n");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn rinomina_mcp_apre_file_chiuso_sporco_senza_salvare_o_rubare_focus() {
            use crate::lsp_mode::LspEvent;

            let (mut app, root, mcp_epoch, lsp_epoch, mut lsp_commands, mut mcp_commands) =
                app_con_mcp_e_rinomina("mcp_rename_closed_file");
            let _ = lsp_commands.try_recv().expect("didOpen");
            let closed = root.join("closed.rs");
            std::fs::write(&closed, "mian\n").unwrap();
            let active = app.active_doc();
            let _ = app.update(Message::McpSetMode(lume_mcp::Mode::Brave));
            let _ = app.update(Message::Mcp(
                mcp_epoch,
                McpEvent::Ask {
                    id: 160,
                    conn: 1,
                    query: AppQuery::Rename {
                        relative: PathBuf::from("main.rs"),
                        line: 1,
                        column: 4,
                        new_name: "main".into(),
                    },
                },
            ));
            let lume_lsp::LspCommand::Ask { id: request, .. } =
                lsp_commands.try_recv().expect("richiesta LSP")
            else {
                panic!("attesa richiesta LSP")
            };
            let edit = lsp_types::WorkspaceEdit {
                changes: Some(
                    [(root.join("main.rs"), 3, 7), (closed.clone(), 0, 4)]
                        .into_iter()
                        .map(|(path, start, end)| {
                            (
                                crate::lsp_uri::path_to_uri(&path).unwrap(),
                                vec![lsp_types::TextEdit {
                                    range: lsp_types::Range::new(
                                        lsp_types::Position::new(0, start),
                                        lsp_types::Position::new(0, end),
                                    ),
                                    new_text: "main".into(),
                                }],
                            )
                        })
                        .collect(),
                ),
                ..lsp_types::WorkspaceEdit::default()
            };
            let _ = app.update(Message::Lsp(
                "rust-analyzer".into(),
                lsp_epoch,
                LspEvent::From(lume_lsp::ActorEvent::Answer(
                    request,
                    Box::new(Ok(lume_lsp::LspAnswer::Rename(Some(edit)))),
                )),
            ));
            let McpCommand::Answer {
                id: 160,
                answer: Ok(AppAnswer::Json(result)),
            } = mcp_commands.try_recv().expect("risposta al client")
            else {
                panic!("atteso esito JSON")
            };
            assert_eq!(result["saved"], false);
            assert_eq!(result["occurrences"], 2);
            let closed_doc = app
                .workspace
                .get_by_path(&closed)
                .expect("file aperto in scheda");
            let document = app.workspace.get(closed_doc).unwrap();
            assert_eq!(document.rope().to_string(), "main\n");
            assert!(document.is_dirty());
            assert_eq!(std::fs::read_to_string(&closed).unwrap(), "mian\n");
            assert_eq!(
                app.active_doc(),
                active,
                "l'apertura silenziosa non cambia focus"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn la_riga_server_mcp_mostra_i_tasti_copia_solo_a_server_vivo() {
            let (mut app, root) = app_con_cartella("mcp_riga");
            let riga = |app: &Lume| {
                app.extension_rows()
                    .into_iter()
                    .find(|riga| riga.feature == Feature::Mcp)
                    .expect("la riga MCP è cablata")
            };
            let ferma = riga(&app);
            assert!(ferma.enabled);
            assert!(ferma.can_start);
            assert_eq!(ferma.detail_override.as_deref(), Some("non avviato"));
            assert!(ferma.copies.is_empty());
            assert!(app.mcp_status_text().is_none());
            let _ = app.mcp_accendi(lume_mcp::confine::canonical(&root).unwrap());
            let epoch = app.mcp.as_ref().unwrap().epoch;
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = app.update(Message::Mcp(epoch, McpEvent::Ready(tx)));
            let viva = riga(&app);
            let trasporto = app.mcp.as_ref().unwrap().endpoint.describe();
            assert_eq!(viva.live, Some(1));
            assert_eq!(
                viva.detail_override.as_deref(),
                Some(format!("{trasporto} · in ascolto · nessun client").as_str())
            );
            #[cfg(unix)]
            let etichette = [
                "Copia per Claude Code",
                "Copia per Codex",
                "Copia JSON (Cursor, Claude Desktop)",
                "Copia ID istanza",
            ];
            #[cfg(windows)]
            let etichette = [
                "Copia per Claude Code",
                "Copia per Codex",
                "Copia JSON (Cursor, Claude Desktop)",
            ];
            assert_eq!(
                viva.copies
                    .iter()
                    .map(|(label, _)| *label)
                    .collect::<Vec<_>>(),
                etichette
            );
            #[cfg(unix)]
            {
                let atteso = app.mcp.as_ref().unwrap().endpoint.instance().unwrap();
                assert_eq!(
                    viva.copies.last().map(|(_, testo)| testo.as_str()),
                    Some(atteso.as_str()),
                    "l'ID copiato è quello del trasporto vivo"
                );
            }
            assert_eq!(app.mcp_status_text().as_deref(), Some("MCP · in ascolto"));
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn la_spia_porta_a_estensioni_e_non_lo_richiude() {
            let mut app = app();
            assert!(!app.registry.is_open(tools::ext::ID));
            let _ = app.update(Message::Statusbar(StatusbarMessage::ShowExtensions));
            assert!(app.registry.is_open(tools::ext::ID));
            let _ = app.update(Message::Statusbar(StatusbarMessage::ShowExtensions));
            assert!(app.registry.is_open(tools::ext::ID));
        }
    }
}

#[cfg(test)]
mod reader_lifecycle_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use futures::StreamExt;

    #[test]
    fn duplicate_open_reader_preserves_accepted_bytes_and_external_conflict() {
        for dirty in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("duplicate.txt");
            std::fs::write(&path, "accettato\r\n").unwrap();
            let (mut app, _) = Lume::new_without_store();
            app.open_file(&path).unwrap();
            let id = app.workspace.get_by_path(&path).unwrap();
            let len = app.workspace.get(id).unwrap().rope().len_chars();
            if dirty {
                app.workspace
                    .get_mut(id)
                    .unwrap()
                    .apply(lume_core::Transaction {
                        changes: lume_core::ChangeSet::replace_many(
                            &[(0, 0, "locale ".into())],
                            len,
                        )
                        .unwrap(),
                        selection_after: None,
                    });
            }
            let accepted_payload = app.workspace.get(id).unwrap().save_payload();
            let version = app.workspace.get(id).unwrap().version();
            let accepted = app.disk_fingerprints[&id];
            app.external_conflicts.insert(id);
            std::fs::write(&path, "modifica esterna\r\n").unwrap();
            let mut stream = iced_runtime::task::into_stream(
                app.open_file_task(path.clone(), crate::file_read::DopoApertura::Niente),
            )
            .unwrap();
            let message = match futures::executor::block_on(stream.next()).unwrap() {
                iced_runtime::Action::Output(message) => message,
                _ => panic!("attesa risposta reader reale"),
            };
            drop(app.update(message));
            assert_eq!(
                app.workspace.get(id).unwrap().save_payload(),
                accepted_payload
            );
            assert_eq!(app.workspace.get(id).unwrap().version(), version);
            assert_eq!(app.workspace.get(id).unwrap().is_dirty(), dirty);
            assert_eq!(app.disk_fingerprints[&id], accepted);
            assert!(app.external_conflicts.contains(&id));
            assert_eq!(std::fs::read(&path).unwrap(), b"modifica esterna\r\n");
        }
    }

    #[test]
    fn watcher_unchanged_echo_does_not_prepare_rope_or_inverse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("echo.txt");
        let raw = "\u{feff}λ\r\n🙂\r\n";
        std::fs::write(&path, raw).unwrap();
        let (mut app, _) = Lume::new_without_store();
        let id = app.workspace.open(Some(path.clone()), raw);
        app.disk_fingerprints.insert(
            id,
            crate::file_read::Testo::from(raw.to_owned()).fingerprint(),
        );
        let version = app.workspace.get(id).unwrap().version();
        let task = app.reload_doc(id, path);
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let message = match runtime.block_on(stream.next()).unwrap() {
            iced_runtime::Action::Output(message) => message,
            _ => panic!("atteso output reader"),
        };
        assert!(
            !message_has_prepared_content(&message),
            "eco identico non deve preparare Rope o inverso"
        );
        drop(app.update(message));
        assert_eq!(app.workspace.get(id).unwrap().version(), version);
        assert_eq!(
            app.workspace.get(id).unwrap().save_payload(),
            raw.as_bytes()
        );
    }

    #[test]
    fn watcher_unchanged_classification_rechecks_changed_accepted_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("echo-race.txt");
        std::fs::write(&path, "disco\r\n").unwrap();
        let (mut app, _) = Lume::new_without_store();
        let id = app.workspace.open(Some(path.clone()), "buffer\n");
        app.disk_fingerprints.insert(
            id,
            crate::file_read::Testo::from("disco\r\n".to_owned()).fingerprint(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut stream = iced_runtime::task::into_stream(app.reload_doc(id, path.clone())).unwrap();
        let message = match runtime.block_on(stream.next()).unwrap() {
            iced_runtime::Action::Output(message) => message,
            _ => panic!("atteso output reader"),
        };
        assert!(!message_has_prepared_content(&message));
        app.disk_fingerprints.insert(
            id,
            crate::file_read::Testo::from("altri byte".to_owned()).fingerprint(),
        );
        let mut recheck =
            iced_runtime::task::into_stream(app.update(message)).expect("serve nuova verifica");
        assert_eq!(
            app.workspace.get(id).unwrap().rope().to_string(),
            "buffer\n"
        );
        let result = match runtime.block_on(recheck.next()).unwrap() {
            iced_runtime::Action::Output(message) => message,
            _ => panic!("atteso output reader"),
        };
        assert!(message_has_prepared_content(&result));
        drop(app.update(result));
        assert_eq!(app.workspace.get(id).unwrap().save_payload(), b"disco\r\n");
    }

    #[test]
    fn five_reader_tasks_deliver_prepared_content_and_preserve_format() {
        for kind in 0..5 {
            #[cfg(not(feature = "mcp"))]
            if kind >= 3 {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            // MCP apre la radice canonica: la fixture fa lo stesso, così il
            // documento resta lo stesso file in entrambe le forme.
            let base = dir.path().canonicalize().unwrap();
            let path = base.join("held-reader.txt");
            let raw = "\u{feff}λ\r\n🙂\r\n";
            std::fs::write(&path, raw).unwrap();
            let (mut app, _) = Lume::new_without_store();
            let task = held_task(&mut app, path.clone(), kind);
            let before = app.workspace.get_by_path(&path).and_then(|id| {
                app.workspace
                    .get(id)
                    .map(|doc| (id, doc.rope().to_string(), doc.read_snapshot()))
            });
            let mut stream = iced_runtime::task::into_stream(task).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let message = match runtime.block_on(stream.next()).unwrap() {
                iced_runtime::Action::Output(message) => message,
                _ => panic!("atteso output reader"),
            };
            let prepared = message_has_prepared_content(&message);
            assert!(
                prepared,
                "reader {kind} deve preparare prima della consegna UI"
            );
            let duplicate = message.clone();
            drop(app.update(message));
            let id = app.workspace.get_by_path(&path).unwrap();
            let doc = app.workspace.get(id).unwrap();
            assert_eq!(doc.rope().to_string(), "λ\n🙂\n");
            assert_eq!(doc.save_payload(), raw.as_bytes());
            assert!(!doc.is_dirty());
            let version = doc.version();
            let expected = crate::file_read::Testo::from(raw.to_owned()).fingerprint();
            assert_eq!(app.disk_fingerprints.get(&id), Some(&expected));
            drop(app.update(duplicate));
            assert_eq!(app.workspace.get(id).unwrap().version(), version);
            if let Some((original_id, text, snapshot)) = before {
                assert_eq!(id, original_id);
                let doc = app.workspace.get_mut(id).unwrap();
                // Il reload ha già accettato una versione nuova: una receipt
                // save della fotografia precedente deve essere rifiutata.
                assert!(!doc.can_mark_snapshot_saved(&snapshot));
                assert!(doc.undo().is_some());
                assert_eq!(doc.rope().to_string(), text);
                assert!(doc.is_dirty());
                assert!(doc.redo().is_some());
                assert_eq!(doc.save_payload(), raw.as_bytes());
                assert!(!doc.is_dirty());
            }
        }
    }

    fn message_has_prepared_content(message: &Message) -> bool {
        match message {
            Message::FileReadFinished(_, _, reading) => {
                reading.esito.as_ref().unwrap().has_prepared()
            }
            Message::RestoredTabReadFinished(_, _, _, _, _, outcome) => outcome
                .result
                .as_ref()
                .unwrap()
                .0
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .text
                .has_prepared(),
            Message::DocRicaricato(_, _, _, result) => {
                result.as_ref().unwrap().testo.has_prepared()
            }
            #[cfg(feature = "mcp")]
            Message::McpOpenFinished { result, .. } => result.as_ref().unwrap().has_prepared(),
            #[cfg(feature = "mcp")]
            Message::McpDocReloaded(_, _, _, result) => result.as_ref().unwrap().has_prepared(),
            _ => panic!("messaggio reader inatteso"),
        }
    }

    #[test]
    fn prepared_reload_results_remain_unconsumed_after_stale_guards() {
        for kind in [1, 2, 4] {
            #[cfg(not(feature = "mcp"))]
            if kind == 4 {
                continue;
            }
            for reason in 0..4 {
                if reason == 3 && kind != 1 {
                    continue;
                }
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("held-reader.txt");
                std::fs::write(&path, "\u{feff}nuovo\r\n").unwrap();
                let (mut app, _) = Lume::new_without_store();
                let task = held_task(&mut app, path.clone(), kind);
                let id = app.workspace.get_by_path(&path).unwrap();
                let mut stream = iced_runtime::task::into_stream(task).unwrap();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let message = match runtime.block_on(stream.next()).unwrap() {
                    iced_runtime::Action::Output(message) => message,
                    _ => panic!("atteso output reader"),
                };
                assert!(message_has_prepared_content(&message));
                match reason {
                    0 => {
                        let doc = app.workspace.get_mut(id).unwrap();
                        let len = doc.rope().len_chars();
                        doc.apply(lume_core::Transaction::with_selection(
                            lume_core::ChangeSet::insert_at(0, "edit", len),
                            Range::point(4),
                        ));
                    }
                    1 => {
                        app.workspace.close(id);
                    }
                    2 => app
                        .workspace
                        .rebind_path(id, dir.path().join("successore.txt"))
                        .unwrap(),
                    _ => {
                        assert_eq!(
                            app.scritture
                                .ammetti(crate::scritture::Op::Rinomina { docs: vec![] }),
                            Ok(crate::scritture::Permesso::Subito)
                        );
                    }
                }
                let before = app
                    .workspace
                    .get(id)
                    .map(|doc| (doc.save_payload(), doc.version(), doc.is_dirty()));
                let clone = message.clone();
                drop(app.update(message));
                assert_eq!(
                    before,
                    app.workspace.get(id).map(|doc| (
                        doc.save_payload(),
                        doc.version(),
                        doc.is_dirty()
                    ))
                );
                assert!(message_has_prepared_content(&clone));
                assert!(!app.disk_fingerprints.contains_key(&id));
            }
        }
    }

    #[test]
    fn repeated_exit_preserves_global_reader_admission_limit() {
        use futures::FutureExt;
        let (mut app, _) = Lume::new_without_store();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (started, ready) = std::sync::mpsc::channel();
            let (release, wait) = std::sync::mpsc::channel();
            assert!(
                app.readers
                    .read(move || {
                        started.send(()).unwrap();
                        wait.recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                    })
                    .now_or_never()
                    .is_none()
            );
            ready
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            releases.push(release);
        }
        drop(app.esci(None));
        for _ in 0..8 {
            let mut revoked = app.readers.read(|| 42);
            assert!(revoked.as_mut().now_or_never().is_none());
            let extra_worker = app.readers.has_workers();
            drop(app.esci(None));
            assert!(futures::executor::block_on(revoked).is_err());
            if extra_worker {
                for release in releases {
                    release.send(()).unwrap();
                }
                panic!("generazioni ripetute non devono ammettere altri reader");
            }
            assert_eq!(app.reader_collections.len(), 1);
        }
        let mut next = app.readers.read(|| 42);
        let pending = next.as_mut().now_or_never();
        let extra_worker = app.readers.has_workers();
        // Libera sempre i worker trattenuti prima dell'asserzione RED.
        for release in releases {
            release.send(()).unwrap();
        }
        assert!(pending.is_none());
        assert!(
            !extra_worker,
            "nuova generazione non deve superare due reader globali"
        );
        assert_eq!(futures::executor::block_on(next), Ok(42));
    }

    #[test]
    fn started_open_drop_runtime_loss_exit_still_collects() {
        held_read_collects_after_runtime_loss(0);
    }

    #[test]
    fn started_restore_drop_runtime_loss_exit_still_collects() {
        held_read_collects_after_runtime_loss(1);
    }

    #[test]
    fn started_watcher_reload_drop_runtime_loss_exit_still_collects() {
        held_read_collects_after_runtime_loss(2);
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn started_mcp_open_drop_runtime_loss_exit_still_collects() {
        held_read_collects_after_runtime_loss(3);
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn started_mcp_reload_drop_runtime_loss_exit_still_collects() {
        held_read_collects_after_runtime_loss(4);
    }

    fn held_task(app: &mut Lume, path: PathBuf, kind: u8) -> Task<Message> {
        #[cfg(feature = "mcp")]
        let root = path.parent().unwrap().to_path_buf();
        match kind {
            0 => app.open_file_task(path, crate::file_read::DopoApertura::Niente),
            1 => {
                let id = app.workspace.open(Some(path.clone()), "");
                app.groups.open(app.groups.focus(), id);
                app.pending_restored_tabs.insert(id, path);
                app.restored_tab_read_task(id, false)
            }
            2 => {
                let id = app
                    .workspace
                    .open(Some(path.clone()), "contenuto precedente\n");
                app.reload_doc(id, path)
            }
            #[cfg(feature = "mcp")]
            3 => {
                app.mcp = Some(
                    crate::mcp_mode::McpWorkspace::new(
                        1,
                        root,
                        Some(std::env::temp_dir()),
                        lume_mcp::Availability::default(),
                    )
                    .unwrap(),
                );
                app.mcp_open_in_editor(1, 1, PathBuf::from("held-reader.txt"), None)
            }
            #[cfg(feature = "mcp")]
            4 => {
                let id = app
                    .workspace
                    .open(Some(path.clone()), "contenuto precedente\n");
                app.mcp_after_write(&path, Some(id))
            }
            _ => unreachable!(),
        }
    }

    fn held_read_collects_after_runtime_loss(kind: u8) {
        held_read_collects_with_gate_runtime_loss(kind, false);
    }

    #[test]
    fn started_exit_task_and_runtime_loss_preserve_reader_collector() {
        for kind in 0..3 {
            held_read_collects_with_gate_runtime_loss(kind, true);
        }
        #[cfg(feature = "mcp")]
        for kind in 3..5 {
            held_read_collects_with_gate_runtime_loss(kind, true);
        }
    }

    fn held_read_collects_with_gate_runtime_loss(kind: u8, lose_gate_runtime: bool) {
        use futures::FutureExt;
        let dir = tempfile::tempdir().unwrap();
        // La radice viene canonica dal prodotto (MCP risolve la radice): la
        // fixture usa la stessa forma anche per la gate della lettura.
        let base = dir.path().canonicalize().unwrap();
        let path = base.join("held-reader.txt");
        std::fs::write(&path, "contenuto reale λ\n").unwrap();
        let (started, release) = crate::file_read::test_read_gate::install(&path);
        let (mut app, _) = Lume::new_without_store();
        let original = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let task = held_task(&mut app, path, kind);
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        original.block_on(async {
            let waiting = tokio::task::spawn_blocking(move || {
                started.recv_timeout(std::time::Duration::from_secs(5))
            });
            tokio::select! {
                result = waiting => { result.unwrap().unwrap(); }
                _ = stream.next() => panic!("reader deve restare trattenuto prima della consegna"),
            }
        });
        assert!(app.readers.has_workers());
        let collection_started = app.readers.collection_started_probe();
        drop(stream);
        drop(original);
        let task = app.esci(None);
        assert!(app.search_exit.as_ref().unwrap().readers.is_some());
        let mut exit = iced_runtime::task::into_stream(task).unwrap();
        let collecting = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        collecting.block_on(async {
            assert!(exit.next().now_or_never().is_none());
            if lose_gate_runtime {
                let started = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while !collection_started.load(std::sync::atomic::Ordering::Acquire) {
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                });
                tokio::select! {
                    result = started => result.unwrap(),
                    _ = exit.next() => panic!("uscita pronta prima di liberare il reader"),
                }
            }
        });
        assert!(
            app.search_exit
                .as_ref()
                .unwrap()
                .completion
                .result()
                .is_none()
        );
        // Nuovo owner: l'uscita differita non impedisce una nuova azione.
        assert_eq!(futures::executor::block_on(app.readers.read(|| 42)), Ok(42));
        let previous = app.search_exit.as_ref().unwrap().readers.clone().unwrap();
        let repeated = app.esci(None);
        drop(exit);
        let collecting = if lose_gate_runtime {
            // Il reader resta trattenuto: non aspettare il blocking collector
            // nello shutdown del vecchio runtime. Il nuovo gate deve ancora
            // raccogliere lo stesso lavoro, senza dichiarare uscita pronta.
            collecting.shutdown_timeout(std::time::Duration::ZERO);
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        } else {
            collecting
        };
        let mut exit = iced_runtime::task::into_stream(repeated).unwrap();
        collecting.block_on(async {
            assert!(exit.next().now_or_never().is_none());
        });
        assert!(previous.peek().is_none());
        drop(release);
        let action = collecting.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), exit.next())
                .await
                .unwrap()
                .unwrap()
        });
        assert!(matches!(
            action,
            iced_runtime::Action::Output(Message::SearchExitReady(_, true))
        ));
        assert!(
            app.search_exit
                .as_ref()
                .unwrap()
                .readers
                .as_ref()
                .unwrap()
                .peek()
                .is_some()
        );
        drop(exit);
        // Le raccolte già concluse vengono eliminate senza aspettare la UI.
        drop(app.esci(None));
        assert!(app.reader_collections.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn app_last_owner_drop_joins_actual_reader_without_runtime() {
        held_last_owner_collects(0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn app_last_owner_drop_joins_restored_reader_without_runtime() {
        held_last_owner_collects(1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn app_last_owner_drop_joins_watcher_reload_without_runtime() {
        held_last_owner_collects(2);
    }

    #[cfg(all(target_os = "linux", feature = "mcp"))]
    #[test]
    fn app_last_owner_drop_joins_mcp_open_without_runtime() {
        held_last_owner_collects(3);
    }

    #[cfg(all(target_os = "linux", feature = "mcp"))]
    #[test]
    fn app_last_owner_drop_joins_mcp_reload_without_runtime() {
        held_last_owner_collects(4);
    }

    #[cfg(target_os = "linux")]
    fn held_last_owner_collects(kind: u8) {
        held_last_owner_collects_with_exit(kind, false);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dropped_cold_exit_task_keeps_reader_collection_owned_by_app() {
        for kind in 0..3 {
            held_last_owner_collects_with_exit(kind, true);
        }
        #[cfg(feature = "mcp")]
        for kind in 3..5 {
            held_last_owner_collects_with_exit(kind, true);
        }
    }

    #[cfg(target_os = "linux")]
    fn held_last_owner_collects_with_exit(kind: u8, abandon_exit: bool) {
        held_last_owner_collects_with_gate(kind, u8::from(abandon_exit));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn app_drop_joins_reader_after_started_collector_runtime_loss() {
        for kind in 0..3 {
            held_last_owner_collects_with_gate(kind, 2);
            held_last_owner_collects_with_gate(kind, 3);
        }
        #[cfg(feature = "mcp")]
        for kind in 3..5 {
            held_last_owner_collects_with_gate(kind, 2);
            held_last_owner_collects_with_gate(kind, 3);
        }
    }

    #[cfg(target_os = "linux")]
    fn held_last_owner_collects_with_gate(kind: u8, exit_state: u8) {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let dir = tempfile::tempdir().unwrap();
        // Stessa forma canonica usata dal prodotto per la radice MCP.
        let base = dir.path().canonicalize().unwrap();
        let path = base.join("held-reader.txt");
        std::fs::write(&path, "lettura reale\n").unwrap();
        let (started, release) = crate::file_read::test_read_gate::install(&path);
        let (mut app, _) = Lume::new_without_store();
        let task = held_task(&mut app, path, kind);
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let tid = runtime.block_on(async {
            let waiting = tokio::task::spawn_blocking(move || {
                started.recv_timeout(std::time::Duration::from_secs(5))
            });
            tokio::select! {
                result = waiting => result.unwrap().unwrap(),
                _ = stream.next() => panic!("reader deve restare trattenuto"),
            }
        });
        let stat_path = format!("/proc/self/task/{tid}/stat");
        let identity = std::fs::read_to_string(&stat_path).unwrap();
        let start_time = identity
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap()
            .to_owned();
        drop(stream);
        drop(runtime);
        if exit_state == 1 {
            // La Task di uscita non è mai polled: il collector condiviso
            // conservato dall'app deve possedere ancora il reader avviato.
            drop(app.esci(None));
            assert!(app.search_exit.as_ref().unwrap().readers.is_some());
        } else if exit_state >= 2 {
            let probe = app.readers.collection_started_probe();
            let mut exit = iced_runtime::task::into_stream(app.esci(None)).unwrap();
            let collecting = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            collecting.block_on(async {
                let started = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while !probe.load(Ordering::Acquire) {
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                });
                tokio::select! {
                    result = started => result.unwrap(),
                    _ = exit.next() => panic!("collector deve attendere il reader"),
                }
            });
            drop(exit);
            collecting.shutdown_timeout(std::time::Duration::ZERO);
            if exit_state == 3 {
                // Invalidazione del gate: il fallback non dipende dal suo
                // future né dal relativo JoinHandle Tokio.
                app.search_exit = None;
            }
        }
        let released = Arc::new(AtomicBool::new(false));
        let marker = released.clone();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(25));
            marker.store(true, Ordering::Release);
            drop(release);
        });
        drop(app);
        let returned_after_release = released.load(Ordering::Acquire);
        releaser.join().unwrap();
        assert!(
            returned_after_release,
            "Drop app deve aspettare il reader trattenuto"
        );
        // Linux può esporre ancora il task in /proc dopo un join riuscito.
        // Attendere soltanto la scomparsa della stessa identità, senza
        // confondere un TID riutilizzato con il reader che abbiamo raccolto.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let final_stat = std::fs::read_to_string(&stat_path).ok();
            let same_thread = final_stat.as_ref().is_some_and(|stat| {
                stat.rsplit_once(')')
                    .and_then(|(_, fields)| fields.split_whitespace().nth(19))
                    == Some(start_time.as_str())
            });
            if !same_thread {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reader nativo deve scomparire dopo Drop app; kind={kind}, exit_state={exit_state}, stat={final_stat:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn dropped_restored_result_allows_retry_after_last_message_clone() {
        for missing in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("message-retry.txt");
            if !missing {
                std::fs::write(&path, "reale\n").unwrap();
            }
            let (mut app, _) = Lume::new_without_store();
            let id = app.workspace.open(Some(path.clone()), "");
            app.pending_restored_tabs.insert(id, path.clone());
            let task = app.restored_tab_read_task(id, false);
            let ticket = app.restored_tab_reads[&id];
            let mut stream = iced_runtime::task::into_stream(task).unwrap();
            let message = match futures::executor::block_on(stream.next()).unwrap() {
                iced_runtime::Action::Output(message) => message,
                _ => panic!("atteso messaggio restore reale"),
            };
            let clone = message.clone();
            drop(message);
            drop(stream);
            drop(app.restored_tab_read_task(id, false));
            assert_eq!(app.restored_tab_reads[&id], ticket);
            drop(clone);
            std::fs::write(&path, "retry reale\n").unwrap();
            let retry = app.restored_tab_read_task(id, false);
            assert!(
                app.restored_tab_reads[&id] > ticket,
                "ultimo messaggio perso deve consentire retry"
            );
            let mut retry = iced_runtime::task::into_stream(retry).unwrap();
            assert!(matches!(
                futures::executor::block_on(retry.next()),
                Some(iced_runtime::Action::Output(_))
            ));
            assert!(app.readers.collect());
        }
    }

    #[test]
    fn cold_restored_task_drop_allows_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retry.txt");
        std::fs::write(&path, "contenuto reale\n").unwrap();
        let (mut app, _) = Lume::new_without_store();
        let id = app.workspace.open(Some(path.clone()), "");
        app.pending_restored_tabs.insert(id, path);
        let first = app.restored_tab_read_task(id, false);
        let ticket = app.restored_tab_reads[&id];
        assert!(!app.readers.has_workers());
        drop(first);
        let retry = app.restored_tab_read_task(id, false);
        assert!(
            app.restored_tab_reads[&id] > ticket,
            "ticket abbandonato deve consentire retry"
        );
        let mut stream = iced_runtime::task::into_stream(retry).unwrap();
        assert!(!app.readers.has_workers());
        match futures::executor::block_on(stream.next()).unwrap() {
            iced_runtime::Action::Output(Message::RestoredTabReadFinished(
                found,
                _,
                request,
                _,
                _,
                result,
            )) => {
                assert_eq!(found, id);
                assert!(request > ticket);
                assert!(result.is_ok());
            }
            _ => panic!("atteso contenuto reale dal retry restore"),
        }
        assert!(!app.workspace.get(id).unwrap().is_dirty());
        assert!(app.readers.collect());
    }

    #[test]
    fn cold_open_task_drop_starts_no_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cold.txt");
        std::fs::write(&path, "contenuto").unwrap();
        let (mut app, _) = Lume::new_without_store();
        let task = app.open_file_task(path, crate::file_read::DopoApertura::Niente);
        assert!(!app.readers.has_workers());
        drop(task);
        assert!(!app.readers.has_workers());
    }

    #[test]
    fn actual_open_task_runs_without_original_tokio_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native.txt");
        std::fs::write(&path, "lettura reale λ\n").unwrap();
        let (mut app, _) = Lume::new_without_store();
        let task = app.open_file_task(path.clone(), crate::file_read::DopoApertura::Niente);
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let action = futures::executor::block_on(stream.next()).unwrap();
        match action {
            iced_runtime::Action::Output(Message::FileReadFinished(_, _, reading)) => {
                assert_eq!(reading.path, path);
                assert!(reading.esito.is_ok());
            }
            _ => panic!("attesa risposta reale di lettura"),
        }
        drop(stream);
        assert!(app.readers.has_workers());
        assert!(app.readers.collect());
        assert!(!app.readers.has_workers());
    }
}
