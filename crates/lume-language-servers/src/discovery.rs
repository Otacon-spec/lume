use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use lume_lsp::{CommandSource, ServerCommand};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use wait_timeout::ChildExt;

use crate::{Platform, ServerSpec};

const VERSION_OUTPUT_CAPACITY: usize = 64 * 1024;
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Il testo dell'errore sta in `error`, per i log. Non si mostra all'utente
/// così com'è: contiene percorsi e, per gli errori di I/O, il testo del sistema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryAttempt {
    pub source: CommandSource,
    pub path: PathBuf,
    pub error: Option<String>,
    /// Il motivo, classificato. `None` per il candidato riuscito.
    pub failure: Option<AttemptFailure>,
}

/// Perché un candidato è stato scartato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptFailure {
    /// Il file non c'è.
    NotFound,
    /// Il percorso esiste ma non è un file.
    NotAFile,
    /// La ricevuta d'installazione manca o non si legge.
    InvalidReceipt,
    /// Il file non è quello che la ricevuta ha registrato.
    HashMismatch,
    /// `--version` è terminato con un errore.
    ProbeFailed,
    /// `--version` non ha risposto entro il tempo limite.
    ProbeTimeout,
    /// `--version` ha stampato più di 64 KiB.
    ProbeOutputTooLarge,
    /// Errore di I/O diverso da «non c'è» (permessi, percorso illeggibile).
    Unreadable,
}

impl AttemptFailure {
    fn of(error: &DiscoveryError) -> Self {
        match error {
            DiscoveryError::NotFound(_) => Self::NotFound,
            DiscoveryError::NotFile(_) => Self::NotAFile,
            DiscoveryError::InvalidReceipt(_) => Self::InvalidReceipt,
            DiscoveryError::HashMismatch(_) => Self::HashMismatch,
            DiscoveryError::Probe(..) => Self::ProbeFailed,
            DiscoveryError::ProbeTimeout(_) => Self::ProbeTimeout,
            DiscoveryError::ProbeOutputTooLarge(_) => Self::ProbeOutputTooLarge,
            // `Unavailable` è l'esito finale, non lo produce nessun candidato.
            DiscoveryError::Io(..) | DiscoveryError::Unavailable { .. } => Self::Unreadable,
        }
    }

    const fn describe(self) -> &'static str {
        match self {
            Self::NotFound => "non trovato",
            Self::NotAFile => "non è un file",
            Self::InvalidReceipt => "ricevuta di installazione mancante o non valida",
            Self::HashMismatch => "il file non corrisponde alla ricevuta",
            Self::ProbeFailed => "non parte (--version fallisce)",
            Self::ProbeTimeout => "non risponde a --version entro il tempo limite",
            Self::ProbeOutputTooLarge => "--version stampa troppo (oltre 64 KiB)",
            Self::Unreadable => "non leggibile",
        }
    }
}

const fn origin_label(source: CommandSource) -> &'static str {
    match source {
        CommandSource::Bundled => "Incluso con Lume",
        CommandSource::Managed => "Scaricato da Lume",
        CommandSource::SystemPath => "PATH",
        CommandSource::Workspace => "Progetto",
    }
}

