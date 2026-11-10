//! PostgreSQL persistence for controlled station embedding backfill and updates.

use async_trait::async_trait;
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};

use crate::search::{
    Embedding, EmbeddingProvenance, EmbeddingStore, EmbeddingStoreError, StationEmbeddingDocument,
};

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

    /// Counts current E5 vectors and verifies the version-neutral ANN index used by search SQL.
    pub async fn e5_release_readiness(
        &self,
        provenance: &EmbeddingProvenance,
    ) -> Result<EmbeddingReleaseReadiness, EmbeddingStoreError> {
        if provenance.model != "intfloat/multilingual-e5-small" || provenance.dimension != 384 {
            return Err(EmbeddingStoreError::safe(
                "unsupported E5 release provenance",
            ));
        }
        let (active_stations, current_vectors) = sqlx::query_as::<_, (i64, i64)>(
            r#"
SELECT count(*), count(e.station_id)
FROM stations AS s
LEFT JOIN station_embeddings AS e
  ON e.station_id = s.id AND e.model = $1 AND e.version = $2 AND e.dimension = 384
 AND e.source_text_sha256 = encode(sha256(convert_to(s.searchable_text, 'UTF8')), 'hex')
WHERE s.retired_at IS NULL
"#,
        )
        .bind(&provenance.model)
        .bind(&provenance.version)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| EmbeddingStoreError::safe("E5 release coverage check failed"))?;
        let ann_index_ready = sqlx::query_scalar::<_, bool>(
            r#"
SELECT EXISTS (
    SELECT 1 FROM pg_index AS i
    JOIN pg_class AS idx ON idx.oid = i.indexrelid
    JOIN pg_am AS am ON am.oid = idx.relam
    WHERE idx.relname = 'station_embeddings_e5_384_hnsw_cosine_idx'
      AND am.amname = 'hnsw' AND i.indisvalid AND i.indisready
      AND pg_get_indexdef(i.indexrelid) LIKE '%(embedding)::vector(384)%'
      AND pg_get_indexdef(i.indexrelid) LIKE '%vector_cosine_ops%'
      AND pg_get_expr(i.indpred, i.indrelid) LIKE '%intfloat/multilingual-e5-small%'
      AND pg_get_expr(i.indpred, i.indrelid) LIKE '%dimension = 384%'
      AND pg_get_expr(i.indpred, i.indrelid) NOT LIKE '%version%'
)
"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|_| EmbeddingStoreError::safe("E5 ANN index check failed"))?;
        Ok(EmbeddingReleaseReadiness {
            active_stations,
            current_vectors,
            ann_index_ready,
        })
    }
}

/// Current catalog coverage and the ANN index prerequisite for one E5 provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddingReleaseReadiness {
    /// Number of active stations requiring embeddings.
    pub active_stations: i64,
    /// Number of active stations with a current vector of the requested provenance.
    pub current_vectors: i64,
    /// Whether the valid E5/384 HNSW index matches the search SQL expression and predicate.
    pub ann_index_ready: bool,
}

impl EmbeddingReleaseReadiness {
    /// Returns true only when all active documents are covered and ANN is indexable.
    pub fn is_ready(self) -> bool {
        self.active_stations > 0
            && self.active_stations == self.current_vectors
            && self.ann_index_ready
    }
}

#[async_trait]
impl EmbeddingStore for PostgresEmbeddingStore {
    async fn station_documents(
        &self,
        provenance: &EmbeddingProvenance,
        after_station_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StationEmbeddingDocument>, EmbeddingStoreError> {
        let limit = i64::try_from(limit)
            .map_err(|_| EmbeddingStoreError::safe("embedding page size exceeded bigint"))?;
        let rows = sqlx::query_as::<_, StationDocumentRow>(
            r#"
SELECT
    s.id AS station_id,
    s.searchable_text AS text,
    encode(sha256(convert_to(s.searchable_text, 'UTF8')), 'hex') AS source_text_sha256
FROM stations AS s
LEFT JOIN station_embeddings AS e
  ON e.station_id = s.id AND e.model = $1 AND e.version = $2
WHERE s.retired_at IS NULL
  AND ($3::text IS NULL OR s.id > $3)
  AND (e.station_id IS NULL OR e.dimension <> $4
       OR e.source_text_sha256 IS DISTINCT FROM encode(sha256(convert_to(s.searchable_text, 'UTF8')), 'hex'))
ORDER BY s.id ASC
LIMIT $5
"#,
        )
        .bind(&provenance.model)
        .bind(&provenance.version)
        .bind(after_station_id)
        .bind(i32::try_from(provenance.dimension).map_err(|_| {
            EmbeddingStoreError::safe("embedding dimension exceeded integer")
        })?)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| EmbeddingStoreError::safe("PostgreSQL station document read failed"))?;

        Ok(rows
            .into_iter()
            .map(|row| StationEmbeddingDocument {
                station_id: row.station_id,
                text: row.text,
                source_text_sha256: row.source_text_sha256,
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
    source_text_sha256 = NULL,
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

    async fn upsert_document_embedding(
        &self,
        document: &StationEmbeddingDocument,
        embedding: &Embedding,
    ) -> Result<bool, EmbeddingStoreError> {
        let dimension = i32::try_from(embedding.provenance().dimension)
            .map_err(|_| EmbeddingStoreError::safe("embedding dimension exceeded integer"))?;
        // Row lock makes a concurrent importer wait; its text-change invalidation then wins.
        let written = sqlx::query(
            r#"
WITH source AS (
    SELECT id FROM stations
    WHERE id = $1 AND retired_at IS NULL
      AND encode(sha256(convert_to(searchable_text, 'UTF8')), 'hex') = $2
    FOR SHARE
)
INSERT INTO station_embeddings
    (station_id, model, version, dimension, embedding, source_text_sha256)
SELECT source.id, $3, $4, $5, $6::vector, $2 FROM source
ON CONFLICT (station_id, model, version) DO UPDATE SET
    dimension = EXCLUDED.dimension,
    embedding = EXCLUDED.embedding,
    source_text_sha256 = EXCLUDED.source_text_sha256,
    updated_at = now()
"#,
        )
        .bind(&document.station_id)
        .bind(&document.source_text_sha256)
        .bind(&embedding.provenance().model)
        .bind(&embedding.provenance().version)
        .bind(dimension)
        .bind(vector_literal(embedding))
        .execute(&self.pool)
        .await
        .map_err(|_| EmbeddingStoreError::safe("PostgreSQL document embedding upsert failed"))?;
        Ok(written.rows_affected() == 1)
    }
}

#[derive(Debug, FromRow)]
struct StationDocumentRow {
    station_id: String,
    text: String,
    source_text_sha256: String,
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
#[path = "embedding_postgres/tests.rs"]
mod tests;
