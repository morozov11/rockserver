//! PostgreSQL store for one versioned station-embedding table.

use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};

use crate::search::EmbeddingStoreError;

use super::{embeddings_table_name, station_fields_hash};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// A station whose versioned vector is missing or stale, with the exact fields
/// to send to `EmbedStations` and the hash attesting them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingStationEmbedding {
    /// Stable station identifier.
    pub station_id: String,
    /// Station name as stored (input to the service-built station text).
    pub name: String,
    /// Station tags in stored order.
    pub tags: Vec<String>,
    /// Broadcast language code, if any.
    pub language: Option<String>,
    /// ISO country code, if any.
    pub country_code: Option<String>,
    /// SHA-256 of the canonical serialization of the fields above.
    pub fields_hash: String,
}

/// A computed vector to persist for one station under its sent fields hash.
#[derive(Clone, Debug, PartialEq)]
pub struct WrittenStationEmbedding {
    /// Stable station identifier the vector was computed for.
    pub station_id: String,
    /// Hash of the station fields that were sent to the service.
    pub fields_hash: String,
    /// Finite, non-zero vector components of the table's dimension.
    pub values: Vec<f32>,
}

/// Owns the single dedicated connection used for the advisory lock and writes.
///
/// The session-level advisory lock is released when this connection drops or
/// breaks, which is the rollback path for a crashed backfill.
#[derive(Debug)]
pub struct EmbeddingVersionStore {
    connection: PgConnection,
    /// Dimension remembered from the last `ensure_table` call; writes to a
    /// version whose table was never ensured are rejected.
    dimension: Option<usize>,
}

