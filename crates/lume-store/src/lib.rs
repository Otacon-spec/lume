//! Persistenza locale di Lume: sessione e profili ambiente non segreti.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use directories::ProjectDirs;
use futures::channel::oneshot;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const LOCAL_ENVIRONMENT_ID: &str = "local";
/// Radice dati alternativa, pensata per sessioni di benchmark e smoke test
/// isolate. Se non è impostata, Lume continua a usare la directory utente
/// determinata dal sistema operativo.
pub const DATA_DIR_OVERRIDE_ENV: &str = "LUME_DATA_DIR";
const SCHEMA_VERSION: i64 = 5;
const NOTES_DIRECTORY_KEY: &str = "notes.directory";
const MCP_MODE_PREFIX: &str = "mcp.mode:";

pub fn mcp_mode_key(root: &Path) -> String {
    // Windows non distingue maiuscole e accetta entrambi i separatori; su
    // Unix il percorso è sensibile al caso e `/` è l'unico separatore. La
    // chiave deve seguire la piattaforma, o due progetti diversi potrebbero
    // condividere la stessa modalità.
    #[cfg(windows)]
    let text = root.to_string_lossy().to_lowercase().replace('/', "\\");
    #[cfg(not(windows))]
    let text = root.to_string_lossy().to_string();
    let trimmed = text.trim_end_matches(std::path::MAIN_SEPARATOR);
    // La radice `/` non ha un nome da trimmare: resta la radice.
    let suffix = if trimmed.is_empty() {
        text.as_str()
    } else {
        trimmed
    };
    format!("{MCP_MODE_PREFIX}{suffix}")
}

/// Directory dati comune a sessione, cache applicative e server gestiti.
/// L'override è letto una sola volta quando un consumer la richiede: non crea
/// polling né stato permanente nel prodotto.
pub fn application_data_dir() -> Result<PathBuf, StoreError> {
    application_data_dir_from(std::env::var_os(DATA_DIR_OVERRIDE_ENV))
}

/// Configurazione esterna (per esempio i temi icona) nella stessa radice
/// isolata, quando l'override è presente.
pub fn application_config_dir() -> Option<PathBuf> {
    std::env::var_os(DATA_DIR_OVERRIDE_ENV)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            ProjectDirs::from("io", "lume", "Lume").map(|dirs| dirs.config_dir().to_owned())
        })
}

/// Cartella Appunti predefinita, nella stessa radice di sessione e cache.
/// Non crea né migra file: una preferenza esplicita resta scelta del consumer.
pub fn default_notes_directory() -> Result<PathBuf, StoreError> {
    notes_directory_from(std::env::var_os(DATA_DIR_OVERRIDE_ENV))
}

fn notes_directory_from(override_path: Option<OsString>) -> Result<PathBuf, StoreError> {
    application_data_dir_from(override_path).map(|radice| radice.join("notes"))
}

fn application_data_dir_from(override_path: Option<OsString>) -> Result<PathBuf, StoreError> {
    override_path
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            ProjectDirs::from("io", "lume", "Lume").map(|dirs| dirs.data_local_dir().to_owned())
        })
        .ok_or(StoreError::NoProjectDirectories)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentKind {
    Local,
    Wsl,
    Ssh,
}

