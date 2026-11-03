//! Scoperta e trasporto MCP su socket Unix.
//!
//! Il confine dichiarato è **un altro account locale**: un processo ostile
//! con gli stessi privilegi dell'utente, o root, non è isolato da questo
//! trasporto. La difesa è composta da:
//!
//! - una cartella `<dati>/mcp-unix-v1/` reale, dell'UID effettivo e 0700,
//!   dentro una catena di antenati non scrivibili da gruppo o altri e senza
//!   link simbolici;
//! - un'identità pubblica `<pid>-<nonce>` e un token nuovo per ogni Avvia;
//! - socket `<id>.sock` e record `<id>.json`, entrambi 0600, creati in modo
//!   esclusivo (mai unlink o overwrite preventivi);
//! - verifica dell'UID del peer dal kernel (`UnixStream::peer_cred`) sia nel
//!   server sia nel ponte, più il PID del peer nel ponte;
//! - il token nella prima riga anche per un peer dello stesso UID, con il
//!   confronto a tempo costante già usato dal protocollo.
//!
//! Questo modulo non conosce il dispatcher MCP: fornisce solo endpoint,
//! record, selezione dell'istanza e verifica delle credenziali.

use std::ffi::OsStr;
use std::fs::{File, Metadata, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;
use tokio::time::{Instant, timeout_at};

use crate::discovery::{Token, now_unix};

/// Cartella separata da `mcp/`: un vecchio ponte Windows non cancella i
/// record Unix attraverso `prune_stale`.
pub const DIR_NAME: &str = "mcp-unix-v1";
pub const SCHEMA: &str = "lume-mcp-unix-v1";
pub const MAX_RECORDS: usize = 64;
/// File di record esaminati da una singola scansione. La potatura dei record
/// stantii avviene mentre si legge, così un processo caduto non consuma il
/// tetto delle istanze attive; oltre questo tetto la scansione si ferma con
/// un avviso invece di far fallire la selezione.
pub const MAX_SCAN_RECORDS: usize = MAX_RECORDS * 4;
pub const MAX_RECORD_BYTES: u64 = 16 * 1024;
/// Unica deadline per connessione, verifica e selezione.
pub const CONNECT_DEADLINE: Duration = Duration::from_secs(3);

const DIR_MODE: u32 = 0o700;
const SOCKET_MODE: u32 = 0o600;
const RECORD_MODE: u32 = 0o600;

#[cfg(target_os = "linux")]
const MAX_SOCKET_PATH: usize = 107;
#[cfg(not(target_os = "linux"))]
const MAX_SOCKET_PATH: usize = 103;

pub fn discovery_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR_NAME)
}

pub fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// La regola dell'ownership, isolata per poterla provare senza simulare un
/// secondo account: la prova nativa con un UID diverso resta un cancello
/// separato della piattaforma.
pub fn owner_allowed(uid: u32, euid: u32) -> bool {
    uid == euid || uid == 0
}

/// Identità pubblica di una sessione: `<pid>-<nonce>`. Non è un segreto e
/// non dà autorità da sola.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pid: u32,
    nonce: String,
}

impl Identity {
    pub fn generate() -> Result<Self, String> {
        Ok(Self {
            pid: std::process::id(),
            nonce: crate::discovery::new_nonce()?,
        })
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (pid, nonce) = text.split_once('-')?;
        if nonce.len() != 8 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let pid = pid.parse::<u32>().ok()?;
        Some(Self {
            pid,
            nonce: nonce.to_owned(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl std::fmt::Display for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.pid, self.nonce)
    }
}

/// Endpoint di una sessione: cartella privata, identità e percorsi derivati.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    dir: PathBuf,
    identity: Identity,
}

impl Endpoint {
    /// Costruisce l'endpoint dell'avvio corrente. Non crea nulla su disco:
    /// la cartella viene verificata e creata solo quando il server parte.
    pub fn new(data_dir: &Path) -> Result<Self, String> {
        Self::with_identity(discovery_dir(data_dir), Identity::generate()?)
    }

