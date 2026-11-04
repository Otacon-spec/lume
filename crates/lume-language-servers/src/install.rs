use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use flate2::read::GzDecoder;
use reqwest::StatusCode;
use reqwest::Url;
use reqwest::blocking::{Client, Response};
use reqwest::header::LOCATION;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{ArchiveFormat, Platform, PlatformAsset, ServerSpec};

const MAX_EXPANDED: usize = 256 * 1024 * 1024;
const DOWNLOAD_CHUNK: usize = 64 * 1024;
const MAX_REDIRECTS: usize = 5;
const ALLOWED_DOWNLOAD_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);
impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<(), InstallError> {
        (!self.is_cancelled())
            .then_some(())
            .ok_or(InstallError::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallProgress {
    Downloading { received: u64, total: u64 },
    Verifying,
    Extracting,
    Installed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallReceipt {
    pub server_id: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub binary: String,
    pub sha256: String,
}

pub struct ServerInstaller {
    managed_root: PathBuf,
}
impl ServerInstaller {
    pub fn new(managed_root: PathBuf) -> Self {
        Self { managed_root }
    }

    /// Downloads only the pinned catalog asset and installs it after exact size and SHA-256 checks.
    pub fn install(
        &self,
        server: &ServerSpec,
        platform: &Platform,
        cancel: &CancelFlag,
        mut progress: impl FnMut(InstallProgress),
    ) -> Result<InstallReceipt, InstallError> {
        let asset = server
            .asset_for(platform)
            .map_err(|error| InstallError::Catalog(error.to_string()))?;
        let bytes = download_asset(asset, cancel, &mut progress)?;
        self.install_bytes(server, platform, asset, &bytes, cancel, &mut progress)
    }

    /// Verified archive installation primitive shared by Core downloads and Bundle creation.
    pub fn install_bytes(
        &self,
        server: &ServerSpec,
        platform: &Platform,
        asset: &PlatformAsset,
        bytes: &[u8],
        cancel: &CancelFlag,
        mut progress: impl FnMut(InstallProgress),
    ) -> Result<InstallReceipt, InstallError> {
        cancel.check()?;
        progress(InstallProgress::Verifying);
        if bytes.len() as u64 != asset.size {
            return Err(InstallError::SizeMismatch {
                expected: asset.size,
                actual: bytes.len() as u64,
            });
        }
        let digest = format!("{:x}", Sha256::digest(bytes));
        if !digest.eq_ignore_ascii_case(asset.digest.strip_prefix("sha256:").unwrap_or("")) {
            return Err(InstallError::HashMismatch);
        }
        cancel.check()?;
        progress(InstallProgress::Extracting);
        let binary = extract_binary(asset, bytes)?;
        cancel.check()?;

        let target = self
            .managed_root
            .join(&server.id)
            .join(&server.release.version)
            .join(format!("{}-{}", platform.os, platform.arch));
        if target.exists() {
            return Err(InstallError::AlreadyInstalled(target));
        }
        fs::create_dir_all(&self.managed_root).map_err(|e| io_error(&self.managed_root, e))?;
        let staging =
            self.managed_root
                .join(format!(".install-{}-{}", server.id, std::process::id()));
        if staging.exists() {
            return Err(InstallError::StagingExists(staging));
        }
        fs::create_dir(&staging).map_err(|e| io_error(&staging, e))?;
        let result = (|| {
            let binary_name = if platform.os == "windows" {
                format!("{}.exe", server.executable)
            } else {
                server.executable.clone()
            };
            let binary_path = staging.join(&binary_name);
            let mut output = File::create(&binary_path).map_err(|e| io_error(&binary_path, e))?;
            output
                .write_all(&binary)
                .and_then(|_| output.sync_all())
                .map_err(|e| io_error(&binary_path, e))?;
            drop(output);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o755))
                    .map_err(|e| io_error(&binary_path, e))?;
            }
            let receipt = InstallReceipt {
                server_id: server.id.clone(),
                version: server.release.version.clone(),
                os: platform.os.clone(),
                arch: platform.arch.clone(),
                binary: binary_name,
                sha256: format!("{:x}", Sha256::digest(&binary)),
            };
            let receipt_path = staging.join("receipt.json");
            let mut receipt_file =
                File::create(&receipt_path).map_err(|e| io_error(&receipt_path, e))?;
            receipt_file
                .write_all(
                    &serde_json::to_vec_pretty(&receipt)
                        .map_err(|e| InstallError::Receipt(e.to_string()))?,
                )
                .and_then(|_| receipt_file.sync_all())
                .map_err(|e| io_error(&receipt_path, e))?;
            drop(receipt_file);
            let target_parent = target
                .parent()
                .ok_or_else(|| InstallError::InvalidInstallPath(target.clone()))?;
            fs::create_dir_all(target_parent).map_err(|e| io_error(&target, e))?;
            fs::rename(&staging, &target).map_err(|e| io_error(&target, e))?;
            Ok(receipt)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        let receipt = result?;
        progress(InstallProgress::Installed);
        Ok(receipt)
    }
}

fn download_asset(
    asset: &PlatformAsset,
    cancel: &CancelFlag,
    progress: &mut impl FnMut(InstallProgress),
) -> Result<Vec<u8>, InstallError> {
    crate::install_crypto_provider();
    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .user_agent("Lume/0.1 verified-server-installer")
        .build()
        .map_err(|error| InstallError::Http(error.to_string()))?;
    let mut url =
        Url::parse(&asset.url).map_err(|_| InstallError::ForbiddenUrl(asset.url.clone()))?;
    validate_download_url(&url)?;

    for redirect_count in 0..=MAX_REDIRECTS {
        cancel.check()?;
        let response = client
            .get(url.clone())
            .send()
            .map_err(|error| InstallError::Http(error.to_string()))?;
        if response.status().is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err(InstallError::TooManyRedirects);
            }
            url = redirect_target(&url, &response)?;
            validate_download_url(&url)?;
            continue;
        }
        if response.status() != StatusCode::OK {
            return Err(InstallError::HttpStatus(response.status().as_u16()));
        }
        return read_download(response, asset.size, cancel, progress);
    }
    Err(InstallError::TooManyRedirects)
}

