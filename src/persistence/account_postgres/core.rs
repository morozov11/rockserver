//! Private PostgreSQL account-store operations grouped by one responsibility.

use super::*;

impl PostgresAccountStore {
    /// Connects to PostgreSQL and applies the shared versioned migration sequence.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        if let Err(error) = MIGRATOR.run(&pool).await {
            pool.close().await;
            return Err(error.into());
        }
        Ok(Self { pool })
    }

    /// Reuses a caller-managed migrated pool, primarily for integration tests.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns a clone of the shared pool for account-owned projection stores.
    pub(crate) fn pool(&self) -> PgPool {
        self.pool.clone()
    }

    /// Closes the underlying pool for deterministic integration-test cleanup.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
