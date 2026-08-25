//! Activate the checksum-pinned RockCatalog baseline in PostgreSQL.
//!
//! This command reads only the snapshot vendored in this repository. It never contacts a network
//! service or a sibling checkout, and preflights the full release before it opens a database run.

use std::{env, error::Error};

use rockserver::{
    catalog::{CatalogImportError, SharedCatalogAdapter},
    persistence::{DATABASE_URL_ENV, PostgresImportStore},
    telemetry,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // Local development may keep the PostgreSQL URL in an ignored `.env`; its value is never logged.
    dotenvy::dotenv().ok();
    telemetry::init()?;
    let adapter = SharedCatalogAdapter::pinned()?;
    let database_url = required_database_url()?;
    let store = PostgresImportStore::connect(&database_url).await?;
    let run_id = store.activate_shared_catalog(adapter.catalog()).await?;
    tracing::info!(%run_id, catalog_version = adapter.catalog().catalog_version(), "shared catalog activated");
    store.close().await;
    Ok(())
}

/// Reads a non-empty database URL without including its value in failure messages.
fn required_database_url() -> Result<String, CatalogImportError> {
    match env::var(DATABASE_URL_ENV) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => Err(CatalogImportError::safe(
            "DATABASE_URL is required for the shared catalog importer",
        )),
        Err(env::VarError::NotUnicode(_)) => Err(CatalogImportError::safe(
            "DATABASE_URL must contain valid Unicode",
        )),
    }
}
