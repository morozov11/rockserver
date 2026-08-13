//! Explicit one-shot Radio Browser catalog importer.

use std::{env, error::Error};

use rockserver::{
    catalog_import::{CatalogImportError, CatalogImporter},
    persistence::{DATABASE_URL_ENV, PostgresImportStore},
    providers::radio_browser::{RadioBrowserClient, RadioBrowserConfig},
    telemetry,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    telemetry::init()?;

    let database_url = required_database_url()?;
    let config = RadioBrowserConfig::from_env()?;
    let client = RadioBrowserClient::new(&config)?;
    let store = PostgresImportStore::connect(&database_url).await?;
    let importer = CatalogImporter::new(client, store.clone(), config.limits);

    let result = importer.run().await;
    store.close().await;
    result?;
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
