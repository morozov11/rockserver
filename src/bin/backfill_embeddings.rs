//! Controlled development-only station embedding backfill/update command.

use std::{env, error::Error, sync::Arc};

use async_trait::async_trait;
use rockserver::{
    persistence::{DATABASE_URL_ENV, PostgresEmbeddingStore},
    providers::embedding_provider_from_env,
    search::{
        Embedding, EmbeddingBackfill, EmbeddingProvider, EmbeddingProviderError,
        EmbeddingStoreError,
    },
    telemetry,
};

const PAGE_SIZE: usize = 100;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    telemetry::init()?;

    let database_url = required_database_url()?;
    let provider = embedding_provider_from_env()?.ok_or_else(|| {
        EmbeddingStoreError::safe("ROCKSERVER_SEMANTIC_PROVIDER is required for embedding backfill")
    })?;
    let store = PostgresEmbeddingStore::connect(&database_url).await?;
    let workflow =
        EmbeddingBackfill::new(SharedEmbeddingProvider(provider), store.clone(), PAGE_SIZE);

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

/// Bridges a selected shared runtime provider to the generic backfill workflow.
struct SharedEmbeddingProvider(Arc<dyn EmbeddingProvider>);

#[async_trait]
impl EmbeddingProvider for SharedEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.0.embed(text).await
    }

    async fn embed_document(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.0.embed_document(text).await
    }
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