    pub fn with_identity(dir: PathBuf, identity: Identity) -> Result<Self, String> {
        if !dir.is_absolute() {
            return Err(format!(
                "la cartella di scoperta {} non è assoluta: scegli una cartella dati assoluta",
                dir.display()
            ));
        }
        let endpoint = Self { dir, identity };
        endpoint.check_socket_path()?;
        Ok(endpoint)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn socket_path(&self) -> PathBuf {
        self.dir.join(format!("{}.sock", self.identity))
    }

    pub fn record_path(&self) -> PathBuf {
        self.dir.join(format!("{}.json", self.identity))
    }

    fn check_socket_path(&self) -> Result<(), String> {
        let socket = self.socket_path();
        let len = socket.as_os_str().as_bytes().len();
        if len > MAX_SOCKET_PATH {
            return Err(format!(
                "il percorso del socket {} è lungo {len} byte, oltre il limite di {MAX_SOCKET_PATH}: scegli una cartella dati più breve",
                socket.display()
            ));
        }
        Ok(())
    }
}

/// Record Unix esplicitamente versionato. Il lettore non lo interpreta mai
/// come una pipe Windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub schema: String,
    pub id: String,
    pub pid: u32,
    pub socket: PathBuf,
    pub socket_inode: u64,
    pub socket_dev: u64,
    pub root: PathBuf,
    pub token: Token,
    pub version: String,
    pub started_at: u64,
}

impl Record {
    pub fn new(
        endpoint: &Endpoint,
        root: PathBuf,
        token: Token,
        version: String,
        socket: &Metadata,
    ) -> Self {
        Self {
            schema: SCHEMA.to_owned(),
            id: endpoint.identity().to_string(),
            pid: endpoint.identity().pid(),
            socket: endpoint.socket_path(),
            socket_inode: socket.ino(),
            socket_dev: socket.dev(),
            root,
            token,
            version,
            started_at: now_unix(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    #[error("cartella di scoperta non leggibile: {0}")]
    Unreadable(String),
    #[error("elemento di scoperta non sicuro: {0}")]
    Unsafe(String),
    #[error("record oltre {MAX_RECORD_BYTES} byte: {}", .0.display())]
    TooLarge(PathBuf),
    #[error("record non valido in {path}: {reason}", path = .0.display(), reason = .1)]
    Invalid(PathBuf, String),
}

/// Un file di record saltato durante la scansione, con il percorso che
/// l'utente deve poter vedere. Non è un errore di selezione: un record
/// appena creato può essere letto a metà, quindi non viene cancellato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryNotice {
    pub path: PathBuf,
    pub reason: String,
}

impl DiscoveryNotice {
    fn from_error(path: &Path, error: &DiscoveryError) -> Self {
        let reason = match error {
            DiscoveryError::Invalid(_, reason) => reason.clone(),
            DiscoveryError::TooLarge(_) => format!("record oltre {MAX_RECORD_BYTES} byte"),
            other => other.to_string(),
        };
        Self {
            path: path.to_path_buf(),
            reason,
        }
    }
}

/// Catena dalla radice filesystem al parent della cartella di scoperta.
/// Nessun componente può essere un link, scrivibile da gruppo o altri, o
/// appartenere a un UID diverso da root/UID effettivo. Lo sticky bit non
/// concede eccezioni.
pub fn verify_parent_chain(parent: &Path) -> Result<(), DiscoveryError> {
    if !parent.is_absolute() {
        return Err(DiscoveryError::Unsafe(format!(
            "{} non è un percorso assoluto",
            parent.display()
        )));
    }
    let euid = effective_uid();
    let mut current = PathBuf::new();
    for component in parent.components() {
        match component {
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(DiscoveryError::Unsafe(format!(
                    "«..» non è ammesso nella catena di {}",
                    parent.display()
                )));
            }
            Component::Normal(name) => current.push(name),
            Component::Prefix(_) => {
                return Err(DiscoveryError::Unsafe(format!(
                    "percorso non Unix: {}",
                    parent.display()
                )));
            }
        }
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            DiscoveryError::Unreadable(format!("{}: {error}", current.display()))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(DiscoveryError::Unsafe(format!(
                "{} è un link simbolico",
                current.display()
            )));
        }
        if !metadata.is_dir() {
            return Err(DiscoveryError::Unsafe(format!(
                "{} non è una cartella",
                current.display()
            )));
        }
        if !owner_allowed(metadata.uid(), euid) {
            return Err(DiscoveryError::Unsafe(format!(
                "{} appartiene all'UID {}, non a root o all'UID effettivo {euid}",
                current.display(),
                metadata.uid()
            )));
        }
        if metadata.mode() & 0o022 != 0 {
            return Err(DiscoveryError::Unsafe(format!(
                "{} è scrivibile da gruppo o altri",
                current.display()
            )));
        }
    }
    Ok(())
}

/// Verifica una cartella di scoperta già esistente: reale, dell'UID
/// effettivo, 0700 esatti. Nessun `chmod` implicito.
pub fn verify_existing_dir(dir: &Path) -> Result<(), DiscoveryError> {
    let parent = dir.parent().ok_or_else(|| {
        DiscoveryError::Unsafe(format!("{} non ha una cartella madre", dir.display()))
    })?;
    verify_parent_chain(parent)?;
    let metadata = std::fs::symlink_metadata(dir)
        .map_err(|error| DiscoveryError::Unreadable(format!("{}: {error}", dir.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(DiscoveryError::Unsafe(format!(
            "{} non è una cartella reale",
            dir.display()
        )));
    }
    let euid = effective_uid();
    if metadata.uid() != euid {
        return Err(DiscoveryError::Unsafe(format!(
            "{} appartiene all'UID {}, non all'UID effettivo {euid}",
            dir.display(),
            metadata.uid()
        )));
    }
    if metadata.mode() & 0o7777 != DIR_MODE {
        return Err(DiscoveryError::Unsafe(format!(
            "{} ha permessi {:04o}, non 0700",
            dir.display(),
            metadata.mode() & 0o7777
        )));
    }
    Ok(())
}

/// Crea la cartella privata se manca, dopo aver verificato la catena; se
/// esiste la verifica senza correggerla.
pub fn ensure_private_dir(dir: &Path) -> Result<(), DiscoveryError> {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => verify_existing_dir(dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = dir.parent().ok_or_else(|| {
                DiscoveryError::Unsafe(format!("{} non ha una cartella madre", dir.display()))
            })?;
            verify_parent_chain(parent)?;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(DIR_MODE);
            builder.create(dir).map_err(|error| {
                DiscoveryError::Unreadable(format!("{}: {error}", dir.display()))
            })?;
            if let Err(error) = verify_existing_dir(dir) {
                // La cartella appena creata non è utilizzabile (per esempio
                // una umask che toglie il proprietario): non la si lascia
                // dietro vuota e inutilizzabile.
                let _ = std::fs::remove_dir(dir);
                return Err(error);
            }
            Ok(())
        }
        Err(error) => Err(DiscoveryError::Unreadable(format!(
            "{}: {error}",
            dir.display()
        ))),
    }
}

fn open_record(path: &Path) -> Result<File, DiscoveryError> {
    use rustix::fs::{Mode, OFlags, open};
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| {
        let error = std::io::Error::from(error);
        DiscoveryError::Unreadable(format!("{}: {error}", path.display()))
    })?;
    Ok(File::from(fd))
}

/// Legge un record con i limiti e le verifiche del confine: file regolare
/// (mai un link), 0600, UID effettivo, al massimo 16 KiB.
pub fn read_record(path: &Path, dir: &Path) -> Result<Record, DiscoveryError> {
    let mut file = open_record(path)?;
    let metadata = file
        .metadata()
        .map_err(|error| DiscoveryError::Unreadable(format!("{}: {error}", path.display())))?;
    if !metadata.is_file() {
        return Err(DiscoveryError::Unsafe(format!(
            "{} non è un file regolare",
            path.display()
        )));
    }
    let euid = effective_uid();
    if metadata.uid() != euid {
        return Err(DiscoveryError::Unsafe(format!(
            "{} appartiene all'UID {}, non all'UID effettivo {euid}",
            path.display(),
            metadata.uid()
        )));
    }
    if metadata.mode() & 0o7777 != RECORD_MODE {
        return Err(DiscoveryError::Unsafe(format!(
            "{} ha permessi {:04o}, non 0600",
            path.display(),
            metadata.mode() & 0o7777
        )));
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(DiscoveryError::TooLarge(path.to_path_buf()));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| DiscoveryError::Unreadable(format!("{}: {error}", path.display())))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(DiscoveryError::TooLarge(path.to_path_buf()));
    }
    let record = serde_json::from_slice::<Record>(&bytes)
        .map_err(|error| DiscoveryError::Invalid(path.to_path_buf(), error.to_string()))?;
    validate_record(&record, path, dir)?;
    Ok(record)
}

fn validate_record(record: &Record, path: &Path, dir: &Path) -> Result<(), DiscoveryError> {
    let invalid = |message: String| DiscoveryError::Invalid(path.to_path_buf(), message);
    if record.schema != SCHEMA {
        return Err(invalid(format!(
            "schema «{}», atteso «{SCHEMA}»",
            record.schema
        )));
    }
    let identity = Identity::parse(&record.id)
        .ok_or_else(|| invalid(format!("identità non valida: {}", record.id)))?;
    if identity.pid() != record.pid {
        return Err(invalid(format!(
            "il PID del record ({}) non corrisponde all'identità {}",
            record.pid, record.id
        )));
    }
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| invalid("nome del file non UTF-8".to_owned()))?;
    if stem != record.id {
        return Err(invalid(format!(
            "il nome del file «{stem}» non corrisponde all'identità {}",
            record.id
        )));
    }
    if record.socket != dir.join(format!("{}.sock", record.id)) {
        return Err(invalid(format!(
            "il socket {} non appartiene alla cartella {}",
            record.socket.display(),
            dir.display()
        )));
    }
    if !record.root.is_absolute() {
        return Err(invalid(format!(
            "la radice {} non è assoluta",
            record.root.display()
        )));
    }
    Ok(())
}

