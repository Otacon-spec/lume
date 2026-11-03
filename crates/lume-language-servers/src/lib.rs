//! Catalogo dei language server di produzione, distribuzione e politica del ciclo di vita.

mod catalog;
mod discovery;
mod install;

pub use catalog::{
    ArchiveFormat, CatalogError, DistributionMode, Platform, PlatformAsset, ReleaseSpec,
    ResourceClass, ServerCatalog, ServerSpec,
};
pub use discovery::{
    AttemptFailure, DiscoveryAttempt, DiscoveryError, ResolvedServer, ServerLocator,
    describe_unavailable,
};
pub use install::{CancelFlag, InstallError, InstallProgress, InstallReceipt, ServerInstaller};