fn redirect_target(current: &Url, response: &Response) -> Result<Url, InstallError> {
    let location = response
        .headers()
        .get(LOCATION)
        .ok_or(InstallError::RedirectWithoutLocation)?
        .to_str()
        .map_err(|_| InstallError::InvalidRedirect)?;
    current
        .join(location)
        .map_err(|_| InstallError::InvalidRedirect)
}

fn validate_download_url(url: &Url) -> Result<(), InstallError> {
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || !ALLOWED_DOWNLOAD_HOSTS.contains(&host)
    {
        return Err(InstallError::ForbiddenUrl(url.to_string()));
    }
    Ok(())
}

#[allow(clippy::indexing_slicing)] // count <= chunk.len() (contratto di Read)
fn read_download(
    mut response: Response,
    expected_size: u64,
    cancel: &CancelFlag,
    progress: &mut impl FnMut(InstallProgress),
) -> Result<Vec<u8>, InstallError> {
    if let Some(length) = response.content_length()
        && length != expected_size
    {
        return Err(InstallError::SizeMismatch {
            expected: expected_size,
            actual: length,
        });
    }
    let capacity = usize::try_from(expected_size).map_err(|_| InstallError::AssetTooLarge)?;
    let mut output = Vec::with_capacity(capacity);
    let mut chunk = [0_u8; DOWNLOAD_CHUNK];
    loop {
        cancel.check()?;
        let count = response
            .read(&mut chunk)
            .map_err(|error| InstallError::Http(error.to_string()))?;
        if count == 0 {
            break;
        }
        let received = output.len() as u64 + count as u64;
        if received > expected_size {
            return Err(InstallError::SizeMismatch {
                expected: expected_size,
                actual: received,
            });
        }
        output.extend_from_slice(&chunk[..count]);
        progress(InstallProgress::Downloading {
            received,
            total: expected_size,
        });
    }
    if output.len() as u64 != expected_size {
        return Err(InstallError::SizeMismatch {
            expected: expected_size,
            actual: output.len() as u64,
        });
    }
    Ok(output)
}