/// Su Linux `ECONNREFUSED` su un socket Unix, con lo stesso inode/dev del
/// record, è il segnale certo che nessuno è in ascolto. Su BSD/macOS la
/// `connect` può restituire lo stesso errore anche con un listener vivo e il
/// backlog pieno (la qualifica macOS del trasporto resta aperta): lì non si
/// pota e non si dichiara il record stantio, si chiede di riprovare.
const REFUSED_IS_CERTAIN: bool = cfg!(target_os = "linux");

/// Legge tutti i record validi della cartella **senza aprire connessioni**, con
/// le diagnostiche dei file saltati: un file illeggibile o invalido non è un
/// errore di selezione (può essere in scrittura), ma non sparisce in silenzio.
/// La potatura resta compito di `scan_records`, che connette prima di contare.
pub fn read_records(dir: &Path) -> Result<RecordScan, DiscoveryError> {
    let (paths, truncated) = record_paths(dir)?;
    let mut records = Vec::new();
    let mut notices = Vec::new();
    for path in paths {
        if records.len() == MAX_RECORDS {
            notices.push(DiscoveryNotice {
                path,
                reason: format!(
                    "oltre il tetto di {MAX_RECORDS} istanze attive: record saltato senza connettersi"
                ),
            });
            continue;
        }
        match read_record(&path, dir) {
            Ok(record) => records.push(record),
            Err(error) => notices.push(DiscoveryNotice::from_error(&path, &error)),
        }
    }
    if truncated {
        notices.push(DiscoveryNotice {
            path: dir.to_path_buf(),
            reason: format!(
                "la cartella contiene più di {MAX_SCAN_RECORDS} file di record: i più lontani non sono stati esaminati"
            ),
        });
    }
    Ok(RecordScan { records, notices })
}

/// Percorsi `.json` della cartella, ordinati e limitati a
/// `MAX_SCAN_RECORDS`. Il secondo valore dice se la cartella contiene altri
/// record oltre il tetto della scansione.
fn record_paths(dir: &Path) -> Result<(Vec<PathBuf>, bool), DiscoveryError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| DiscoveryError::Unreadable(format!("{}: {error}", dir.display())))?;
    let mut paths = Vec::new();
    let mut truncated = false;
    for entry in entries {
        let entry = entry
            .map_err(|error| DiscoveryError::Unreadable(format!("{}: {error}", dir.display())))?;
        let path = entry.path();
        if path.extension().and_then(OsStr::to_str) != Some("json") {
            continue;
        }
        if paths.len() == MAX_SCAN_RECORDS {
            truncated = true;
            break;
        }
        paths.push(path);
    }
    paths.sort();
    Ok((paths, truncated))
}

#[derive(Debug)]
pub struct RecordScan {
    pub records: Vec<Record>,
    pub notices: Vec<DiscoveryNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Liveness {
    Live,
    Dead,
    Unknown,
}

/// Stato di un record rispetto al suo socket. `Dead` è un segnale certo: il
/// socket descritto dal record (stesso inode/dev) non ha nessuno in ascolto,
/// oppure il percorso è sparito. Un record viene pubblicato solo dopo il bind
/// e l'identità non viene riusata: un socket sparito lascia un record orfano,
/// non un'istanza che può tornare viva. Un errore incerto non autorizza una
/// cancellazione; `ECONNREFUSED` vale come certo solo dove il kernel lo
/// documenta così (`REFUSED_IS_CERTAIN`).
async fn record_liveness(record: &Record, deadline: Instant) -> Liveness {
    let metadata = match std::fs::symlink_metadata(&record.socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Liveness::Dead,
        Err(_) => return Liveness::Unknown,
    };
    if !metadata.file_type().is_socket()
        || metadata.ino() != record.socket_inode
        || metadata.dev() != record.socket_dev
    {
        return Liveness::Unknown;
    }
    match timeout_at(deadline, UnixStream::connect(&record.socket)).await {
        Ok(Ok(stream)) => {
            drop(stream);
            Liveness::Live
        }
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Liveness::Dead,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            if REFUSED_IS_CERTAIN {
                Liveness::Dead
            } else {
                Liveness::Unknown
            }
        }
        _ => Liveness::Unknown,
    }
}

/// Scansione con potatura: i record stantii certi vengono rimossi prima di
/// contare le istanze attive, così il tetto resta una difesa di risorse e non
/// un blocco permanente. I file invalidi si saltano con una diagnostica e non
/// si cancellano (potrebbero essere in scrittura).
///
/// La connessione di liveness serve **prima** del conteggio (un processo
/// caduto non deve consumare il tetto dei 64 record); la connessione di
/// `probe` serve alla scelta fra i candidati. Nessuna delle due consuma il
/// tetto dei client: il server conta solo le connessioni che hanno superato il
/// preambolo (`MAX_PENDING_HANDSHAKES` limita le altre), quindi una prova non
/// può far rifiutare la connessione finale.
async fn scan_records(dir: &Path, deadline: Instant) -> Result<RecordScan, DiscoveryError> {
    let (paths, truncated) = record_paths(dir)?;
    let mut records = Vec::new();
    let mut notices = Vec::new();
    for path in paths {
        if Instant::now() >= deadline {
            notices.push(DiscoveryNotice {
                path: dir.to_path_buf(),
                reason: "potatura dei record stantii interrotta per scadenza: alcuni record non sono stati esaminati".to_owned(),
            });
            break;
        }
        let record = match read_record(&path, dir) {
            Ok(record) => record,
            Err(error) => {
                notices.push(DiscoveryNotice::from_error(&path, &error));
                continue;
            }
        };
        match record_liveness(&record, deadline).await {
            Liveness::Dead => {
                if let Err(reason) = remove_unreachable(dir, &record) {
                    notices.push(DiscoveryNotice { path, reason });
                }
            }
            Liveness::Live | Liveness::Unknown => {
                if records.len() == MAX_RECORDS {
                    notices.push(DiscoveryNotice {
                        path,
                        reason: format!(
                            "oltre il tetto di {MAX_RECORDS} istanze attive: record saltato senza connettersi"
                        ),
                    });
                } else {
                    records.push(record);
                }
            }
        }
    }
    if truncated {
        notices.push(DiscoveryNotice {
            path: dir.to_path_buf(),
            reason: format!(
                "la cartella contiene più di {MAX_SCAN_RECORDS} file di record: i più lontani non sono stati esaminati"
            ),
        });
    }
    Ok(RecordScan { records, notices })
}

