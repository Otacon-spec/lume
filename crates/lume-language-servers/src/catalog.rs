use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

const EMBEDDED_CATALOG: &str = include_str!("../assets/server-releases.json");
const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributionMode {
    Core,
    Bundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceClass {
    Light,
    Heavy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    Gzip,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformAsset {
    pub platform: Platform,
    pub name: String,
    pub url: String,
    pub digest: String,
    pub size: u64,
    pub archive: ArchiveFormat,
    pub binary_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSpec {
    pub repository: String,
    pub version: String,
    pub assets: Vec<PlatformAsset>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSpec {
    pub id: String,
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub languages: Vec<String>,
    pub extensions: Vec<String>,
    #[serde(default)]
    pub workspace_markers: Vec<String>,
    pub resource_class: ResourceClass,
    pub release: ReleaseSpec,
}

impl ServerSpec {
    pub fn asset_for(&self, platform: &Platform) -> Result<&PlatformAsset, CatalogError> {
        self.release
            .assets
            .iter()
            .find(|asset| &asset.platform == platform)
            .ok_or_else(|| CatalogError::UnsupportedPlatform {
                server: self.id.clone(),
                os: platform.os.clone(),
                arch: platform.arch.clone(),
            })
    }
}

#[derive(Debug, Clone)]
pub struct ServerCatalog {
    servers: Vec<ServerSpec>,
    extensions: HashMap<String, usize>,
}

impl ServerCatalog {
    pub fn embedded() -> Result<&'static Self, CatalogError> {
        static CATALOG: OnceLock<Result<ServerCatalog, CatalogError>> = OnceLock::new();
        CATALOG
            .get_or_init(|| Self::parse(EMBEDDED_CATALOG))
            .as_ref()
            .map_err(Clone::clone)
    }

    pub fn parse(source: &str) -> Result<Self, CatalogError> {
        let wire: CatalogWire = serde_json::from_str(source)
            .map_err(|error| CatalogError::InvalidJson(error.to_string()))?;
        if wire.schema != 1 {
            return Err(CatalogError::UnsupportedSchema(wire.schema));
        }
        let mut ids = HashSet::new();
        let mut extensions = HashMap::new();
        for (index, server) in wire.servers.iter().enumerate() {
            validate_server(server)?;
            if !ids.insert(server.id.clone()) {
                return Err(CatalogError::DuplicateServer(server.id.clone()));
            }
            for extension in &server.extensions {
                let key = normalize_extension(extension);
                if extensions.insert(key.clone(), index).is_some() {
                    return Err(CatalogError::DuplicateExtension(key));
                }
            }
        }
        Ok(Self {
            servers: wire.servers,
            extensions,
        })
    }

    pub fn servers(&self) -> impl Iterator<Item = &ServerSpec> {
        self.servers.iter()
    }

    pub fn get(&self, id: &str) -> Option<&ServerSpec> {
        self.servers.iter().find(|server| server.id == id)
    }

    pub fn for_path(&self, path: &Path) -> Option<&ServerSpec> {
        let extension = path.extension()?.to_str()?;
        self.extensions
            .get(&normalize_extension(extension))
            .and_then(|index| self.servers.get(*index))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogWire {
    schema: u32,
    servers: Vec<ServerSpec>,
}

fn validate_server(server: &ServerSpec) -> Result<(), CatalogError> {
    for value in [
        server.id.as_str(),
        server.name.as_str(),
        server.executable.as_str(),
        server.release.repository.as_str(),
        server.release.version.as_str(),
    ] {
        if value.is_empty() || value.len() > MAX_TEXT_BYTES {
            return Err(CatalogError::InvalidField(value.to_owned()));
        }
    }
    let expected_repository = match server.id.as_str() {
        "rust-analyzer" => "rust-lang/rust-analyzer",
        "tombi" => "tombi-toml/tombi",
        "rumdl" => "rvben/rumdl",
        _ => return Err(CatalogError::UnknownServer(server.id.clone())),
    };
    if server.release.repository != expected_repository {
        return Err(CatalogError::InvalidRepository(
            server.release.repository.clone(),
        ));
    }
    if server.languages.is_empty()
        || server.extensions.is_empty()
        || server.release.assets.is_empty()
        || server.languages.len() > 32
        || server.extensions.len() > 64
        || server.release.assets.len() > 16
    {
        return Err(CatalogError::InvalidField(server.id.clone()));
    }
    let repository_prefix = format!(
        "https://github.com/{}/releases/download/{}/",
        server.release.repository, server.release.version
    );
    let mut platforms = HashSet::new();
    for asset in &server.release.assets {
        if !platforms.insert(asset.platform.clone()) {
            return Err(CatalogError::DuplicatePlatform(server.id.clone()));
        }
        if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
            return Err(CatalogError::InvalidAssetSize(asset.name.clone()));
        }
        if !asset.url.starts_with(&repository_prefix) {
            return Err(CatalogError::InvalidAssetUrl(asset.url.clone()));
        }
        let digest = asset.digest.strip_prefix("sha256:").unwrap_or("");
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CatalogError::InvalidDigest(asset.digest.clone()));
        }
        let binary_path = Path::new(&asset.binary_path);
        if binary_path.is_absolute()
            || binary_path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(CatalogError::InvalidBinaryPath(asset.binary_path.clone()));
        }
    }
    Ok(())
}

fn normalize_extension(extension: &str) -> String {
    extension.trim_start_matches('.').to_ascii_lowercase()
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("invalid server catalog JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported server catalog schema: {0}")]
    UnsupportedSchema(u32),
    #[error("duplicate server id: {0}")]
    DuplicateServer(String),
    #[error("duplicate file extension: {0}")]
    DuplicateExtension(String),
    #[error("duplicate platform for server: {0}")]
    DuplicatePlatform(String),
    #[error("invalid catalog field: {0}")]
    InvalidField(String),
    #[error("invalid official repository: {0}")]
    InvalidRepository(String),
    #[error("unknown production server: {0}")]
    UnknownServer(String),
    #[error("invalid release asset URL: {0}")]
    InvalidAssetUrl(String),
    #[error("invalid release digest: {0}")]
    InvalidDigest(String),
    #[error("invalid release asset size: {0}")]
    InvalidAssetSize(String),
    #[error("invalid binary path: {0}")]
    InvalidBinaryPath(String),
    #[error("{server} has no managed asset for {os}/{arch}")]
    UnsupportedPlatform {
        server: String,
        os: String,
        arch: String,
    },
}