fn extract_binary(asset: &PlatformAsset, bytes: &[u8]) -> Result<Vec<u8>, InstallError> {
    match asset.archive {
        ArchiveFormat::Gzip => bounded(GzDecoder::new(bytes)),
        ArchiveFormat::Zip => {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
                .map_err(|e| InstallError::Archive(e.to_string()))?;
            let mut found = None;
            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|e| InstallError::Archive(e.to_string()))?;
                let path = entry.enclosed_name().ok_or_else(|| {
                    InstallError::UnsafeEntry(
                        String::from_utf8_lossy(entry.name_raw()).into_owned(),
                    )
                })?;
                if path == Path::new(&asset.binary_path) && !entry.is_dir() {
                    if found.is_some() {
                        return Err(InstallError::DuplicateBinary);
                    }
                    found = Some(bounded(&mut entry)?);
                }
            }
            found.ok_or(InstallError::BinaryMissing)
        }
        ArchiveFormat::TarGz => {
            let mut archive = tar::Archive::new(GzDecoder::new(bytes));
            let mut found = None;
            for entry in archive
                .entries()
                .map_err(|e| InstallError::Archive(e.to_string()))?
            {
                let mut entry = entry.map_err(|e| InstallError::Archive(e.to_string()))?;
                let path = entry
                    .path()
                    .map_err(|e| InstallError::Archive(e.to_string()))?;
                if path == Path::new(&asset.binary_path) {
                    if !entry.header().entry_type().is_file() {
                        return Err(InstallError::UnsafeEntry(path.display().to_string()));
                    }
                    if found.is_some() {
                        return Err(InstallError::DuplicateBinary);
                    }
                    found = Some(bounded(&mut entry)?);
                }
            }
            found.ok_or(InstallError::BinaryMissing)
        }
    }
}

fn bounded(mut input: impl Read) -> Result<Vec<u8>, InstallError> {
    let mut output = Vec::new();
    input
        .by_ref()
        .take(MAX_EXPANDED as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|e| InstallError::Archive(e.to_string()))?;
    if output.len() > MAX_EXPANDED {
        Err(InstallError::ExpandedTooLarge)
    } else {
        Ok(output)
    }
}
fn io_error(path: &Path, error: std::io::Error) -> InstallError {
    InstallError::Io(path.to_path_buf(), error.to_string())
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum InstallError {
    #[error("download cancelled")]
    Cancelled,
    #[error("catalog error: {0}")]
    Catalog(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("download returned HTTP {0}")]
    HttpStatus(u16),
    #[error("forbidden download URL: {0}")]
    ForbiddenUrl(String),
    #[error("redirect response has no location")]
    RedirectWithoutLocation,
    #[error("invalid redirect location")]
    InvalidRedirect,
    #[error("download exceeded five redirects")]
    TooManyRedirects,
    #[error("asset cannot fit in memory on this platform")]
    AssetTooLarge,
    #[error("asset size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("asset SHA-256 mismatch")]
    HashMismatch,
    #[error("archive error: {0}")]
    Archive(String),
    #[error("unsafe archive entry: {0}")]
    UnsafeEntry(String),
    #[error("archive contains duplicate binary")]
    DuplicateBinary,
    #[error("expected binary is missing")]
    BinaryMissing,
    #[error("expanded binary exceeds 256 MiB")]
    ExpandedTooLarge,
    #[error("installation already exists: {0}")]
    AlreadyInstalled(PathBuf),
    #[error("staging directory already exists: {0}")]
    StagingExists(PathBuf),
    #[error("invalid installation path: {0}")]
    InvalidInstallPath(PathBuf),
    #[error("receipt error: {0}")]
    Receipt(String),
    #[error("I/O error at {0}: {1}")]
    Io(PathBuf, String),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{CancelFlag, InstallError, validate_download_url};
    use reqwest::Url;

    #[test]
    fn download_urls_are_https_allowlisted_and_have_no_credentials() {
        assert!(validate_download_url(&Url::parse("https://github.com/a/b").unwrap()).is_ok());
        for value in [
            "http://github.com/a/b",
            "https://evil.example/a/b",
            "https://user@github.com/a/b",
        ] {
            assert!(matches!(
                validate_download_url(&Url::parse(value).unwrap()),
                Err(InstallError::ForbiddenUrl(_))
            ));
        }
    }

    #[test]
    fn il_flag_di_annullamento_si_legge_e_ferma_i_controlli() {
        let flag = CancelFlag::default();
        assert!(!flag.is_cancelled());
        let copia = flag.clone();
        copia.cancel();
        assert!(flag.is_cancelled(), "le copie condividono lo stesso flag");
        assert!(matches!(flag.check(), Err(InstallError::Cancelled)));
    }
}