/// Potatura una volta sola all'accensione del server. Non fallisce l'avvio:
/// restituisce le diagnostiche da mostrare all'utente.
pub async fn prune_stale_records(dir: &Path) -> Vec<DiscoveryNotice> {
    let deadline = Instant::now() + CONNECT_DEADLINE;
    match scan_records(dir, deadline).await {
        Ok(scan) => scan.notices,
        Err(error) => vec![DiscoveryNotice {
            path: dir.to_path_buf(),
            reason: error.to_string(),
        }],
    }
}

/// Rimozione di un record certo irraggiungibile, con lo stesso ricontrollo di
/// `remove_owned`: identità, token e inode/dev devono essere ancora quelli
/// letti prima di cancellare. Il record si rimuove per primo; il socket solo
/// se è ancora lo stesso oggetto.
fn remove_unreachable(dir: &Path, record: &Record) -> Result<bool, String> {
    let path = record_path(dir, &record.id);
    let current = match read_record(&path, dir) {
        Ok(current) => current,
        Err(DiscoveryError::Unreadable(_)) if !path.exists() => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if current != *record {
        return Ok(false);
    }
    match std::fs::symlink_metadata(&record.socket) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket()
                || metadata.ino() != record.socket_inode
                || metadata.dev() != record.socket_dev
            {
                return Ok(false);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "socket non verificabile in {}: {error}",
                record.socket.display()
            ));
        }
    }
    std::fs::remove_file(&path).map_err(|error| {
        format!(
            "record di scoperta non rimosso in {}: {error}",
            path.display()
        )
    })?;
    match std::fs::symlink_metadata(&record.socket) {
        Ok(metadata)
            if metadata.ino() == record.socket_inode && metadata.dev() == record.socket_dev =>
        {
            std::fs::remove_file(&record.socket).map_err(|error| {
                format!("socket non rimosso in {}: {error}", record.socket.display())
            })?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "socket non verificabile in {}: {error}",
                record.socket.display()
            ));
        }
    }
    Ok(true)
}

/// Guardia di record e socket: il `Drop` rimuove soltanto ciò che corrisponde
/// ancora a identità, token e inode posseduti. Gli esiti incerti restano.
#[derive(Debug)]
pub struct DiscoveryFile {
    endpoint: Endpoint,
    token: Token,
    socket_inode: u64,
    socket_dev: u64,
}

impl DiscoveryFile {
    pub fn publish(endpoint: &Endpoint, record: &Record) -> Result<Self, String> {
        let text = serde_json::to_vec_pretty(record)
            .map_err(|error| format!("record di scoperta non serializzabile: {error}"))?;
        if text.len() as u64 > MAX_RECORD_BYTES {
            return Err(format!(
                "record di scoperta oltre {MAX_RECORD_BYTES} byte: la radice è troppo lunga"
            ));
        }
        let path = endpoint.record_path();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(RECORD_MODE)
            .open(&path)
            .map_err(|error| {
                format!(
                    "record di scoperta non creato in {} (esiste già?): {error}",
                    path.display()
                )
            })?;
        // Da qui il file è nostro: ogni errore lo rimuove invece di lasciare
        // un record parziale che il ponte leggerebbe come un'istanza viva.
        let fail = |message: String| {
            let _ = std::fs::remove_file(&path);
            message
        };
        file.write_all(&text)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                fail(format!(
                    "record di scoperta non scritto in {}: {error}",
                    path.display()
                ))
            })?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            fail(format!(
                "record di scoperta non verificabile in {}: {error}",
                path.display()
            ))
        })?;
        if !metadata.is_file()
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o7777 != RECORD_MODE
        {
            return Err(fail(format!(
                "record di scoperta non sicuro in {}: atteso file regolare 0600 dell'UID effettivo",
                path.display()
            )));
        }
        Ok(Self {
            endpoint: endpoint.clone(),
            token: record.token.clone(),
            socket_inode: record.socket_inode,
            socket_dev: record.socket_dev,
        })
    }
}

impl Drop for DiscoveryFile {
    fn drop(&mut self) {
        let _ = remove_owned(
            &self.endpoint,
            &self.token,
            self.socket_inode,
            self.socket_dev,
        );
    }
}

/// Pulisce record e socket di un'identità ancora corrispondente. Usata dal
/// `Drop` della guardia e dalla chiusura esplicita dell'app.
pub fn cleanup(endpoint: &Endpoint, token: &Token) -> Result<(), String> {
    let record = read_record(&endpoint.record_path(), endpoint.dir())
        .map_err(|error| format!("record di scoperta non leggibile: {error}"))?;
    remove_owned(endpoint, token, record.socket_inode, record.socket_dev)
}

fn remove_owned(
    endpoint: &Endpoint,
    token: &Token,
    socket_inode: u64,
    socket_dev: u64,
) -> Result<(), String> {
    let record = read_record(&endpoint.record_path(), endpoint.dir())
        .map_err(|error| format!("record di scoperta non leggibile: {error}"))?;
    if record.id != endpoint.identity().to_string() || record.token != *token {
        return Ok(());
    }
    if record.socket_inode != socket_inode || record.socket_dev != socket_dev {
        return Ok(());
    }
    std::fs::remove_file(endpoint.record_path()).map_err(|error| {
        format!(
            "record di scoperta non rimosso in {}: {error}",
            endpoint.record_path().display()
        )
    })?;
    let socket = endpoint.socket_path();
    match std::fs::symlink_metadata(&socket) {
        Ok(metadata) if metadata.dev() == socket_dev && metadata.ino() == socket_inode => {
            std::fs::remove_file(&socket)
                .map_err(|error| format!("socket non rimosso in {}: {error}", socket.display()))?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "socket non verificabile in {}: {error}",
                socket.display()
            ));
        }
    }
    Ok(())
}

/// Bind esclusivo, permessi 0600 verificati, poi pubblicazione del record.
/// Prima di creare il nuovo endpoint pota una volta sola i record stantii
/// della cartella, così un processo caduto non consuma il tetto delle
/// istanze. Il listener appartiene al chiamante; la guardia a record e socket;
/// le diagnostiche della potatura vanno mostrate all'utente.
pub async fn bind(
    endpoint: &Endpoint,
    token: &Token,
    root: &Path,
    version: &str,
) -> Result<
    (
        tokio::net::UnixListener,
        DiscoveryFile,
        Vec<DiscoveryNotice>,
    ),
    String,