impl EnvironmentKind {
    fn as_db(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Wsl => "wsl",
            Self::Ssh => "ssh",
        }
    }

    fn from_db(value: &str) -> Result<Self, StoreError> {
        match value {
            "local" => Ok(Self::Local),
            "wsl" => Ok(Self::Wsl),
            "ssh" => Ok(Self::Ssh),
            other => Err(StoreError::InvalidEnvironmentKind(other.into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentProfile {
    pub id: String,
    pub label: String,
    pub kind: EnvironmentKind,
}

/// Profilo di connessione SQL, parte **non segreta**.
///
/// Password e client secret non compaiono, e non è una dimenticanza: stanno
/// nel vault del sistema operativo. Qui c'è solo dove connettersi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlConnectionRecord {
    pub id: String,
    pub label: String,
    /// `azure-sql` oppure `oracle`; la traduzione in un enum sta in
    /// `lume-app`, così lo store non dipende da `lume-sql`.
    pub engine: String,
    pub host: String,
    pub database: String,
    /// `sql-login` oppure `service-principal`.
    pub auth: String,
    pub username: Option<String>,
    pub tenant_id: Option<String>,
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppearancePreferences {
    pub color_theme_id: String,
    pub icon_theme_id: String,
}

impl AppearancePreferences {
    pub fn new(color_theme_id: impl Into<String>, icon_theme_id: impl Into<String>) -> Self {
        Self {
            color_theme_id: color_theme_id.into(),
            icon_theme_id: icon_theme_id.into(),
        }
    }
}

impl Default for AppearancePreferences {
    fn default() -> Self {
        Self::new("officina", "material")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionSnapshot {
    pub workspace_root: Option<PathBuf>,
    pub tabs: Vec<PathBuf>,
    pub active_tab: Option<PathBuf>,
    pub appearance: AppearancePreferences,
    pub panel_width: f32,
    pub left_tools: Vec<String>,
    pub right_tools: Vec<String>,
    pub active_panel: Option<String>,
    /// Tool Dock mostrato (None = dock chiuso).
    pub active_dock: Option<String>,
    pub dock_height: f32,
    pub environment_id: String,
    /// Segnalibri (F11) del progetto.
    pub bookmarks: Vec<BookmarkRecord>,
    /// Cronologia recente (aperture, salvataggi, commit), più recente per prima.
    pub history: Vec<HistoryRecord>,
    /// Il **secondo** gruppo dell'editor, se la sessione era divisa.
    ///
    /// `tabs`/`active_tab` restano il gruppo 0 e non cambiano forma: così la
    /// compatibilità vale in **entrambe** le direzioni — un binario vecchio
    /// riapre una sessione nuova mostrando le schede del gruppo 0 e
    /// ignorando il resto, invece di non aprire niente.
    pub secondary_group: Option<TabGroupRecord>,
    /// Il fuoco era sul secondo gruppo.
    pub focus_secondary: bool,
    /// Quanto spazio prende il gruppo 0, da 0 a 1. Il layout non conosce i
    /// pixel: `Length::FillPortion` fa il resto.
    pub split_ratio: f32,
    /// Le funzionalità che l'utente ha **spento** nel pannello Estensioni.
    ///
    /// Si salvano le spente e non le accese di proposito: una funzionalità
    /// aggiunta domani nasce **accesa** anche per una sessione salvata oggi,
    /// coerentemente col ripristino dei tool, che aggiunge in coda quelli
    /// mancanti invece di dedurre l'assenza da una lista.
    pub disabled_features: Vec<String>,
    /// Stato della console SQL al momento del salvataggio (testo + ultimo
    /// risultato). I socket non si ripristinano: l'ibernazione resta.
    #[serde(default)]
    pub sql_console: Option<SqlConsoleSnapshot>,
}

/// Tetto righe risultato nella sessione (stesso ordine di grandezza di una pagina).
pub const SQL_SNAPSHOT_MAX_ROWS: usize = 20;
/// Tetto byte JSON per un result set salvato.
pub const SQL_SNAPSHOT_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SqlConsoleSnapshot {
    pub editor_text: String,
    pub compare: bool,
    pub sync_scroll: bool,
    pub panes: Vec<SqlConsolePaneSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SqlConsolePaneSnapshot {
    pub connection_id: Option<String>,
    pub page: usize,
    pub status: SqlPaneStatusSnapshot,
    pub results: Option<SqlResultSnapshot>,
    /// Result set successivi del batch SQL; default vuoto per snapshot legacy.
    #[serde(default)]
    pub additional_results: Vec<SqlResultSnapshot>,
    /// Indice del result set attivo al momento dello snapshot.
    #[serde(default)]
    pub selected_result_set: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SqlPaneStatusSnapshot {
    Idle,
    Running,
    Ok,
    Error(String),
    /// Il result set era troppo grosso per il disco locale.
    Truncated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SqlResultSnapshot {
    pub columns: Vec<SqlColumnSnapshot>,
    pub rows: Vec<Vec<SqlCellSnapshot>>,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SqlColumnSnapshot {
    pub name: String,
    pub type_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SqlCellSnapshot {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
}

/// Un gruppo di schede dell'editor: i file aperti e quale era attivo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabGroupRecord {
    pub tabs: Vec<PathBuf>,
    pub active: Option<PathBuf>,
}

/// Metà e metà: la divisione nasce simmetrica.
pub const DEFAULT_SPLIT_RATIO: f32 = 0.5;

const fn default_split_ratio() -> f32 {
    DEFAULT_SPLIT_RATIO
}

/// Segnalibro: file + riga (1-based) + anteprima della riga.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkRecord {
    pub path: PathBuf,
    pub line: usize,
    #[serde(default)]
    pub preview: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryAction {
    Opened,
    Saved,
    Committed,
    AgentWrote,
    AgentRan,
}

/// Voce di cronologia: istante unix (secondi), azione, etichetta e file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub at: u64,
    pub action: HistoryAction,
    pub label: String,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

impl SessionSnapshot {
    pub fn empty() -> Self {
        Self {
            workspace_root: None,
            tabs: Vec::new(),
            active_tab: None,
            appearance: AppearancePreferences::default(),
            panel_width: 220.0,
            left_tools: Vec::new(),
            right_tools: Vec::new(),
            active_panel: None,
            active_dock: None,
            dock_height: 300.0,
            environment_id: LOCAL_ENVIRONMENT_ID.into(),
            bookmarks: Vec::new(),
            history: Vec::new(),
            disabled_features: Vec::new(),
            secondary_group: None,
            focus_secondary: false,
            split_ratio: DEFAULT_SPLIT_RATIO,
            sql_console: None,
        }
    }
}

#[derive(Deserialize)]
struct SessionSnapshotWire {
    workspace_root: Option<PathBuf>,
    tabs: Vec<PathBuf>,
    active_tab: Option<PathBuf>,
    #[serde(default)]
    appearance: Option<AppearancePreferences>,
    #[serde(default)]
    theme_name: Option<String>,
    panel_width: f32,
    left_tools: Vec<String>,
    right_tools: Vec<String>,
    active_panel: Option<String>,
    /// Campi delle sessioni precedenti al dock unificato (solo lettura).
    #[serde(default)]
    open_tools: Vec<String>,
    #[serde(default)]
    tool_heights: BTreeMap<String, f32>,
    #[serde(default)]
    active_dock: Option<String>,
    #[serde(default)]
    dock_height: Option<f32>,
    environment_id: String,
    #[serde(default)]
    bookmarks: Vec<BookmarkRecord>,
    #[serde(default)]
    history: Vec<HistoryRecord>,
    #[serde(default)]
    disabled_features: Vec<String>,
    #[serde(default)]
    secondary_group: Option<TabGroupRecord>,
    #[serde(default)]
    focus_secondary: bool,
    #[serde(default = "default_split_ratio")]
    split_ratio: f32,
    #[serde(default)]
    sql_console: Option<SqlConsoleSnapshot>,
}

impl<'de> Deserialize<'de> for SessionSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SessionSnapshotWire::deserialize(deserializer)?;
        let appearance = wire.appearance.unwrap_or_else(|| {
            AppearancePreferences::new(
                wire.theme_name
                    .as_deref()
                    .map(legacy_color_theme_id)
                    .unwrap_or("officina"),
                "material",
            )
        });
        Ok(Self {
            workspace_root: wire.workspace_root,
            tabs: wire.tabs,
            active_tab: wire.active_tab,
            appearance,
            panel_width: wire.panel_width,
            left_tools: wire.left_tools,
            right_tools: wire.right_tools,
            active_panel: wire.active_panel,
            // sessioni legacy: il primo tool bottom aperto diventa il dock e la
            // sua altezza (poi clampata dal registry) l'altezza del dock
            active_dock: wire
                .active_dock
                .or_else(|| wire.open_tools.first().cloned()),
            dock_height: wire
                .dock_height
                .or_else(|| wire.tool_heights.get("terminal").copied())
                .unwrap_or(300.0),
            environment_id: wire.environment_id,
            bookmarks: wire.bookmarks,
            history: wire.history,
            disabled_features: wire.disabled_features,
            secondary_group: wire.secondary_group,
            focus_secondary: wire.focus_secondary,
            // una sessione scritta male non deve dare una colonna invisibile
            split_ratio: wire.split_ratio.clamp(0.15, 0.85),
            sql_console: wire.sql_console,
        })
    }
}

fn legacy_color_theme_id(name: &str) -> &'static str {
    match name.to_lowercase().as_str() {
        "grafite" => "grafite",
        "ossido" => "ossido",
        "inchiostro" => "inchiostro",
        "terracotta" => "terracotta",
        _ => "officina",
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("directory dati applicazione non disponibile")]
    NoProjectDirectories,
    #[error("errore I/O store: {0}")]
    Io(#[from] std::io::Error),
    #[error("errore SQLite store: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("errore serializzazione store: {0}")]
    Json(#[from] serde_json::Error),
    #[error("schema store {found} piu recente della versione supportata")]
    UnsupportedSchema { found: i64 },
    #[error("profilo ambiente con tipo non valido: {0}")]
    InvalidEnvironmentKind(String),
    #[error("worker store chiuso")]
    WorkerClosed,
    #[error("worker store non ha risposto")]
    WorkerUnresponsive,
}

#[derive(Clone)]
pub struct StoreHandle {
    requests: mpsc::Sender<Request>,
}

impl StoreHandle {
    pub async fn open_default() -> Result<Self, StoreError> {
        let override_path = std::env::var_os(DATA_DIR_OVERRIDE_ENV);
        let protect_default_data = override_path.as_ref().is_none_or(|path| path.is_empty());
        Self::open_at_with_permissions(
            application_data_dir_from(override_path)?.join("lume.sqlite3"),
            protect_default_data,
        )
        .await
    }

    pub async fn open_at(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_at_with_permissions(path.into(), false).await
    }

    async fn open_at_with_permissions(
        path: PathBuf,
        protect_default_data: bool,
    ) -> Result<Self, StoreError> {
        let (requests, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        thread::Builder::new()
            .name("lume-store".into())
            .spawn(move || worker(path, protect_default_data, receiver, ready_tx))
            .map_err(StoreError::Io)?;
        ready_rx
            .await
            .map_err(|_| StoreError::WorkerUnresponsive)??;
        Ok(Self { requests })
    }

    pub async fn load_session(
        &self,
    ) -> Result<(SessionSnapshot, Vec<EnvironmentProfile>), StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::Load { reply })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver.await.map_err(|_| StoreError::WorkerUnresponsive)?
    }

    pub async fn save_session(&self, snapshot: SessionSnapshot) -> Result<(), StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::Save {
                snapshot: Box::new(snapshot),
                reply,
            })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver.await.map_err(|_| StoreError::WorkerUnresponsive)?
    }

    pub async fn load_sql_connections(&self) -> Result<Vec<SqlConnectionRecord>, StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::SqlLoad { reply })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver.await.map_err(|_| StoreError::WorkerUnresponsive)?
    }

    /// Inserisce o aggiorna un profilo. L'id è la chiave: risalvarlo con lo
    /// stesso id lo modifica invece di duplicarlo.
    pub async fn save_sql_connection(&self, record: SqlConnectionRecord) -> Result<(), StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::SqlSave {
                record: Box::new(record),
                reply,
            })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver.await.map_err(|_| StoreError::WorkerUnresponsive)?
    }

    pub async fn delete_sql_connection(&self, id: String) -> Result<(), StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::SqlDelete { id, reply })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver.await.map_err(|_| StoreError::WorkerUnresponsive)?
    }

    pub async fn load_notes_directory(&self) -> Result<Option<PathBuf>, StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::PreferenceLoad {
                key: NOTES_DIRECTORY_KEY,
                reply,
            })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver
            .await
            .map_err(|_| StoreError::WorkerUnresponsive)?
            .map(|value| value.map(PathBuf::from))
    }

    pub async fn save_notes_directory(&self, directory: PathBuf) -> Result<(), StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::PreferenceSave {
                key: NOTES_DIRECTORY_KEY,
                value: directory.to_string_lossy().into_owned(),
                reply,
            })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver.await.map_err(|_| StoreError::WorkerUnresponsive)?
    }

    pub async fn load_mcp_mode(&self, root: &Path) -> Result<Option<String>, StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::McpModeLoad {
                key: mcp_mode_key(root),
                reply,
            })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver.await.map_err(|_| StoreError::WorkerUnresponsive)?
    }

    pub async fn save_mcp_mode(&self, root: &Path, mode: &str) -> Result<(), StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Request::McpModeSave {
                key: mcp_mode_key(root),
                value: mode.to_owned(),
                reply,
            })
            .map_err(|_| StoreError::WorkerClosed)?;
        receiver.await.map_err(|_| StoreError::WorkerUnresponsive)?
    }
}

