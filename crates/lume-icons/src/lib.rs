//! Contratti nativi dei temi di icone dei file, indipendenti dalla UI.

mod builtin;
pub mod limits;
mod manifest;
mod model;
mod registry;
mod resolver;

pub use builtin::{
    CatppuccinIconTheme, LUME_DOT_ID, LUME_MONO_ID, LumeDotIconTheme, LumeMonoIconTheme,
    MaterialIconTheme,
};
pub use manifest::ManifestIconTheme;
pub use model::{
    FileIconContext, IconAsset, IconAssetId, IconColorMode, IconThemeError, IconThemeMetadata,
    IconThemeProvider, TintRole,
};
pub use registry::IconThemeRegistry;
pub use resolver::{FileIconResolver, IconThemeAssociations};