> {
    endpoint.check_socket_path()?;
    ensure_private_dir(endpoint.dir()).map_err(|error| error.to_string())?;
    let notices = prune_stale_records(endpoint.dir()).await;
    let socket = endpoint.socket_path();
    let listener = tokio::net::UnixListener::bind(&socket).map_err(|error| {
        format!(
            "il socket {} non si crea (esiste già?): {error}",
            socket.display()
        )
    })?;
    if let Err(error) = std::fs::set_permissions(&socket, Permissions::from_mode(SOCKET_MODE)) {
        drop(listener);
        let _ = std::fs::remove_file(&socket);
        return Err(format!(
            "permessi del socket {} non impostati: {error}",
            socket.display()
        ));
    }
    let metadata = match std::fs::symlink_metadata(&socket) {
        Ok(metadata) => metadata,
        Err(error) => {
            drop(listener);
            let _ = std::fs::remove_file(&socket);
            return Err(format!(
                "socket {} non verificabile: {error}",
                socket.display()
            ));
        }
    };
    let euid = effective_uid();
    if !metadata.file_type().is_socket()
        || metadata.uid() != euid
        || metadata.mode() & 0o7777 != SOCKET_MODE
    {
        drop(listener);
        let _ = std::fs::remove_file(&socket);
        return Err(format!(
            "socket {} non sicuro: atteso socket 0600 dell'UID {euid}",
            socket.display()
        ));
    }
    let record = Record::new(
        endpoint,
        root.to_path_buf(),
        token.clone(),
        version.to_owned(),
        &metadata,
    );
    match DiscoveryFile::publish(endpoint, &record) {
        Ok(guard) => Ok((listener, guard, notices)),
        Err(error) => {
            drop(listener);
            let _ = std::fs::remove_file(&socket);
            Err(error)
        }
    }
}

/// UID del peer dal kernel. Un errore o un UID diverso vanno rifiutati.
pub fn peer_uid(stream: &UnixStream) -> Result<u32, String> {
    let credentials = stream
        .peer_cred()
        .map_err(|error| format!("credenziali del peer non disponibili: {error}"))?;
    Ok(credentials.uid())
}