enum Request {
    Load {
        reply: oneshot::Sender<Result<(SessionSnapshot, Vec<EnvironmentProfile>), StoreError>>,
    },
    Save {
        snapshot: Box<SessionSnapshot>,
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
    SqlLoad {
        reply: oneshot::Sender<Result<Vec<SqlConnectionRecord>, StoreError>>,
    },
    SqlSave {
        record: Box<SqlConnectionRecord>,
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
    SqlDelete {
        id: String,
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
    PreferenceLoad {
        key: &'static str,
        reply: oneshot::Sender<Result<Option<String>, StoreError>>,
    },
    PreferenceSave {
        key: &'static str,
        value: String,
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
    McpModeLoad {
        key: String,
        reply: oneshot::Sender<Result<Option<String>, StoreError>>,
    },
    McpModeSave {
        key: String,
        value: String,
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
}

fn worker(
    path: PathBuf,
    protect_default_data: bool,
    requests: mpsc::Receiver<Request>,
    ready: oneshot::Sender<Result<(), StoreError>>,
) {
    let mut connection = match open_connection(&path, protect_default_data) {
        Ok(connection) => {
            let _ = ready.send(Ok(()));
            connection
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    while let Ok(request) = requests.recv() {
        match request {
            Request::Load { reply } => {
                let _ = reply.send(load_session(&connection));
            }
            Request::Save { snapshot, reply } => {
                let _ = reply.send(save_session(&mut connection, *snapshot));
            }
            Request::SqlLoad { reply } => {
                let _ = reply.send(load_sql_connections(&connection));
            }
            Request::SqlSave { record, reply } => {
                let _ = reply.send(save_sql_connection(&mut connection, &record));
            }
            Request::SqlDelete { id, reply } => {
                let _ = reply.send(delete_sql_connection(&mut connection, &id));
            }
            Request::PreferenceLoad { key, reply } => {
                let result = connection
                    .query_row(
                        "SELECT value FROM user_preference WHERE key = ?1",
                        params![key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(StoreError::from);
                let _ = reply.send(result);
            }
            Request::PreferenceSave { key, value, reply } => {
                let result = connection
                    .execute(
                        "INSERT INTO user_preference (key, value) VALUES (?1, ?2)
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                        params![key, value],
                    )
                    .map(|_| ())
                    .map_err(StoreError::from);
                let _ = reply.send(result);
            }
            Request::McpModeLoad { key, reply } => {
                let result = connection
                    .query_row(
                        "SELECT value FROM user_preference WHERE key = ?1",
                        params![key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(StoreError::from);
                let _ = reply.send(result);
            }
            Request::McpModeSave { key, value, reply } => {
                let result = connection.execute(
                    "INSERT INTO user_preference (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                ).map(|_| ()).map_err(StoreError::from);
                let _ = reply.send(result);
            }
        }
    }
}

fn open_connection(path: &Path, protect_default_data: bool) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent() {
        if protect_default_data {
            #[cfg(unix)]
            ensure_private_default_data_dir(parent)?;
            #[cfg(not(unix))]
            std::fs::create_dir_all(parent)?;
        } else {
            std::fs::create_dir_all(parent)?;
        }
    }
    #[cfg(unix)]
    if protect_default_data {
        validate_private_database_target(path)?;
    }
    let mut connection = Connection::open(path)?;
    #[cfg(unix)]
    if protect_default_data {
        set_private_database_permissions(path)?;
    }
    migrate(&mut connection)?;
    Ok(connection)
}

#[cfg(unix)]
fn ensure_private_default_data_dir(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "la directory dati predefinita non è una directory regolare",
        )
        .into());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permessi 0700 della directory dati predefinita non confermati",
        )
        .into());
    }
    Ok(())
}

#[cfg(unix)]
fn validate_private_database_target(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::MetadataExt;

    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.nlink() != 1 =>
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "database preesistente non regolare o con hard link",
            )
            .into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_private_database_permissions(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "database non regolare o con hard link dopo apertura",
        )
        .into());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != 0o600
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permessi 0600 del database non confermati",
        )
        .into());
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema { found: version });
    }
    if version == SCHEMA_VERSION {
        return Ok(());
    }