/// Una riga per origine, in italiano, per il pannello Estensioni. Un motivo
/// diverso da «non trovato» vince su «non trovato»: se il binario del `PATH`
/// esiste ma non parte, dirlo è più utile che contare le cartelle vuote.
/// **Nessun percorso e nessun testo grezzo degli errori**: il pannello si
/// mostra nelle richieste d'aiuto.
pub fn describe_unavailable(attempts: &[DiscoveryAttempt]) -> String {
    let mut lines = Vec::new();
    for source in [
        CommandSource::Bundled,
        CommandSource::Managed,
        CommandSource::SystemPath,
        CommandSource::Workspace,
    ] {
        let of_source: Vec<&DiscoveryAttempt> = attempts
            .iter()
            .filter(|attempt| attempt.source == source)
            .collect();
        if of_source.is_empty() {
            continue;
        }
        let unusual = of_source
            .iter()
            .filter_map(|attempt| attempt.failure)
            .find(|failure| *failure != AttemptFailure::NotFound);
        let reason = match unusual {
            Some(failure) => failure.describe().to_owned(),
            None if of_source.len() > 1 => {
                format!("non trovato in {} cartelle", of_source.len())
            }
            None => AttemptFailure::NotFound.describe().to_owned(),
        };
        lines.push(format!("{}: {reason}", origin_label(source)));
    }
    if lines.is_empty() {
        "nessun percorso di ricerca disponibile".to_owned()
    } else {
        lines.join("\n")
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedServer {
    pub command: ServerCommand,
    pub version: String,
    pub attempts: Vec<DiscoveryAttempt>,
}

pub struct ServerLocator {
    bundle_root: Option<PathBuf>,
    managed_root: PathBuf,
    search_path: Vec<PathBuf>,
    probe_timeout: Duration,
}

impl ServerLocator {
    pub fn new(
        bundle_root: Option<PathBuf>,
        managed_root: PathBuf,
        search_path: Vec<PathBuf>,
    ) -> Self {
        Self {
            bundle_root,
            managed_root,
            search_path,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
        }
    }

    pub fn from_environment(bundle_root: Option<PathBuf>, managed_root: PathBuf) -> Self {
        let search_path = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        Self::new(bundle_root, managed_root, search_path)
    }

    #[doc(hidden)]
    pub fn with_probe_timeout(mut self, timeout: Duration) -> Self {
        self.probe_timeout = timeout;
        self
    }

    pub fn resolve(
        &self,
        server: &ServerSpec,
        platform: &Platform,
    ) -> Result<ResolvedServer, DiscoveryError> {
        let executable = executable_name(&server.executable, &platform.os);
        let relative = PathBuf::from(&server.id)
            .join(&server.release.version)
            .join(platform_key(platform));
        let mut candidates = Vec::new();
        if let Some(root) = &self.bundle_root {
            candidates.push((
                CommandSource::Bundled,
                root.join(&relative).join(&executable),
                true,
            ));
        }
        candidates.push((
            CommandSource::Managed,
            self.managed_root.join(&relative).join(&executable),
            true,
        ));
        for directory in &self.search_path {
            if directory.as_os_str().is_empty() || !directory.is_absolute() {
                continue;
            }
            candidates.push((
                CommandSource::SystemPath,
                directory.join(&executable),
                false,
            ));
        }

        let mut attempts = Vec::new();
        for (source, path, verify) in candidates {
            let result = self.validate_candidate(server, platform, &path, verify);
            match result {
                Ok((canonical, version)) => {
                    attempts.push(DiscoveryAttempt {
                        source,
                        path: canonical.clone(),
                        error: None,
                        failure: None,
                    });
                    return Ok(ResolvedServer {
                        command: ServerCommand {
                            program: canonical,
                            args: server.args.iter().map(OsString::from).collect(),
                            cwd: None,
                            env: Vec::new(),
                            source,
                        },
                        version,
                        attempts,
                    });
                }
                Err(error) => attempts.push(DiscoveryAttempt {
                    source,
                    path,
                    failure: Some(AttemptFailure::of(&error)),
                    error: Some(error.to_string()),
                }),
            }
        }
        Err(DiscoveryError::Unavailable {
            server: server.id.clone(),
            attempts,
        })
    }

    fn validate_candidate(
        &self,
        server: &ServerSpec,
        platform: &Platform,
        path: &Path,
        verify: bool,
    ) -> Result<(PathBuf, String), DiscoveryError> {
        let canonical = path.canonicalize().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                DiscoveryError::NotFound(path.to_path_buf())
            } else {
                DiscoveryError::Io(path.to_path_buf(), error.to_string())
            }
        })?;
        if !canonical.is_file() {
            return Err(DiscoveryError::NotFile(canonical));
        }
        if verify {
            verify_receipt(server, platform, &canonical)?;
        }
        let version = probe_version(&canonical, self.probe_timeout)?;
        Ok((canonical, version))
    }
}

fn executable_name(base: &str, os: &str) -> String {
    if os == "windows" {
        format!("{base}.exe")
    } else {
        base.to_owned()
    }
}

fn platform_key(platform: &Platform) -> String {
    format!("{}-{}", platform.os, platform.arch)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    server_id: String,
    version: String,
    os: String,
    arch: String,
    binary: String,
    sha256: String,
}

