//! Explicit one-shot Radio Browser catalog importer.
//!
//! When `RADIO_BROWSER_TAGS` is set (comma-separated), the importer runs one
//! pass per tag to maximize genre coverage. Otherwise it runs a single unfiltered
//! pass for backward compatibility.

use std::{env, error::Error};

use rockserver::{
    catalog_import::{CatalogImportError, CatalogImporter},
    persistence::{DATABASE_URL_ENV, PostgresImportStore},
    providers::radio_browser::{RadioBrowserClient, RadioBrowserConfig, TAGS_ENV},
    telemetry,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    telemetry::init()?;

    let database_url = required_database_url()?;
    let config = RadioBrowserConfig::from_env()?;
    let store = PostgresImportStore::connect(&database_url).await?;

    let tags = env::var(TAGS_ENV)
        .ok()
        .map(|v| {
            v.split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if tags.is_empty() {
        tracing::info!("running unfiltered import pass");
        let client = RadioBrowserClient::new(&config)?;
        let importer = CatalogImporter::new(client, store.clone(), config.limits);
        importer.run().await?;
    } else {
        tracing::info!(tags = ?tags, "running multi-tag import");
        for tag in &tags {
            tracing::info!(tag = %tag, "importing tag");
            let client = RadioBrowserClient::with_tag(&config, Some(tag.clone()))?;
            let importer = CatalogImporter::new(client, store.clone(), config.limits);
            match importer.run().await {
                Ok(result) => {
                    tracing::info!(
                        tag = %tag,
                        fetched = result.counts.fetched,
                        imported = result.counts.imported,
                        skipped = result.counts.skipped,
                        "tag import completed"
                    );
                }
                Err(error) => {
                    tracing::error!(tag = %tag, error = %error, "tag import failed, continuing");
                }
            }
        }
    }

    store.close().await;
    Ok(())
}

fn required_database_url() -> Result<String, CatalogImportError> {
    match env::var(DATABASE_URL_ENV) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => Err(CatalogImportError::safe(
            "DATABASE_URL is required for the Radio Browser importer",
        )),
        Err(env::VarError::NotUnicode(_)) => Err(CatalogImportError::safe(
            "DATABASE_URL must contain valid Unicode",
        )),
    }
}
