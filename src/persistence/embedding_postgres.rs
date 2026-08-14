//! PostgreSQL persistence for controlled station embedding backfill and updates.

use async_trait::async_trait;
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};

use crate::search::{Embedding, EmbeddingStore, EmbeddingStoreError, StationEmbeddingDocument};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// PostgreSQL store for station documents and provenance-aware embeddings.
#[derive(Clone, Debug)]
pub struct PostgresEmbeddingStore {
    pool: PgPool,
}

impl PostgresEmbeddingStore {
    /// Connects to PostgreSQL and applies pending versioned migrations.
    pub async fn connect(database_url: &str) -> Result<Self, EmbeddingStoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|_| EmbeddingStoreError::safe("PostgreSQL embedding connection failed"))?;
        if MIGRATOR.run(&pool).await.is_err() {
            pool.close().await;
            return Err(EmbeddingStoreError::safe(
                "PostgreSQL embedding migration failed",
            ));
        }
        Ok(Self { pool })
    }

    /// Closes this store's shared connection pool after in-flight work completes.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[async_trait]
impl EmbeddingStore for PostgresEmbeddingStore {
    async fn station_documents(
        &self,
        after_station_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StationEmbeddingDocument>, EmbeddingStoreError> {
        let limit = i64::try_from(limit)
            .map_err(|_| EmbeddingStoreError::safe("embedding page size exceeded bigint"))?;
        let rows = sqlx::query_as::<_, StationDocumentRow>(
            r#"
SELECT
    id AS station_id,
    concat_ws(' ', name, array_to_string(tags, ' ')) AS text
FROM stations
WHERE ($1::text IS NULL OR id > $1)
ORDER BY id ASC
LIMIT $2
"#,
        )
        .bind(after_station_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| EmbeddingStoreError::safe("PostgreSQL station document read failed"))?;

        Ok(rows
            .into_iter()
            .map(|row| StationEmbeddingDocument {
                station_id: row.station_id,
                text: row.text,
            })
            .collect())
    }

    async fn upsert_embedding(
        &self,
        station_id: &str,
        embedding: &Embedding,
    ) -> Result<(), EmbeddingStoreError> {
        let dimension = i32::try_from(embedding.provenance().dimension)
            .map_err(|_| EmbeddingStoreError::safe("embedding dimension exceeded integer"))?;
        let vector = vector_literal(embedding);
        sqlx::query(
            r#"
INSERT INTO station_embeddings (station_id, model, version, dimension, embedding)
VALUES ($1, $2, $3, $4, $5::vector)
ON CONFLICT (station_id, model, version) DO UPDATE SET
    dimension = EXCLUDED.dimension,
    embedding = EXCLUDED.embedding,
    updated_at = now()
"#,
        )
        .bind(station_id)
        .bind(&embedding.provenance().model)
        .bind(&embedding.provenance().version)
        .bind(dimension)
        .bind(vector)
        .execute(&self.pool)
        .await
        .map_err(|_| EmbeddingStoreError::safe("PostgreSQL embedding upsert failed"))?;
        Ok(())
    }
}

#[derive(Debug, FromRow)]
struct StationDocumentRow {
    station_id: String,
    text: String,
}

/// Formats already-validated finite values as a pgvector input literal.
pub(crate) fn vector_literal(embedding: &Embedding) -> String {
    let values = embedding
        .values()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("[{values}]")
}

#[cfg(test)]
mod tests {
    use crate::search::Embedding;

    use super::vector_literal;

    #[test]
    fn validated_embedding_formats_as_pgvector_literal() {
        let embedding = Embedding::new("fake", "1", 3, vec![1.0, -0.5, 0.25]).unwrap();

        assert_eq!(vector_literal(&embedding), "[1,-0.5,0.25]");
    }
}