fn verify_receipt(
    server: &ServerSpec,
    platform: &Platform,
    binary: &Path,
) -> Result<(), DiscoveryError> {
    let receipt_path = binary
        .parent()
        .ok_or_else(|| DiscoveryError::InvalidReceipt(binary.to_path_buf()))?
        .join("receipt.json");
    let source = std::fs::read(&receipt_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            DiscoveryError::InvalidReceipt(receipt_path.clone())
        } else {
            DiscoveryError::Io(receipt_path.clone(), error.to_string())
        }
    })?;
    if source.len() > 64 * 1024 {
        return Err(DiscoveryError::InvalidReceipt(receipt_path));
    }
    let receipt: Receipt = serde_json::from_slice(&source)
        .map_err(|_| DiscoveryError::InvalidReceipt(receipt_path.clone()))?;
    let file_name = binary
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| DiscoveryError::InvalidReceipt(receipt_path.clone()))?;
    if receipt.server_id != server.id
        || receipt.version != server.release.version
        || receipt.os != platform.os
        || receipt.arch != platform.arch
        || receipt.binary != file_name
        || receipt.sha256.len() != 64
    {
        return Err(DiscoveryError::InvalidReceipt(receipt_path));
    }
    let actual = hash_file(binary)?;
    if !actual.eq_ignore_ascii_case(&receipt.sha256) {
        return Err(DiscoveryError::HashMismatch(binary.to_path_buf()));
    }
    Ok(())
}

#[allow(clippy::indexing_slicing)] // read <= buffer.len() (contratto di Read)
fn hash_file(path: &Path) -> Result<String, DiscoveryError> {
    let mut file = File::open(path)
        .map_err(|error| DiscoveryError::Io(path.to_path_buf(), error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| DiscoveryError::Io(path.to_path_buf(), error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn probe_version(path: &Path, timeout: Duration) -> Result<String, DiscoveryError> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| DiscoveryError::Probe(path.to_path_buf(), error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| DiscoveryError::Probe(path.to_path_buf(), "stdout unavailable".into()))?;
    let reader = thread::spawn(move || bounded_read(stdout));
    let status = child
        .wait_timeout(timeout)
        .map_err(|error| DiscoveryError::Probe(path.to_path_buf(), error.to_string()))?;
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return Err(DiscoveryError::ProbeTimeout(path.to_path_buf()));
    }
    let output = reader
        .join()
        .map_err(|_| DiscoveryError::Probe(path.to_path_buf(), "reader panicked".into()))?
        .map_err(|error| DiscoveryError::Probe(path.to_path_buf(), error.to_string()))?;
    if output.truncated {
        return Err(DiscoveryError::ProbeOutputTooLarge(path.to_path_buf()));
    }
    let version = String::from_utf8(output.bytes)
        .map_err(|_| DiscoveryError::Probe(path.to_path_buf(), "version is not UTF-8".into()))?;
    let version = version.trim();
    if version.is_empty() {
        return Err(DiscoveryError::Probe(
            path.to_path_buf(),
            "empty version".into(),
        ));
    }
    Ok(version.to_owned())
}

struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

#[allow(clippy::indexing_slicing)] // read <= buffer.len() (contratto di Read)
fn bounded_read(mut reader: impl Read) -> io::Result<BoundedOutput> {
    let mut bytes = Vec::with_capacity(VERSION_OUTPUT_CAPACITY);
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = VERSION_OUTPUT_CAPACITY.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        truncated |= read > remaining;
    }
    Ok(BoundedOutput { bytes, truncated })
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum DiscoveryError {
    #[error("candidate not found: {0}")]
    NotFound(PathBuf),
    #[error("I/O error at {0}: {1}")]
    Io(PathBuf, String),
    #[error("candidate is not a file: {0}")]
    NotFile(PathBuf),
    #[error("invalid installation receipt: {0}")]
    InvalidReceipt(PathBuf),
    #[error("binary hash mismatch: {0}")]
    HashMismatch(PathBuf),
    #[error("version probe failed for {0}: {1}")]
    Probe(PathBuf, String),
    #[error("version probe timed out for {0}")]
    ProbeTimeout(PathBuf),
    #[error("version output exceeded 64 KiB for {0}")]
    ProbeOutputTooLarge(PathBuf),
    #[error("language server {server} is unavailable")]
    Unavailable {
        server: String,
        attempts: Vec<DiscoveryAttempt>,
    },
}
