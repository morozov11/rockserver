//! Import the checksum-pinned complete station release into PostgreSQL.

use std::{env, error::Error};

use rockserver::{
    catalog::{CatalogImporter, FullReleaseCatalogAdapter, ImportLimits},
    persistence::{DATABASE_URL_ENV, PostgresImportStore},
    telemetry,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    telemetry::init()?;
    let adapter = FullReleaseCatalogAdapter::pinned().await?;
    let expected = adapter.station_count();
    let version = adapter.catalog_version().to_owned();
    let database_url = env::var(DATABASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or("DATABASE_URL is required")?;
    let store = PostgresImportStore::connect(&database_url).await?;
    let result = CatalogImporter::new(
        adapter,
        store,
        ImportLimits {
            page_size: 500,
            max_pages: 40,
        },
    )
    .run()
    .await?;
    if result.counts.imported != expected || result.counts.failed != 0 || result.counts.skipped != 0
    {
        return Err("full catalog import did not persist every pinned station".into());
    }
    tracing::info!(catalog_version = %version, stations = expected, run_id = %result.run_id, "complete station catalog activated");
    Ok(())
}