    // Tutti i passi pendenti stanno in una sola transazione `BEGIN IMMEDIATE`:
    // un errore a qualunque passo lascia il file esattamente alla versione di
    // partenza, `user_version` compresa e dati compresi. SQLite sa fare DDL e
    // `PRAGMA user_version` dentro una transazione (sono voci del journal);
    // `journal_mode` e `foreign_keys` sarebbero gli unici pragma da tenere
    // fuori, e qui non se ne usa nessuno.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Il lock di scrittura è nostro da qui in poi: rileggere la versione evita
    // di ripetere passi che un altro processo ha già committato fra la prima
    // lettura e `BEGIN`.
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema { found: version });
    }
    if version < 1 {
        transaction.execute_batch(
            "CREATE TABLE app_session (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            snapshot_json TEXT NOT NULL
        );
        CREATE TABLE environment_profile (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('local', 'wsl', 'ssh'))
        );",
        )?;
        transaction.execute(
            "INSERT INTO environment_profile (id, label, kind) VALUES (?1, ?2, ?3)",
            params![
                LOCAL_ENVIRONMENT_ID,
                "Locale",
                EnvironmentKind::Local.as_db()
            ],
        )?;
    }
    if version < 2 {
        transaction.execute_batch(
            "CREATE TABLE workspace_trust (
                canonical_root TEXT PRIMARY KEY,
                configuration_fingerprint TEXT NOT NULL,
                decision TEXT NOT NULL CHECK (decision IN ('trusted', 'denied')),
                updated_at INTEGER NOT NULL
            );",
        )?;
    }
    if version < 3 {
        // Nessuna colonna per password o client secret, e non è una svista:
        // i segreti stanno nel vault del sistema operativo, mai qui. Questa
        // tabella descrive *dove* connettersi, non *con che cosa*.
        transaction.execute_batch(
            "CREATE TABLE sql_connection (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                engine TEXT NOT NULL CHECK (engine IN ('azure-sql', 'oracle')),
                host TEXT NOT NULL,
                database TEXT NOT NULL,
                auth TEXT NOT NULL CHECK (auth IN ('sql-login', 'service-principal')),
                username TEXT,
                tenant_id TEXT,
                client_id TEXT
            );",
        )?;
    }
    if version < 4 {
        transaction.execute_batch(
            "CREATE TABLE user_preference (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
    }
    if version < 5 {
        // N24: l'archivio delle decisioni di fiducia non aveva chiamanti.
        // Il controllo che conta (fail-closed) sta in `lume-lsp`, non qui.
        transaction.execute_batch("DROP TABLE IF EXISTS workspace_trust;")?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn load_session(
    connection: &Connection,
) -> Result<(SessionSnapshot, Vec<EnvironmentProfile>), StoreError> {
    let session_json: Option<String> = connection
        .query_row(
            "SELECT snapshot_json FROM app_session WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let session = match session_json {
        Some(json) => serde_json::from_str(&json)?,
        None => SessionSnapshot::empty(),
    };
    let mut statement =
        connection.prepare("SELECT id, label, kind FROM environment_profile ORDER BY id")?;
    let environments = statement
        .query_map([], |row| {
            let kind: String = row.get(2)?;
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, kind))
        })?
        .map(|row| {
            let (id, label, kind) = row?;
            Ok(EnvironmentProfile {
                id,
                label,
                kind: EnvironmentKind::from_db(&kind)?,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok((session, environments))
}

fn load_sql_connections(connection: &Connection) -> Result<Vec<SqlConnectionRecord>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id, label, engine, host, database, auth, username, tenant_id, client_id
         FROM sql_connection ORDER BY label, id",
    )?;
    let records = statement
        .query_map([], |row| {
            Ok(SqlConnectionRecord {
                id: row.get(0)?,
                label: row.get(1)?,
                engine: row.get(2)?,
                host: row.get(3)?,
                database: row.get(4)?,
                auth: row.get(5)?,
                username: row.get(6)?,
                tenant_id: row.get(7)?,
                client_id: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(records)
}

fn save_sql_connection(
    connection: &mut Connection,
    record: &SqlConnectionRecord,
) -> Result<(), StoreError> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO sql_connection
             (id, label, engine, host, database, auth, username, tenant_id, client_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
             label = excluded.label,
             engine = excluded.engine,
             host = excluded.host,
             database = excluded.database,
             auth = excluded.auth,
             username = excluded.username,
             tenant_id = excluded.tenant_id,
             client_id = excluded.client_id",
        params![
            record.id,
            record.label,
            record.engine,
            record.host,
            record.database,
            record.auth,
            record.username,
            record.tenant_id,
            record.client_id,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn delete_sql_connection(connection: &mut Connection, id: &str) -> Result<(), StoreError> {
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM sql_connection WHERE id = ?1", params![id])?;
    transaction.commit()?;
    Ok(())
}

fn save_session(connection: &mut Connection, snapshot: SessionSnapshot) -> Result<(), StoreError> {
    let snapshot_json = serde_json::to_string(&snapshot)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO app_session (id, snapshot_json) VALUES (1, ?1)
         ON CONFLICT(id) DO UPDATE SET snapshot_json = excluded.snapshot_json",
        params![snapshot_json],
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn db_path(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lume_store_{tag}_{}_{}.sqlite3",
            std::process::id(),
            nonce
        ))
    }

    #[test]
    fn la_modalita_mcp_e_salvata_per_progetto_e_sopravvive_alla_riapertura() {
        let path = db_path("mcp_mode");
        let root = Path::new(r"C:\Progetti\Lume");
        let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
        assert_eq!(
            futures::executor::block_on(store.load_mcp_mode(root)).unwrap(),
            None
        );
        futures::executor::block_on(store.save_mcp_mode(root, "auto")).unwrap();
        drop(store);
        let reopened = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
        assert_eq!(
            futures::executor::block_on(reopened.load_mcp_mode(root))
                .unwrap()
                .as_deref(),
            Some("auto")
        );
    }

    #[cfg(windows)]
    #[test]
    fn chiave_modalita_normalizza_separatore_maiuscole_e_trailing_slash() {
        assert_eq!(
            mcp_mode_key(Path::new(r"C:\Progetti\Lume")),
            r"mcp.mode:c:\progetti\lume"
        );
        assert_eq!(
            mcp_mode_key(Path::new("c:/PROGETTI/LUME/")),
            r"mcp.mode:c:\progetti\lume"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn chiave_modalita_su_unix_distingue_maiuscole_e_non_riscrive_i_separatori() {
        assert_eq!(
            mcp_mode_key(Path::new("/home/utente/Progetto/")),
            "mcp.mode:/home/utente/Progetto"
        );
        assert_ne!(
            mcp_mode_key(Path::new("/home/utente/Progetto")),
            mcp_mode_key(Path::new("/home/utente/progetto"))
        );
        assert_ne!(
            mcp_mode_key(Path::new("/home/utente/a/b")),
            mcp_mode_key(Path::new(r"/home/utente/a\b"))
        );
    }

    #[test]
    fn le_azioni_agente_si_serializzano_nella_sessione() {
        for action in [HistoryAction::AgentWrote, HistoryAction::AgentRan] {
            let record = HistoryRecord {
                at: 7,
                action,
                label: "src/main.rs".into(),
                path: Some("src/main.rs".into()),
            };
            let encoded = serde_json::to_string(&record).unwrap();
            assert_eq!(
                serde_json::from_str::<HistoryRecord>(&encoded).unwrap(),
                record
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn lo_store_locale_protegge_la_nuova_directory_e_il_database() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("lume");
        let database = directory.join("lume.sqlite3");

        let connection = open_connection(&database, true).unwrap();
        drop(connection);

        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&database).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn lo_store_con_override_non_modifica_i_permessi_esistenti() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("override");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o750)).unwrap();
        let database = directory.join("lume.sqlite3");
        std::fs::File::create(&database).unwrap();
        std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o640)).unwrap();

        let store = futures::executor::block_on(StoreHandle::open_at(&database)).unwrap();

        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            std::fs::metadata(&database).unwrap().permissions().mode() & 0o777,
            0o640
        );
        drop(store);
    }

    #[cfg(unix)]
    #[test]
    fn lo_store_locale_rende_privati_directory_e_database_legacy() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("lume");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        let database = directory.join("lume.sqlite3");
        std::fs::File::create(&database).unwrap();
        std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o644)).unwrap();

        let connection = open_connection(&database, true).unwrap();
        drop(connection);

        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&database).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn lo_store_locale_rifiuta_database_symlink_senza_scrivere_sul_target() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("lume");
        std::fs::create_dir(&directory).unwrap();
        let target = parent.path().join("target.sqlite3");
        let original = b"dati non SQLite da preservare";
        std::fs::write(&target, original).unwrap();
        let database = directory.join("lume.sqlite3");
        std::os::unix::fs::symlink(&target, &database).unwrap();

        let result = open_connection(&database, true);

        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn open_default_protegge_il_percorso_utente_senza_override() {
        const CHILD: &str = "LUME_STORE_PRIVATE_DEFAULT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let directory = application_data_dir().unwrap();
            let store = futures::executor::block_on(StoreHandle::open_default()).unwrap();
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(directory.join("lume.sqlite3"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            drop(store);
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let xdg_data = home.path().join("data");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::open_default_protegge_il_percorso_utente_senza_override",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HOME", home.path())
            .env("XDG_DATA_HOME", &xdg_data)
            .env_remove(DATA_DIR_OVERRIDE_ENV)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "figlio fallito: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_default_conserva_i_permessi_dell_override_utente() {
        const CHILD: &str = "LUME_STORE_OVERRIDE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            futures::executor::block_on(StoreHandle::open_default()).unwrap();
            return;
        }

        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("custom-data");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o750)).unwrap();
        let database = directory.join("lume.sqlite3");
        std::fs::File::create(&database).unwrap();
        std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o640)).unwrap();

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::open_default_conserva_i_permessi_dell_override_utente",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(DATA_DIR_OVERRIDE_ENV, &directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "figlio fallito: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            std::fs::metadata(&database).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[test]
    fn directory_override_is_used_without_touching_the_profile_directory() {
        let isolated = PathBuf::from(r"D:\\benchmark\\sessione-uno");
        assert_eq!(
            application_data_dir_from(Some(isolated.clone().into_os_string())).unwrap(),
            isolated
        );
    }

    #[test]
    fn la_cartella_degli_appunti_sta_sotto_l_override() {
        let isolata = PathBuf::from("dati isolati è");
        assert_eq!(
            notes_directory_from(Some(isolata.clone().into_os_string())).unwrap(),
            isolata.join("notes")
        );
    }

    #[test]
    fn senza_override_la_cartella_degli_appunti_e_quella_dell_utente() {
        assert_eq!(
            notes_directory_from(None).unwrap(),
            application_data_dir_from(None).unwrap().join("notes")
        );
    }

    #[test]
    fn un_override_vuoto_vale_come_assente() {
        assert_eq!(
            notes_directory_from(Some(OsString::new())).unwrap(),
            notes_directory_from(None).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn la_radice_degli_appunti_conserva_i_byte_non_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let radice = PathBuf::from(OsString::from_vec(b"dati-\xff".to_vec()));
        assert_eq!(
            notes_directory_from(Some(radice.clone().into_os_string())).unwrap(),
            radice.join("notes")
        );
    }

    #[test]
    fn first_open_creates_local_environment_and_empty_session() {
        let path = db_path("first_open");
        let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
        let (session, environments) = futures::executor::block_on(store.load_session()).unwrap();
        assert_eq!(session, SessionSnapshot::empty());
        assert_eq!(
            environments,
            vec![EnvironmentProfile {
                id: LOCAL_ENVIRONMENT_ID.into(),
                label: "Locale".into(),
                kind: EnvironmentKind::Local,
            }]
        );
    }

    #[test]
    fn saved_session_roundtrips_all_session_fields() {
        let path = db_path("roundtrip");
        let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
        let snapshot = SessionSnapshot {
            workspace_root: Some(PathBuf::from("C:/work/lume")),
            tabs: vec![PathBuf::from("C:/work/lume/src/main.rs")],
            active_tab: Some(PathBuf::from("C:/work/lume/src/main.rs")),
            appearance: AppearancePreferences::new("grafite", "catppuccin-mocha"),
            panel_width: 333.0,
            left_tools: vec!["project".into()],
            right_tools: vec!["search".into()],
            active_panel: Some("project".into()),
            active_dock: Some("terminal".into()),
            dock_height: 300.0,
            environment_id: LOCAL_ENVIRONMENT_ID.into(),
            bookmarks: vec![BookmarkRecord {
                path: PathBuf::from("C:/work/lume/src/main.rs"),
                line: 7,
                preview: "fn main() {".into(),
            }],
            history: vec![HistoryRecord {
                at: 1_700_000_000,
                action: HistoryAction::Saved,
                label: "main.rs".into(),
                path: Some(PathBuf::from("C:/work/lume/src/main.rs")),
            }],
            disabled_features: vec!["database".into(), "terminale".into()],
            secondary_group: Some(TabGroupRecord {
                tabs: vec![PathBuf::from("C:/work/lume/src/app.rs")],
                active: Some(PathBuf::from("C:/work/lume/src/app.rs")),
            }),
            focus_secondary: true,
            split_ratio: 0.4,
            sql_console: None,
        };
        futures::executor::block_on(store.save_session(snapshot.clone())).unwrap();
        assert_eq!(
            futures::executor::block_on(store.load_session()).unwrap().0,
            snapshot
        );
    }

    #[test]
    fn future_schema_is_rejected_without_replacing_database() {
        let path = db_path("future_schema");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            futures::executor::block_on(StoreHandle::open_at(&path)),
            Err(StoreError::UnsupportedSchema { found }) if found == SCHEMA_VERSION + 1
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// Le tabelle presenti nel file, lette da una connessione separata da
    /// quella del worker (SQLite ammette più connessioni allo stesso file).
    fn tabelle(path: &Path) -> Vec<String> {
        let connection = rusqlite::Connection::open(path).unwrap();
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|nome| nome.unwrap())
            .collect()
    }

    fn versione(path: &Path) -> i64 {
        rusqlite::Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    /// Una base come la scriveva Lume alla versione indicata (1–4), con dati
    /// sentinella in ogni tabella che esiste a quella versione: è il punto di
    /// partenza delle prove di migrazione.
    fn base_alla_versione(path: &Path, versione: i64) {
        assert!(
            (1..=4).contains(&versione),
            "versione di partenza non storica: {versione}"
        );
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE app_session (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    snapshot_json TEXT NOT NULL
                );
                INSERT INTO app_session VALUES (
                    1,
                    '{\"workspace_root\":\"C:/lavoro/lume\",\"tabs\":[],\"active_tab\":null,\"panel_width\":220.0,\"left_tools\":[],\"right_tools\":[],\"active_panel\":null,\"environment_id\":\"local\"}'
                );
                CREATE TABLE environment_profile (
                    id TEXT PRIMARY KEY,
                    label TEXT NOT NULL,
                    kind TEXT NOT NULL CHECK (kind IN ('local', 'wsl', 'ssh'))
                );
                INSERT INTO environment_profile (id, label, kind)
                    VALUES ('local', 'Locale', 'local');",
            )
            .unwrap();
        if versione >= 2 {
            connection
                .execute_batch(
                    "CREATE TABLE workspace_trust (
                        canonical_root TEXT PRIMARY KEY,
                        configuration_fingerprint TEXT NOT NULL,
                        decision TEXT NOT NULL CHECK (decision IN ('trusted', 'denied')),
                        updated_at INTEGER NOT NULL
                    );
                    INSERT INTO workspace_trust
                        VALUES ('C:/work/lume', 'impronta', 'trusted', 1);",
                )
                .unwrap();
        }
        if versione >= 3 {
            connection
                .execute_batch(
                    "CREATE TABLE sql_connection (
                        id TEXT PRIMARY KEY,
                        label TEXT NOT NULL,
                        engine TEXT NOT NULL CHECK (engine IN ('azure-sql', 'oracle')),
                        host TEXT NOT NULL,
                        database TEXT NOT NULL,
                        auth TEXT NOT NULL CHECK (auth IN ('sql-login', 'service-principal')),
                        username TEXT,
                        tenant_id TEXT,
                        client_id TEXT
                    );
                    INSERT INTO sql_connection
                        VALUES ('azure-prod', 'azure · prod', 'azure-sql',
                                'acme.database.windows.net', 'prod', 'sql-login',
                                'lume', NULL, NULL);",
                )
                .unwrap();
        }
        if versione >= 4 {
            connection
                .execute_batch(
                    "CREATE TABLE user_preference (
                        key TEXT PRIMARY KEY,
                        value TEXT NOT NULL
                    );
                    INSERT INTO user_preference VALUES ('notes.directory', 'C:/appunti');",
                )
                .unwrap();
        }
        connection
            .execute_batch(&format!("PRAGMA user_version = {versione};"))
            .unwrap();
    }

    /// Ostacolo vero nel file, non un gancio di test nella produzione: un
    /// oggetto già presente che fa fallire l'SQL della migrazione.
    /// `PreferenzaGiaPresente` riproduce la diagnosi del 13/10 (la `CREATE
    /// TABLE user_preference` del passo 4 trova la tabella); la vista con il
    /// nome di `workspace_trust` fa fallire l'ultimo passo, perché
    /// `DROP TABLE IF EXISTS` rifiuta una vista.
    #[derive(Clone, Copy)]
    enum Ostacolo {
        PreferenzaGiaPresente,
        WorkspaceTrustEVista,
    }

    /// Prepara la base con l'ostacolo e restituisce l'SQL che lo rimuove.
    fn base_con_ostacolo(path: &Path, versione: i64, ostacolo: Ostacolo) -> &'static str {
        base_alla_versione(path, versione);
        let connection = rusqlite::Connection::open(path).unwrap();
        match ostacolo {
            Ostacolo::PreferenzaGiaPresente => {
                connection
                    .execute_batch(
                        "CREATE TABLE user_preference (
                            chiave TEXT PRIMARY KEY,
                            valore TEXT NOT NULL
                        );
                        INSERT INTO user_preference VALUES ('ostacolo', 'presente');",
                    )
                    .unwrap();
                "DROP TABLE user_preference;"
            }
            Ostacolo::WorkspaceTrustEVista => {
                connection
                    .execute_batch(
                        "DROP TABLE workspace_trust;
                        CREATE VIEW workspace_trust AS
                            SELECT 'ostacolo' AS canonical_root;",
                    )
                    .unwrap();
                "DROP VIEW workspace_trust;"
            }
        }
    }

    /// Contenuto logico completo del file, in ordine stabile: versione,
    /// oggetti di schema con il loro SQL e righe di ogni tabella. Due
    /// istantanee uguali dicono che la migrazione fallita non ha lasciato
    /// traccia, né in schema né nei dati.
    fn istantanea(path: &Path) -> String {
        use rusqlite::types::ValueRef;

        let connection = rusqlite::Connection::open(path).unwrap();
        let mut righe = vec![format!(
            "user_version={}",
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap()
        )];
        let oggetti: Vec<String> = {
            let mut statement = connection
                .prepare(
                    "SELECT type, name, tbl_name, COALESCE(sql, '')
                     FROM sqlite_master ORDER BY type, name",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok(format!(
                        "schema|{}|{}|{}|{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?
                    ))
                })
                .unwrap()
                .map(|riga| riga.unwrap())
                .collect()
        };
        righe.extend(oggetti);
        let tabelle: Vec<String> = {
            let mut statement = connection
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(|riga| riga.unwrap())
                .collect()
        };
        for tabella in tabelle {
            let mut statement = connection
                .prepare(&format!(r#"SELECT * FROM "{tabella}""#))
                .unwrap();
            let colonne = statement.column_count();
            let mut valori: Vec<String> = statement
                .query_map([], |row| {
                    let mut celle = Vec::new();
                    for indice in 0..colonne {
                        celle.push(match row.get_ref(indice)? {
                            ValueRef::Null => "NULL".to_owned(),
                            ValueRef::Integer(valore) => format!("int:{valore}"),
                            ValueRef::Real(valore) => format!("real:{valore:?}"),
                            ValueRef::Text(valore) => {
                                format!("text:{}", String::from_utf8_lossy(valore))
                            }
                            ValueRef::Blob(valore) => format!("blob:{valore:?}"),
                        });
                    }
                    Ok(celle.join("\u{1f}"))
                })
                .unwrap()
                .map(|riga| riga.unwrap())
                .collect();
            valori.sort();
            for valore in valori {
                righe.push(format!("riga|{tabella}|{valore}"));
            }
        }
        righe.join("\n")
    }

    #[test]
    fn una_base_v4_perde_workspace_trust_e_tiene_tutto_il_resto() {
        assert_eq!(SCHEMA_VERSION, 5);
        let path = db_path("v4_a_v5");
        base_alla_versione(&path, 4);
        assert!(tabelle(&path).contains(&"workspace_trust".to_owned()));

        let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();

        assert_eq!(versione(&path), 5);
        assert!(
            !tabelle(&path).contains(&"workspace_trust".to_owned()),
            "la tabella workspace_trust doveva sparire: {:?}",
            tabelle(&path)
        );
        // Il resto dei dati dell'utente è intatto.
        let (_, ambienti) = futures::executor::block_on(store.load_session()).unwrap();
        assert_eq!(ambienti.len(), 1);
        assert_eq!(ambienti[0].id, LOCAL_ENVIRONMENT_ID);
        assert_eq!(
            futures::executor::block_on(store.load_notes_directory()).unwrap(),
            Some(PathBuf::from("C:/appunti"))
        );
    }

    #[test]
    fn un_database_nuovo_nasce_alla_versione_5_senza_workspace_trust() {
        let path = db_path("nuovo_v5");
        let _store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
        assert_eq!(versione(&path), 5);
        assert_eq!(
            tabelle(&path),
            vec![
                "app_session".to_owned(),
                "environment_profile".to_owned(),
                "sql_connection".to_owned(),
                "user_preference".to_owned(),
            ]
        );
    }

    #[test]
    fn la_migrazione_regge_una_base_senza_la_tabella_e_una_riapertura() {
        let path = db_path("v4_senza_tabella");
        base_alla_versione(&path, 4);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TABLE workspace_trust;")
            .unwrap();

        // Prima apertura: la tabella non c'è già, `IF EXISTS` non deve fallire.
        drop(futures::executor::block_on(StoreHandle::open_at(&path)).unwrap());
        // Seconda apertura: la base è già a v5, non si rimigra niente.
        let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();

        assert_eq!(versione(&path), 5);
        assert_eq!(
            futures::executor::block_on(store.load_notes_directory()).unwrap(),
            Some(PathBuf::from("C:/appunti"))
        );
    }

    /// Un errore a qualunque passo non deve lasciare il database a una
    /// versione intermedia: o tutti i passi, o nessuno. Le basi sono le
    /// versioni storiche 1–4 e l'ostacolo è un oggetto vero nel file, non un
    /// gancio di test nella produzione.
    #[test]
    fn una_migrazione_fallita_lascia_il_database_alla_versione_di_partenza() {
        // A v1 l'ultimo passo in grado di fallire è il 4: il passo 5 è un
        // `DROP TABLE IF EXISTS` e `workspace_trust` a v1 non esiste ancora;
        // l'ostacolo riproduce la diagnosi del 13/10. Per v2–v4 l'ostacolo
        // colpisce il passo 5, l'ultimo della catena.
        let casi = [
            (1_i64, Ostacolo::PreferenzaGiaPresente),
            (2, Ostacolo::WorkspaceTrustEVista),
            (3, Ostacolo::WorkspaceTrustEVista),
            (4, Ostacolo::WorkspaceTrustEVista),
        ];
        for (partenza, ostacolo) in casi {
            let path = db_path(&format!("fallita_v{partenza}"));
            let rimedio = base_con_ostacolo(&path, partenza, ostacolo);
            let prima = istantanea(&path);

            let errore = futures::executor::block_on(StoreHandle::open_at(&path)).err();
            assert!(
                matches!(errore.as_ref(), Some(StoreError::Sqlite(_))),
                "v{partenza}: la migrazione doveva fallire, invece {errore:?}"
            );
            assert_eq!(
                versione(&path),
                partenza,
                "v{partenza}: la versione è avanzata nonostante l'errore"
            );
            assert_eq!(
                istantanea(&path),
                prima,
                "v{partenza}: schema o dati sono cambiati dopo la migrazione fallita"
            );

            // Ripetere il tentativo non accumula passi: stesso errore, stesso
            // file.
            assert!(futures::executor::block_on(StoreHandle::open_at(&path)).is_err());
            assert_eq!(
                istantanea(&path),
                prima,
                "v{partenza}: il secondo tentativo ha lasciato traccia"
            );

            // Tolto l'ostacolo la stessa base arriva a 5 con i dati sentinella.
            rusqlite::Connection::open(&path)
                .unwrap()
                .execute_batch(rimedio)
                .unwrap();
            let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
            assert_eq!(versione(&path), 5, "v{partenza}: dopo il rimedio");
            assert!(
                !tabelle(&path).contains(&"workspace_trust".to_owned()),
                "v{partenza}: workspace_trust doveva sparire: {:?}",
                tabelle(&path)
            );
            let (sessione, ambienti) = futures::executor::block_on(store.load_session()).unwrap();
            assert_eq!(
                sessione.workspace_root,
                Some(PathBuf::from("C:/lavoro/lume")),
                "v{partenza}: la sessione sentinella è andata persa"
            );
            assert_eq!(ambienti.len(), 1);
            assert_eq!(ambienti[0].id, LOCAL_ENVIRONMENT_ID);
            assert_eq!(
                futures::executor::block_on(store.load_sql_connections())
                    .unwrap()
                    .len(),
                usize::from(partenza >= 3),
                "v{partenza}: profili SQL"
            );
            assert_eq!(
                futures::executor::block_on(store.load_notes_directory()).unwrap(),
                if partenza >= 4 {
                    Some(PathBuf::from("C:/appunti"))
                } else {
                    None
                },
                "v{partenza}: preferenza degli appunti"
            );
        }
    }

    fn sql_record() -> SqlConnectionRecord {
        SqlConnectionRecord {
            id: "azure-prod".into(),
            label: "azure · prod".into(),
            engine: "azure-sql".into(),
            host: "acme.database.windows.net".into(),
            database: "prod".into(),
            auth: "sql-login".into(),
            username: Some("lume".into()),
            tenant_id: None,
            client_id: None,
        }
    }

    #[test]
    fn sql_connections_survive_a_reopen_and_update_in_place() {
        let path = db_path("sql_connections");
        let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
        futures::executor::block_on(store.save_sql_connection(sql_record())).unwrap();

        let mut edited = sql_record();
        edited.label = "azure · collaudo".into();
        futures::executor::block_on(store.save_sql_connection(edited.clone())).unwrap();

        // riaperto da zero: e' il disco a rispondere, non una cache
        drop(store);
        let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
        assert_eq!(
            futures::executor::block_on(store.load_sql_connections()).unwrap(),
            vec![edited],
            "lo stesso id aggiorna il profilo invece di duplicarlo"
        );

        futures::executor::block_on(store.delete_sql_connection("azure-prod".into())).unwrap();
        assert!(
            futures::executor::block_on(store.load_sql_connections())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_sql_profile_has_nowhere_to_put_a_secret() {
        let path = db_path("sql_no_secret");
        let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
        futures::executor::block_on(store.save_sql_connection(sql_record())).unwrap();
        drop(store);

        // La regola («i segreti solo nel vault del sistema operativo») deve
        // valere per costruzione: se un giorno qualcuno aggiungesse una
        // colonna per la password, questo test lo ferma.
        let connection = rusqlite::Connection::open(&path).unwrap();
        let mut statement = connection
            .prepare("PRAGMA table_info(sql_connection)")
            .unwrap();
        let columns: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            !columns
                .iter()
                .any(|name| name.contains("password") || name.contains("secret")),
            "nessuna colonna per i segreti: {columns:?}"
        );
    }
}