/// Verifica kernel per il ponte: UID effettivo e PID del peer contro il
/// record. Un PID assente è un rifiuto, non una tolleranza.
pub fn verify_peer(stream: &UnixStream, record: &Record) -> Result<(), String> {
    let credentials = stream
        .peer_cred()
        .map_err(|error| format!("credenziali del peer non disponibili: {error}"))?;
    let euid = effective_uid();
    if credentials.uid() != euid {
        return Err(format!(
            "il peer del socket {} appartiene all'UID {}, non all'UID effettivo {euid}",
            record.socket.display(),
            credentials.uid()
        ));
    }
    let pid = credentials
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or_else(|| {
            format!(
                "il kernel non riporta il PID del peer del socket {}",
                record.socket.display()
            )
        })?;
    if pid != record.pid {
        return Err(format!(
            "il processo dietro il socket {} ha PID {pid}, non {}: il record non descrive più quell'istanza",
            record.socket.display(),
            record.pid
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectError {
    #[error("nessun Lume con il server MCP acceso: avvialo da Estensioni → Server MCP")]
    NoInstance,
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    #[error("nessun Lume con il server MCP acceso ha aperto {}", .0.display())]
    NoMatch(PathBuf),
    #[error("l'istanza «{0}» non è fra quelle accese")]
    UnknownInstance(String),
    #[error("più istanze di Lume corrispondono a questo progetto ({}): scegli con --instance <id> o --project <cartella>", elenco(.0))]
    Ambiguous(Vec<String>),
    #[error("nessuna delle istanze è raggiungibile ({}): non è stata scelta nessuna", elenco(.0))]
    Unreachable(Vec<String>),
    #[error("scoperta non sicura: {0}")]
    Unsafe(String),
    #[error("scoperta incoerente: {0}")]
    Inconsistent(String),
    #[error("la coda del socket {} è occupata: riprova quando una connessione si libera", .0.display())]
    Busy(PathBuf),
    #[error("nessuna risposta entro {0:?}: il tentativo va ripetuto")]
    Deadline(Duration),
    #[error("connessione al socket fallita: {0}")]
    Io(String),
}

fn elenco(ids: &[String]) -> String {
    ids.join("; ")
}

/// Messaggio utente per una selezione fallita: percorso della cartella di
/// scoperta e rimedio, oltre al motivo specifico.
pub fn failure_message(error: &SelectError, dir: &Path) -> String {
    format!(
        "{error}; cartella di scoperta {DIR_NAME}: {}; rimedio: avvia o riavvia il server MCP da Estensioni → Server MCP; se il problema resta, esamina o rimuovi i file <id>.json e <id>.sock di quell'istanza",
        dir.display()
    )
}

#[derive(Debug)]
pub struct Selected {
    pub record: Record,
    pub stream: UnixStream,
}

/// Esito della selezione con le diagnostiche sui record saltati. Il ponte
/// stampa le diagnostiche su stderr anche quando la selezione riesce.
#[derive(Debug)]
pub struct SelectionReport {
    pub result: Result<Selected, SelectError>,
    pub notices: Vec<DiscoveryNotice>,
}

/// Sceglie l'istanza con `--instance` o per progetto/cwd, verifica i
/// candidati con connessioni reali e restituisce **una nuova** connessione
/// già rivalidata. Nessun byte del client viene letto qui e nessun token
/// viene mandato ai candidati esclusi.
pub async fn select(
    dir: &Path,
    project: Option<&Path>,
    cwd: Option<&Path>,
    instance: Option<&str>,
) -> Result<Selected, SelectError> {
    select_report(dir, project, cwd, instance).await.result
}

/// Come `select`, ma restituisce anche i record invalidi saltati e le voci
/// oltre i limiti, con il percorso del file.
pub async fn select_report(
    dir: &Path,
    project: Option<&Path>,
    cwd: Option<&Path>,
    instance: Option<&str>,
) -> SelectionReport {
    select_until_report(dir, project, cwd, instance, None).await
}

/// Variante con deadline esplicita: solo le prove la usano per rappresentare
/// un selettore sospeso e ripreso dopo la scadenza. La produzione passa
/// `None` e la deadline parte prima della scansione, che è bounded.
async fn select_until_report(
    dir: &Path,
    project: Option<&Path>,
    cwd: Option<&Path>,
    instance: Option<&str>,
    deadline: Option<Instant>,
) -> SelectionReport {
    let mut notices = Vec::new();
    let result = select_inner(dir, project, cwd, instance, deadline, &mut notices).await;
    SelectionReport { result, notices }
}

async fn select_inner(
    dir: &Path,
    project: Option<&Path>,
    cwd: Option<&Path>,
    instance: Option<&str>,
    deadline: Option<Instant>,
    notices: &mut Vec<DiscoveryNotice>,
) -> Result<Selected, SelectError> {
    match std::fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(SelectError::Unsafe(format!(
                "{} non è una cartella",
                dir.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SelectError::NoInstance);
        }
        Err(error) => {
            return Err(SelectError::Io(format!("{}: {error}", dir.display())));
        }
    }
    verify_existing_dir(dir)?;
    let deadline = deadline.unwrap_or_else(|| Instant::now() + CONNECT_DEADLINE);
    if Instant::now() >= deadline {
        return Err(SelectError::Deadline(CONNECT_DEADLINE));
    }
    let scan = scan_records(dir, deadline).await?;
    notices.extend(scan.notices);
    let candidates = candidates(scan.records, project, cwd, instance)?;
    if Instant::now() >= deadline {
        return Err(SelectError::Deadline(CONNECT_DEADLINE));
    }
    let mut reachable = Vec::new();
    for record in candidates {
        if Instant::now() >= deadline {
            return Err(SelectError::Deadline(CONNECT_DEADLINE));
        }
        match probe(&record, deadline).await? {
            Probe::Reachable => reachable.push(record),
            Probe::Dead => {
                if let Err(reason) = remove_unreachable(dir, &record) {
                    notices.push(DiscoveryNotice {
                        path: record_path(dir, &record.id),
                        reason,
                    });
                }
            }
            // Un candidato che non descrive più il suo endpoint (inode/dev o
            // tipo diversi, PID del peer diverso) viene escluso con un avviso:
            // non fa fallire una selezione che ha altre istanze sane e non
            // viene cancellato.
            Probe::Inconsistent(reason) => notices.push(DiscoveryNotice {
                path: record_path(dir, &record.id),
                reason,
            }),
        }
    }
    let record = match reachable.len() {
        0 => {
            let ids = candidates_fallback(dir, project, cwd, instance, notices)?;
            return Err(SelectError::Unreachable(ids));
        }
        1 => match reachable.pop() {
            Some(record) => record,
            None => return Err(SelectError::NoInstance),
        },
        _ => {
            let roots: Vec<String> = reachable
                .iter()
                .map(|record| format!("{} ({})", record.id, record.root.display()))
                .collect();
            return Err(SelectError::Ambiguous(roots));
        }
    };
    if Instant::now() >= deadline {
        return Err(SelectError::Deadline(CONNECT_DEADLINE));
    }
    // La scelta viene rivalidata prima e dopo la connessione finale. Se il
    // record o l'endpoint cambiano, l'errore chiede un nuovo tentativo e non
    // ripiega su un'altra istanza né riusa lo stream appena aperto.
    let current = read_record(&record_path(dir, &record.id), dir)?;
    if current != record {
        return Err(SelectError::Inconsistent(format!(
            "il record dell'istanza {} è cambiato durante la scelta: ripeti il tentativo",
            record.id
        )));
    }
    checked_socket(&current)?;
    let stream = match timeout_at(deadline, UnixStream::connect(&current.socket)).await {
        Err(_) => return Err(SelectError::Deadline(CONNECT_DEADLINE)),
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            // Il candidato era raggiungibile nella prova ed è caduto prima
            // della connessione finale: stessa potatura con ricontrollo,
            // nessun ripiego su un'altra istanza.
            if let Err(reason) = remove_unreachable(dir, &current) {
                notices.push(DiscoveryNotice {
                    path: record_path(dir, &current.id),
                    reason,
                });
            }
            return Err(SelectError::Unreachable(vec![current.id.clone()]));
        }
        Ok(Err(error))
            if error.kind() == std::io::ErrorKind::ConnectionRefused && REFUSED_IS_CERTAIN =>
        {
            // Vedi `REFUSED_IS_CERTAIN`: solo dove è il segnale certo di un
            // listener sparito.
            if let Err(reason) = remove_unreachable(dir, &current) {
                notices.push(DiscoveryNotice {
                    path: record_path(dir, &current.id),
                    reason,
                });
            }
            return Err(SelectError::Unreachable(vec![current.id.clone()]));
        }
        Ok(Err(error))
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            // Coda piena (su BSD/macOS la `connect` riporta `ECONNREFUSED`
            // anche in questo caso): il candidato non viene potato.
            return Err(SelectError::Busy(current.socket.clone()));
        }
        Ok(Err(error)) => {
            return Err(SelectError::Io(format!(
                "{}: {error}",
                current.socket.display()
            )));
        }
    };
    let after = read_record(&record_path(dir, &current.id), dir)?;
    if after != current {
        return Err(SelectError::Inconsistent(format!(
            "il record dell'istanza {} è cambiato mentre si apriva il socket: ripeti il tentativo",
            current.id
        )));
    }
    checked_socket(&after)?;
    verify_peer(&stream, &after).map_err(SelectError::Inconsistent)?;
    Ok(Selected {
        record: after,
        stream,
    })
}

/// Verifica tipo, proprietario, permessi e inode/dev del socket contro il
/// record. Un path che non è più lo stesso endpoint è un errore, non un
/// candidato assente.
fn checked_socket(record: &Record) -> Result<Metadata, SelectError> {
    let metadata = std::fs::symlink_metadata(&record.socket).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SelectError::Inconsistent(format!(
                "il socket {} non esiste più: ripeti il tentativo",
                record.socket.display()
            ))
        } else {
            SelectError::Io(format!("{}: {error}", record.socket.display()))
        }
    })?;
    check_socket_metadata(record, &metadata)?;
    Ok(metadata)
}

fn check_socket_metadata(record: &Record, metadata: &Metadata) -> Result<(), SelectError> {
    if !metadata.file_type().is_socket() {
        return Err(SelectError::Inconsistent(format!(
            "{} non è un socket",
            record.socket.display()
        )));
    }
    let euid = effective_uid();
    if metadata.uid() != euid {
        return Err(SelectError::Unsafe(format!(
            "il socket {} appartiene all'UID {}, non all'UID effettivo {euid}",
            record.socket.display(),
            metadata.uid()
        )));
    }
    if metadata.mode() & 0o7777 != SOCKET_MODE {
        return Err(SelectError::Unsafe(format!(
            "il socket {} ha permessi {:04o}, non 0600",
            record.socket.display(),
            metadata.mode() & 0o7777
        )));
    }
    if metadata.ino() != record.socket_inode || metadata.dev() != record.socket_dev {
        return Err(SelectError::Inconsistent(format!(
            "il socket {} non è l'endpoint del record (inode {}:{}, atteso {}:{})",
            record.socket.display(),
            metadata.dev(),
            metadata.ino(),
            record.socket_dev,
            record.socket_inode
        )));
    }
    Ok(())
}

fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn candidates(
    records: Vec<Record>,
    project: Option<&Path>,
    cwd: Option<&Path>,
    instance: Option<&str>,
) -> Result<Vec<Record>, SelectError> {
    if let Some(instance) = instance {
        let mut found = records.into_iter().filter(|record| record.id == instance);
        let record = found
            .next()
            .ok_or_else(|| SelectError::UnknownInstance(instance.to_owned()))?;
        if found.next().is_some() {
            return Err(SelectError::Inconsistent(format!(
                "più record dichiarano l'identità {instance}"
            )));
        }
        if let Some(project) = project
            && !project.starts_with(&record.root)
        {
            return Err(SelectError::NoMatch(project.to_path_buf()));
        }
        return Ok(vec![record]);
    }
    if let Some(project) = project {
        let found: Vec<Record> = records
            .into_iter()
            .filter(|record| project.starts_with(&record.root))
            .collect();
        if found.is_empty() {
            return Err(SelectError::NoMatch(project.to_path_buf()));
        }
        return Ok(found);
    }
    if let Some(cwd) = cwd {
        let found: Vec<Record> = records
            .iter()
            .filter(|record| cwd.starts_with(&record.root))
            .cloned()
            .collect();
        if !found.is_empty() {
            return Ok(found);
        }
    }
    if records.is_empty() {
        return Err(SelectError::NoInstance);
    }
    Ok(records)
}

fn candidates_fallback(
    dir: &Path,
    project: Option<&Path>,
    cwd: Option<&Path>,
    instance: Option<&str>,
    notices: &mut Vec<DiscoveryNotice>,
) -> Result<Vec<String>, SelectError> {
    let scan = read_records(dir)?;
    notices.extend(scan.notices);
    let candidates = candidates(scan.records, project, cwd, instance)?;
    Ok(candidates.into_iter().map(|record| record.id).collect())
}

/// Esito di una connessione di prova: nessun token viene mandato e la
/// connessione si chiude subito. Solo `NotFound`/`ConnectionRefused` (dove
/// certo) rendono il candidato irraggiungibile; un record che non descrive più
/// l'endpoint viene escluso con un avviso, ogni altra incongruenza resta un
/// errore esplicito.
enum Probe {
    Reachable,
    Dead,
    Inconsistent(String),
}

