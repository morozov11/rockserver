//! Catalog domain and import orchestration.

pub mod full_release;
pub mod import;
pub mod shared;

pub use full_release::{FULL_RELEASE_SOURCE, FullReleaseCatalogAdapter};
pub use import::{
    CatalogImportError, CatalogImportProvider, CatalogImportStore, CatalogImporter, ImportCounts,
    ImportLimits, ImportPage, ImportRunResult, ImportedStation, ImportedStream, ImportedTombstone,
    TombstoneReason,
};
pub use shared::{
    CatalogReplacement, PinnedSharedCatalog, ROCKCATALOG_SOURCE, SharedCatalogAdapter,
    SharedCatalogError,
};
