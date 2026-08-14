//! Controlled development-only station embedding backfill/update command.

use std::{env, error::Error};

use rockserver::{
    persistence::{DATABASE_URL_ENV, PostgresEmbeddingStore},
    providers::deterministic_embedding::{
        DETERMINISTIC_DEV_PROVIDER, DeterministicEmbeddingProvider, SEMANTIC_PROVIDER_ENV,
    },
    search::{EmbeddingBackfill, EmbeddingStoreError},
    telemetry,
};

const PAGE_SIZE: usize = 100;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    telemetry::init()?;

    let database_url = required_database_url()?;
    let provider = DeterministicEmbeddingProvider::optional_from_env()?.ok_or_else(|| {
        EmbeddingStoreError::safe(format!(
            "{SEMANTIC_PROVIDER_ENV}={DETERMINISTIC_DEV_PROVIDER} is required for this development workflow"
        ))
    })?;
    let store = PostgresEmbeddingStore::connect(&database_url).await?;
    let workflow = EmbeddingBackfill::new(provider, store.clone(), PAGE_SIZE);

    let result = workflow.run().await;
    store.close().await;
    let result = result?;
    tracing::info!(
        processed = result.processed,
        updated = result.updated,
        "station embedding backfill completed"
    );
    Ok(())
}

fn required_database_url() -> Result<String, EmbeddingStoreError> {
    match env::var(DATABASE_URL_ENV) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => Err(EmbeddingStoreError::safe(
            "DATABASE_URL is required for embedding backfill",
        )),
        Err(env::VarError::NotUnicode(_)) => Err(EmbeddingStoreError::safe(
            "DATABASE_URL must contain valid Unicode",
        )),
    }
}