impl EmbeddingVersionStore {
    /// Applies pending migrations, then opens the dedicated connection.
    pub async fn connect(database_url: &str) -> Result<Self, EmbeddingStoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await
            .map_err(|_| EmbeddingStoreError::safe("versioned embedding connection failed"))?;
        if MIGRATOR.run(&pool).await.is_err() {
            pool.close().await;
            return Err(EmbeddingStoreError::safe(
                "versioned embedding migration failed",
            ));
        }
        let connection = pool
            .acquire()
            .await
            .map_err(|_| EmbeddingStoreError::safe("versioned embedding acquire failed"))?
            .detach();
        pool.close().await;
        Ok(Self {
            connection,
            dimension: None,
        })
    }

    /// Takes ownership of an already-migrated dedicated connection.
    pub fn from_connection(connection: PgConnection) -> Self {
        Self {
            connection,
            dimension: None,
        }
    }

    /// Creates the versioned table and HNSW index if absent and verifies the
    /// schema matches the requested dimension.
    ///
    /// An existing table with a different `embedding` column type, a missing
    /// primary key, or no valid HNSW cosine index is an error; the caller then
    /// keeps the no-vectors mode instead of running searches against an
    /// incompatible table.
    pub async fn ensure_table(
        &mut self,
        version: u32,
        dimension: usize,
    ) -> Result<(), EmbeddingStoreError> {
        let table = embeddings_table_name(version);
        let index = format!("{table}_hnsw_cosine_idx");
        sqlx::query(&format!(
            r#"
CREATE TABLE IF NOT EXISTS {table} (
    station_id varchar(128) PRIMARY KEY REFERENCES stations(id) ON DELETE CASCADE,
    embedding vector({dimension}) NOT NULL,
    fields_hash text NOT NULL CHECK (fields_hash ~ '^[0-9a-f]{{64}}$'),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (vector_norm(embedding) > 0)
)
"#,
        ))
        .execute(&mut self.connection)
        .await
        .map_err(|_| EmbeddingStoreError::safe("versioned embedding table creation failed"))?;
        sqlx::query(&format!(
            "CREATE INDEX IF NOT EXISTS {index} ON {table} USING hnsw (embedding vector_cosine_ops)",
        ))
        .execute(&mut self.connection)
        .await
        .map_err(|_| EmbeddingStoreError::safe("versioned embedding index creation failed"))?;

        let expected_type = format!("vector({dimension})");
        let schema = sqlx::query(
            r#"
SELECT
  (SELECT format_type(a.atttypid, a.atttypmod)
     FROM pg_attribute a
    WHERE a.attrelid = $1::regclass AND a.attname = 'embedding' AND a.attnum > 0) AS embedding_type,
  (SELECT EXISTS (
     SELECT 1 FROM pg_index i
     JOIN pg_class c ON c.oid = i.indexrelid
     JOIN pg_am am ON am.oid = c.relam
    WHERE i.indrelid = $1::regclass AND am.amname = 'hnsw'
      AND i.indisvalid AND i.indisready
      AND pg_get_indexdef(i.indexrelid) LIKE '%vector_cosine_ops%')) AS hnsw_ready,
  (SELECT EXISTS (
     SELECT 1 FROM pg_index i
    WHERE i.indrelid = $1::regclass AND i.indisprimary)) AS primary_key_ready
"#,
        )
        .bind(&table)
        .fetch_one(&mut self.connection)
        .await
        .map_err(|_| EmbeddingStoreError::safe("versioned embedding schema check failed"))?;
        let embedding_type: Option<String> = row_string(&schema, "embedding_type");
        let hnsw_ready: bool = row_bool(&schema, "hnsw_ready");
        let primary_key_ready: bool = row_bool(&schema, "primary_key_ready");
        if embedding_type.as_deref() != Some(expected_type.as_str())
            || !hnsw_ready
            || !primary_key_ready
        {
            return Err(EmbeddingStoreError::safe(
                "versioned embedding table schema is incompatible with this build",
            ));
        }
        self.dimension = Some(dimension);
        Ok(())
    }

    /// Blocks until the per-table advisory backfill lock is held.
    ///
    /// The lock is session-scoped on the dedicated connection, so a second
    /// lifecycle waits here and a crashed holder releases on disconnect.
    pub async fn acquire_advisory_lock(&mut self, version: u32) -> Result<(), EmbeddingStoreError> {
        sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
            .bind(embeddings_table_name(version))
            .execute(&mut self.connection)
            .await
            .map_err(|_| EmbeddingStoreError::safe("versioned embedding lock failed"))?;
        Ok(())
    }

    /// Reads the next page of stations needing a vector for this table version.
    ///
    /// A station is pending while it has no row, or its row's `fields_hash`
    /// differs from the current `stations.fields_hash` (including `NULL`,
    /// which legacy rows have until they are stamped).
    pub async fn pending_stations(
        &mut self,
        version: u32,
        limit: usize,
    ) -> Result<Vec<PendingStationEmbedding>, EmbeddingStoreError> {
        let table = embeddings_table_name(version);
        let limit = i64::try_from(limit)
            .map_err(|_| EmbeddingStoreError::safe("embedding page size exceeded bigint"))?;
        let rows = sqlx::query(&format!(
            r#"
SELECT s.id AS station_id, s.name, s.tags, s.language, s.country_code
FROM stations AS s
LEFT JOIN {table} AS e ON e.station_id = s.id
WHERE s.retired_at IS NULL
  AND (e.station_id IS NULL OR e.fields_hash IS DISTINCT FROM s.fields_hash)
ORDER BY s.id ASC
LIMIT $1
"#,
        ))
        .bind(limit)
        .fetch_all(&mut self.connection)
        .await
        .map_err(|_| EmbeddingStoreError::safe("versioned embedding pending read failed"))?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let name: String = row.get("name");
                let tags: Vec<String> = row.get("tags");
                let language: Option<String> = row.get("language");
                let country_code: Option<String> = row.get("country_code");
                PendingStationEmbedding {
                    station_id: row.get("station_id"),
                    fields_hash: station_fields_hash(
                        &name,
                        &tags,
                        language.as_deref(),
                        country_code.as_deref(),
                    ),
                    name,
                    tags,
                    language,
                    country_code,
                }
            })
            .collect())
    }

    /// Persists one batch of vectors under the import-race rule.
    ///
    /// In one short transaction: stations with a `NULL` hash are stamped with
    /// the hash that was sent (legacy rows predating this build), then each
    /// vector row is written only while the station's current
    /// `fields_hash` still equals the sent hash. A concurrent import that
    /// changed the fields wins and the vector is silently skipped; the station
    /// stays pending and is re-embedded on the next run. Returns the number of
    /// vectors actually written.
    pub async fn write_embeddings(
        &mut self,
        version: u32,
        batch: &[WrittenStationEmbedding],
    ) -> Result<usize, EmbeddingStoreError> {
        if batch.is_empty() {
            return Ok(0);
        }
        let table = embeddings_table_name(version);
        let dimension = self.dimension.ok_or_else(|| {
            EmbeddingStoreError::safe("versioned embedding table was not ensured before writing")
        })?;
        let ids: Vec<&str> = batch.iter().map(|row| row.station_id.as_str()).collect();
        let hashes: Vec<&str> = batch.iter().map(|row| row.fields_hash.as_str()).collect();
        let vectors: Vec<String> = batch
            .iter()
            .map(|row| vector_literal(&row.values))
            .collect();
        let mut transaction = self
            .connection
            .begin()
            .await
            .map_err(|_| EmbeddingStoreError::safe("versioned embedding transaction failed"))?;
        sqlx::query(
            r#"
UPDATE stations AS s
SET fields_hash = data.fields_hash, updated_at = now()
FROM unnest($1::text[], $2::text[]) AS data(station_id, fields_hash)
WHERE s.id = data.station_id AND s.fields_hash IS NULL
"#,
        )
        .bind(&ids)
        .bind(&hashes)
        .execute(&mut *transaction)
        .await
        .map_err(|_| EmbeddingStoreError::safe("versioned embedding hash stamp failed"))?;
        let written = sqlx::query(&format!(
            r#"
INSERT INTO {table} AS target (station_id, embedding, fields_hash)
SELECT s.id, data.vector::vector({dimension}), data.fields_hash
FROM unnest($1::text[], $2::text[], $3::text[]) AS data(station_id, fields_hash, vector)
JOIN stations AS s ON s.id = data.station_id AND s.fields_hash = data.fields_hash
ON CONFLICT (station_id) DO UPDATE SET
    embedding = EXCLUDED.embedding,
    fields_hash = EXCLUDED.fields_hash,
    created_at = now()
"#,
        ))
        .bind(&ids)
        .bind(&hashes)
        .bind(&vectors)
        .execute(&mut *transaction)
        .await
        .map_err(|_| EmbeddingStoreError::safe("versioned embedding write failed"))?
        .rows_affected();
        transaction
            .commit()
            .await
            .map_err(|_| EmbeddingStoreError::safe("versioned embedding commit failed"))?;
        usize::try_from(written)
            .map_err(|_| EmbeddingStoreError::safe("versioned embedding count overflow"))
    }

    /// Returns active stations and how many have a current-hash vector.
    pub async fn coverage(&mut self, version: u32) -> Result<(i64, i64), EmbeddingStoreError> {
        let table = embeddings_table_name(version);
        sqlx::query(&format!(
            r#"
SELECT count(*) AS active,
       count(*) FILTER (WHERE e.station_id IS NOT NULL AND e.fields_hash = s.fields_hash) AS covered
FROM stations AS s
LEFT JOIN {table} AS e ON e.station_id = s.id
WHERE s.retired_at IS NULL
"#,
        ))
        .fetch_one(&mut self.connection)
        .await
        .map(|row| (row.get::<i64, _>("active"), row.get::<i64, _>("covered")))
        .map_err(|_| EmbeddingStoreError::safe("versioned embedding coverage check failed"))
    }
}

/// Reads a nullable text column without panicking on type drift.
fn row_string(row: &sqlx::postgres::PgRow, column: &str) -> Option<String> {
    row.try_get::<Option<String>, _>(column).ok().flatten()
}

/// Reads a boolean column defaulting to `false` on type drift.
fn row_bool(row: &sqlx::postgres::PgRow, column: &str) -> bool {
    row.try_get::<bool, _>(column).unwrap_or(false)
}

/// Formats finite f32 components as a pgvector input literal.
fn vector_literal(values: &[f32]) -> String {
    let joined = values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("[{joined}]")
}
