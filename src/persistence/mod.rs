//! Persistent catalog implementations and environment-driven backend selection.

mod import_postgres;
mod postgres;

use std::{env, sync::Arc};

pub use import_postgres::PostgresImportStore;
pub use postgres::PostgresStationRepository;

use crate::search::{InMemoryStationRepository, StationRepository};

/// Environment variable that enables the PostgreSQL catalog backend.
pub const DATABASE_URL_ENV: &str = "DATABASE_URL";

/// Selects and initializes the catalog backend from the process environment.
///
/// PostgreSQL migrations and the development seed are applied before the backend is returned.
/// When `DATABASE_URL` is absent, the six-station in-memory fallback remains the default.
pub async fn repository_from_env()
-> Result<Arc<dyn StationRepository + Send + Sync>, crate::search::RepositoryError> {
    match env::var(DATABASE_URL_ENV) {
        Ok(database_url) => {
            let repository = PostgresStationRepository::connect(&database_url).await?;
            tracing::info!(backend = "postgresql", "station repository selected");
            Ok(Arc::new(repository))
        }
        Err(env::VarError::NotPresent) => {
            tracing::info!(backend = "in_memory", "station repository selected");
            Ok(Arc::new(InMemoryStationRepository::with_builtin_catalog()))
        }
        Err(error) => Err(crate::search::RepositoryError::new("configuration", error)),
    }
}
