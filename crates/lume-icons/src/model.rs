use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IconAssetId(String);

impl IconAssetId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for IconAssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("IconAssetId").field(&self.0).finish()
    }
}

impl fmt::Display for IconAssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<&str> for IconAssetId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for IconAssetId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Ruolo semantico del colore di un'icona tintabile: la UI lo traduce nel
/// token del tema (`num`, `faint`); senza ruolo usa il colore di default
/// della superficie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TintRole {
    Num,
    Faint,
}

#[derive(Debug, Clone)]
pub struct IconAsset {
    bytes: Arc<[u8]>,
    tintable: bool,
    tint_role: Option<TintRole>,
}

impl IconAsset {
    pub fn new(bytes: impl Into<Arc<[u8]>>, tintable: bool) -> Self {
        Self {
            bytes: bytes.into(),
            tintable,
            tint_role: None,
        }
    }

    pub fn with_tint_role(mut self, role: TintRole) -> Self {
        self.tint_role = Some(role);
        self
    }

    pub fn tint_role(&self) -> Option<TintRole> {
        self.tint_role
    }

    pub fn from_static(bytes: &'static [u8], tintable: bool) -> Self {
        Self::new(Arc::<[u8]>::from(bytes), tintable)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn tintable(&self) -> bool {
        self.tintable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconThemeMetadata {
    pub id: String,
    pub name: String,
    pub built_in_rank: Option<u16>,
}

impl IconThemeMetadata {
    pub fn built_in(id: impl Into<String>, name: impl Into<String>, rank: u16) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            built_in_rank: Some(rank),
        }
    }

    pub fn external(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            built_in_rank: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconColorMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIconContext {
    pub display_name: String,
    pub language_id: Option<String>,
    pub is_directory: bool,
    pub is_root: bool,
    pub is_open: bool,
    pub color_mode: IconColorMode,
}

impl FileIconContext {
    pub fn file(display_name: impl Into<String>) -> Self {
        Self {
            display_name: display_name.into(),
            language_id: None,
            is_directory: false,
            is_root: false,
            is_open: false,
            color_mode: IconColorMode::Dark,
        }
    }

    pub fn folder(display_name: impl Into<String>, is_open: bool) -> Self {
        Self {
            display_name: display_name.into(),
            language_id: None,
            is_directory: true,
            is_root: false,
            is_open,
            color_mode: IconColorMode::Dark,
        }
    }

    pub fn root_folder(display_name: impl Into<String>, is_open: bool) -> Self {
        Self {
            is_root: true,
            ..Self::folder(display_name, is_open)
        }
    }
}

pub trait IconThemeProvider: Send + Sync {
    fn metadata(&self) -> &IconThemeMetadata;
    fn resolve(&self, context: &FileIconContext) -> IconAssetId;
    fn asset(&self, id: &IconAssetId) -> Result<IconAsset, IconThemeError>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IconThemeError {
    #[error("duplicate icon theme provider id: {0}")]
    DuplicateId(String),
    #[error("unknown icon theme provider: {0}")]
    UnknownProvider(String),
    #[error("icon theme provider id is empty")]
    EmptyProviderId,
    #[error("icon asset is missing: {0}")]
    MissingAsset(IconAssetId),
    #[error("I/O error for {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("manifest is not valid UTF-8: {path}")]
    InvalidUtf8 { path: PathBuf },
    #[error("invalid icon manifest: {0}")]
    InvalidManifest(String),
    #[error("manifest is {actual} bytes; maximum is {max}")]
    ManifestTooLarge { actual: usize, max: usize },
    #[error("SVG {path} is {actual} bytes; maximum is {max}")]
    SvgTooLarge {
        path: PathBuf,
        actual: usize,
        max: usize,
    },
    #[error("theme has {actual} assets; maximum is {max}")]
    TooManyAssets { actual: usize, max: usize },
    #[error("theme has {actual} associations; maximum is {max}")]
    TooManyAssociations { actual: usize, max: usize },
    #[error("theme occupies {actual} bytes; maximum is {max}")]
    PackTooLarge { actual: usize, max: usize },
    #[error("path escapes icon pack root: {path}")]
    PathEscape { path: PathBuf },
    #[error("absolute icon asset path is forbidden: {0}")]
    AbsoluteAssetPath(String),
    #[error("remote icon asset path is forbidden: {0}")]
    RemoteAssetPath(String),
    #[error("icon association references missing definition: {0}")]
    MissingDefinition(String),
    #[error("forbidden SVG construct in {path}: {reason}")]
    ForbiddenSvg { path: PathBuf, reason: String },
    #[error("invalid SVG {path}: {message}")]
    InvalidSvg { path: PathBuf, message: String },
    #[error("invalid SVG asset {provider}/{id}: {message}")]
    InvalidSvgAsset {
        provider: String,
        id: IconAssetId,
        message: String,
    },
}