async fn probe(record: &Record, deadline: Instant) -> Result<Probe, SelectError> {
    match std::fs::symlink_metadata(&record.socket) {
        Ok(metadata) => match check_socket_metadata(record, &metadata) {
            Ok(()) => {}
            Err(SelectError::Inconsistent(reason)) => return Ok(Probe::Inconsistent(reason)),
            Err(error) => return Err(error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Probe::Dead),
        Err(error) => {
            return Err(SelectError::Io(format!(
                "{}: {error}",
                record.socket.display()
            )));
        }
    }
    let stream = match timeout_at(deadline, UnixStream::connect(&record.socket)).await {
        Err(_) => return Err(SelectError::Deadline(CONNECT_DEADLINE)),
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Probe::Dead);
        }
        Ok(Err(error))
            if error.kind() == std::io::ErrorKind::ConnectionRefused && REFUSED_IS_CERTAIN =>
        {
            return Ok(Probe::Dead);
        }
        Ok(Err(error))
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            return Err(SelectError::Busy(record.socket.clone()));
        }
        Ok(Err(error)) => {
            return Err(SelectError::Io(format!(
                "{}: {error}",
                record.socket.display()
            )));
        }
    };
    if let Err(reason) = verify_peer(&stream, record) {
        // Il socket è vivo ma non è l'istanza che il record descrive (PID
        // riusato, altro endpoint): si esclude, non si cancella.
        return Ok(Probe::Inconsistent(reason));
    }
    drop(stream);
    Ok(Probe::Reachable)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn dati_privati() -> tempfile::TempDir {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("HOME serve per una cartella dati privata");
        tempfile::Builder::new()
            .prefix("lume-mcp-prova-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(home)
            .expect("cartella privata di prova")
    }

    #[test]
    fn il_messaggio_di_selezione_fallita_nomina_cartella_e_rimedio() {
        let dir = PathBuf::from("/dati/lume/mcp-unix-v1");
        let messaggio = failure_message(&SelectError::NoInstance, &dir);
        assert!(messaggio.contains("mcp-unix-v1"), "{messaggio}");
        assert!(messaggio.contains("/dati/lume/mcp-unix-v1"), "{messaggio}");
        assert!(messaggio.contains("rimedio"), "{messaggio}");
    }

    #[test]
    fn l_identita_e_pid_e_nonce_esadecimale() {
        let identita = Identity::generate().unwrap();
        assert_eq!(identita.pid(), std::process::id());
        let testo = identita.to_string();
        assert_eq!(Identity::parse(&testo), Some(identita));
        assert!(Identity::parse("42-zzzzzzzz").is_none());
        assert!(Identity::parse("42-corto").is_none());
        assert!(Identity::parse("42").is_none());
    }

    #[test]
    fn la_regola_dell_owner_rifiuta_un_uid_diverso() {
        assert!(owner_allowed(1000, 1000));
        assert!(owner_allowed(0, 1000));
        assert!(!owner_allowed(1001, 1000));
    }

    #[test]
    fn un_endpoint_relativo_e_rifiutato_prima_di_toccare_il_disco() {
        let errore = Endpoint::with_identity(
            PathBuf::from("dati/mcp-unix-v1"),
            Identity::generate().unwrap(),
        )
        .unwrap_err();
        assert!(errore.contains("non è assoluta"), "{errore}");
    }

    #[tokio::test]
    async fn il_pid_del_peer_deve_corrispondere_al_record() {
        let (stream, _altro) = UnixStream::pair().unwrap();
        let record = Record {
            schema: SCHEMA.to_owned(),
            id: format!("{}-00000000", std::process::id().wrapping_add(1)),
            pid: std::process::id().wrapping_add(1),
            socket: PathBuf::from("/socket/di/prova.sock"),
            socket_inode: 1,
            socket_dev: 1,
            root: PathBuf::from("/"),
            token: Token::generate().unwrap(),
            version: "0.0.0-prova".to_owned(),
            started_at: 0,
        };
        let errore = verify_peer(&stream, &record).unwrap_err();
        assert!(errore.contains("PID"), "{errore}");
    }

    #[test]
    fn la_cartella_nasce_0700_e_viene_verificata() {
        let dati = dati_privati();
        let dir = discovery_dir(dati.path());
        ensure_private_dir(&dir).unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&dir).unwrap().mode() & 0o7777,
            0o700
        );
        ensure_private_dir(&dir).unwrap();
    }

    #[test]
    fn una_cartella_con_permessi_larghi_non_viene_corretta() {
        let dati = dati_privati();
        let dir = discovery_dir(dati.path());
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, Permissions::from_mode(0o755)).unwrap();
        let errore = ensure_private_dir(&dir).unwrap_err().to_string();
        assert!(errore.contains("0755"), "{errore}");
        assert_eq!(
            std::fs::symlink_metadata(&dir).unwrap().mode() & 0o7777,
            0o755,
            "nessun chmod implicito"
        );
    }

    #[test]
    fn un_antenato_scrivibile_da_altri_rifiuta_la_cartella() {
        let dati = dati_privati();
        let pubblico = dati.path().join("pubblico");
        std::fs::create_dir(&pubblico).unwrap();
        std::fs::set_permissions(&pubblico, Permissions::from_mode(0o777)).unwrap();
        let dir = discovery_dir(&pubblico);
        let errore = ensure_private_dir(&dir).unwrap_err().to_string();
        assert!(errore.contains("scrivibile"), "{errore}");
        assert!(!dir.exists());
    }

    #[test]
    fn un_link_simbolico_nella_catena_rifiuta_la_cartella() {
        let dati = dati_privati();
        let reale = dati.path().join("reale");
        std::fs::create_dir(&reale).unwrap();
        let link = dati.path().join("link");
        std::os::unix::fs::symlink(&reale, &link).unwrap();
        let errore = ensure_private_dir(&discovery_dir(&link))
            .unwrap_err()
            .to_string();
        assert!(errore.contains("link simbolico"), "{errore}");
    }

    #[test]
    fn un_record_ha_identita_token_e_inode_e_non_svela_il_token() {
        let dati = dati_privati();
        let dir = discovery_dir(dati.path());
        ensure_private_dir(&dir).unwrap();
        let endpoint = Endpoint::with_identity(dir.clone(), Identity::generate().unwrap()).unwrap();
        let token = Token::generate().unwrap();
        let record = Record::new(
            &endpoint,
            dati.path().to_path_buf(),
            token.clone(),
            "0.0.0-prova".to_owned(),
            &std::fs::symlink_metadata(dati.path()).unwrap(),
        );
        let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();
        let letto = read_records(&dir).unwrap();
        assert_eq!(letto.records, vec![record.clone()]);
        assert!(letto.notices.is_empty(), "{:?}", letto.notices);
        assert!(!format!("{letto:?}").contains(&token.to_hex()));
        drop(guard);
        assert!(!endpoint.record_path().exists());
    }

    /// Un selettore ripreso dopo oltre cinque secondi non riusa un record
    /// scaduto: fallisce per deadline e lascia il candidato dov'è.
    #[tokio::test]
    async fn una_scelta_ripresa_dopo_la_scadenza_fallisce_senza_rimuovere_i_candidati() {
        let dati = dati_privati();
        let dir = discovery_dir(dati.path());
        ensure_private_dir(&dir).unwrap();
        let endpoint = Endpoint::with_identity(dir.clone(), Identity::generate().unwrap()).unwrap();
        let record = Record::new(
            &endpoint,
            dati.path().to_path_buf(),
            Token::generate().unwrap(),
            "0.0.0-prova".to_owned(),
            &std::fs::symlink_metadata(dati.path()).unwrap(),
        );
        let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();
        let deadline = Instant::now() + CONNECT_DEADLINE;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let report = select_until_report(&dir, None, None, None, Some(deadline)).await;
        assert!(
            matches!(report.result, Err(SelectError::Deadline(_))),
            "{:?}",
            report.result
        );
        assert!(
            endpoint.record_path().exists(),
            "il candidato non viene rimosso da una scelta scaduta"
        );
        drop(guard);
    }

    #[tokio::test]
    async fn un_record_oltre_il_tetto_viene_saltato_con_diagnostica() {
        let dati = dati_privati();
        let dir = discovery_dir(dati.path());
        ensure_private_dir(&dir).unwrap();
        let path = dir.join("1-00000000.json");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        let mut file = file;
        file.write_all(&vec![b'x'; MAX_RECORD_BYTES as usize + 1])
            .unwrap();
        let scan = scan_records(&dir, Instant::now() + CONNECT_DEADLINE)
            .await
            .unwrap();
        assert!(scan.records.is_empty());
        assert_eq!(scan.notices.len(), 1);
        assert_eq!(scan.notices[0].path, path);
        assert!(
            scan.notices[0].reason.contains("byte"),
            "{:?}",
            scan.notices
        );
        assert!(path.exists(), "un record illeggibile non viene cancellato");
        assert!(read_records(&dir).unwrap().records.is_empty());
    }

    /// Anche la lettura senza connessioni (`read_records`) non perde in
    /// silenzio i file che salta: il percorso finisce nelle diagnostiche.
    #[test]
    fn la_lettura_senza_connessioni_riporta_i_file_saltati() {
        let dati = dati_privati();
        let dir = discovery_dir(dati.path());
        ensure_private_dir(&dir).unwrap();
        let path = dir.join("1-00000000.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(b"{non-json").unwrap();
        let scan = read_records(&dir).unwrap();
        assert!(scan.records.is_empty());
        assert_eq!(scan.notices.len(), 1, "{:?}", scan.notices);
        assert_eq!(scan.notices[0].path, path);
        assert!(!scan.notices[0].reason.is_empty(), "{:?}", scan.notices);
        assert!(path.exists(), "la lettura non cancella mai un record");
    }

    #[tokio::test]
    async fn un_record_con_schema_di_pipe_non_viene_interpretato() {
        let dati = dati_privati();
        let dir = discovery_dir(dati.path());
        ensure_private_dir(&dir).unwrap();
        let path = dir.join("1-00000000.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(br#"{"pid":1,"pipe":"\\\\.\\pipe\\x"}"#)
            .unwrap();
        let scan = scan_records(&dir, Instant::now() + CONNECT_DEADLINE)
            .await
            .unwrap();
        assert!(scan.records.is_empty());
        assert_eq!(scan.notices.len(), 1);
        assert_eq!(scan.notices[0].path, path);
        assert!(
            !scan.notices[0].reason.is_empty(),
            "la diagnostica spiega il motivo"
        );
        assert!(read_records(&dir).unwrap().records.is_empty());
    }

    #[test]
    fn il_cleanup_rimuove_solo_cio_che_corrisponde_ancora() {
        let dati = dati_privati();
        let dir = discovery_dir(dati.path());
        ensure_private_dir(&dir).unwrap();
        let endpoint = Endpoint::with_identity(dir.clone(), Identity::generate().unwrap()).unwrap();
        let token = Token::generate().unwrap();
        let record = Record::new(
            &endpoint,
            dati.path().to_path_buf(),
            token.clone(),
            "0.0.0-prova".to_owned(),
            &std::fs::symlink_metadata(dati.path()).unwrap(),
        );
        let guard = DiscoveryFile::publish(&endpoint, &record).unwrap();
        std::mem::forget(guard);
        assert!(cleanup(&endpoint, &Token::generate().unwrap()).is_ok());
        assert!(endpoint.record_path().exists(), "token diverso: resta");
        assert!(cleanup(&endpoint, &token).is_ok());
        assert!(!endpoint.record_path().exists());
    }
}
