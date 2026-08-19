//! PostgreSQL import persistence isolated from the HTTP search repository.

use async_trait::async_trait;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::catalog_import::{
    CatalogImportError, CatalogImportStore, ImportCounts, ImportedStation,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// PostgreSQL store for provider-owned catalog upserts and import-run bookkeeping.
#[derive(Clone, Debug)]
pub struct PostgresImportStore {
    pool: PgPool,
}

impl PostgresImportStore {
    /// Connects to PostgreSQL and applies pending migrations before an import starts.
    pub async fn connect(database_url: &str) -> Result<Self, CatalogImportError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|_| CatalogImportError::safe("PostgreSQL importer connection failed"))?;
        if MIGRATOR.run(&pool).await.is_err() {
            pool.close().await;
            return Err(CatalogImportError::safe(
                "PostgreSQL importer migration failed",
            ));
        }
        Ok(Self { pool })
    }

    /// Closes the importer's shared connection pool after in-flight work completes.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[async_trait]
impl CatalogImportStore for PostgresImportStore {
    async fn start_run(&self, source: &str) -> Result<String, CatalogImportError> {
        let run_id = Uuid::new_v4();
        sqlx::query("INSERT INTO import_runs (id, source, status) VALUES ($1, $2, 'started')")
            .bind(run_id)
            .bind(source)
            .execute(&self.pool)
            .await
            .map_err(|_| CatalogImportError::safe("PostgreSQL import run creation failed"))?;
        Ok(run_id.to_string())
    }

    async fn upsert_batch(
        &self,
        run_id: &str,
        source: &str,
        stations: &[ImportedStation],
    ) -> Result<usize, CatalogImportError> {
        let run_id = parse_run_id(run_id)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| CatalogImportError::safe("PostgreSQL import transaction failed"))?;
        // Bind every write to the source recorded by this still-active run.
        let owns_run = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM import_runs WHERE id = $1 AND source = $2 AND status = 'started')",
        )
        .bind(run_id)
        .bind(source)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| CatalogImportError::safe("PostgreSQL import ownership check failed"))?;
        if !owns_run {
            return Err(CatalogImportError::safe(
                "PostgreSQL import run ownership mismatch",
            ));
        }

        for station in stations {
            sqlx::query(
                r#"
INSERT INTO stations (
    id, source, source_station_id, name, homepage_url, tags, language, country_code,
    searchable_text, last_import_run_id
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
ON CONFLICT (source, source_station_id) DO UPDATE SET
    name = EXCLUDED.name,
    homepage_url = EXCLUDED.homepage_url,
    tags = EXCLUDED.tags,
    language = EXCLUDED.language,
    country_code = EXCLUDED.country_code,
    searchable_text = EXCLUDED.searchable_text,
    last_import_run_id = EXCLUDED.last_import_run_id,
    updated_at = now()
"#,
            )
            .bind(&station.id)
            .bind(source)
            .bind(&station.source_station_id)
            .bind(&station.name)
            .bind(&station.homepage_url)
            .bind(&station.tags)
            .bind(&station.language)
            .bind(&station.country_code)
            .bind(searchable_text(station))
            .bind(run_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| CatalogImportError::safe("PostgreSQL station upsert failed"))?;

            sqlx::query(
                r#"
INSERT INTO station_streams (
    station_id, source, source_stream_id, stream_url, codec, bitrate_kbps, health,
    is_primary, last_import_run_id
)
VALUES ($1, $2, $3, $4, $5, $6, 'healthy', true, $7)
ON CONFLICT (source, source_stream_id) DO UPDATE SET
    station_id = EXCLUDED.station_id,
    stream_url = EXCLUDED.stream_url,
    codec = EXCLUDED.codec,
    bitrate_kbps = EXCLUDED.bitrate_kbps,
    health = EXCLUDED.health,
    is_primary = EXCLUDED.is_primary,
    last_import_run_id = EXCLUDED.last_import_run_id,
    updated_at = now()
"#,
            )
            .bind(&station.id)
            .bind(source)
            .bind(&station.source_station_id)
            .bind(&station.stream_url)
            .bind(&station.codec)
            .bind(station.bitrate_kbps.map(|bitrate| bitrate as i32))
            .bind(run_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| CatalogImportError::safe("PostgreSQL stream upsert failed"))?;
        }

        transaction
            .commit()
            .await
            .map_err(|_| CatalogImportError::safe("PostgreSQL import transaction commit failed"))?;
        Ok(stations.len())
    }

    async fn complete_run(
        &self,
        run_id: &str,
        counts: ImportCounts,
    ) -> Result<(), CatalogImportError> {
        finish_run(&self.pool, run_id, "completed", counts, None).await
    }

    async fn fail_run(
        &self,
        run_id: &str,
        counts: ImportCounts,
        error_summary: &str,
    ) -> Result<(), CatalogImportError> {
        let summary = error_summary.chars().take(500).collect::<String>();
        finish_run(&self.pool, run_id, "failed", counts, Some(&summary)).await
    }
}

/// Builds the bounded, normalized document used by local embedding backfills
/// and PostgreSQL full-text search. CamelCase names are split so that
/// `"radioDJ"` produces both `"radiodj"` and `"radio dj"`.
fn searchable_text(station: &ImportedStation) -> String {
    use crate::search::tokenize;

    let tags = station.tags.join(" ");
    let name_tokens = tokenize(&station.name).join(" ");
    [
        station.name.as_str(),
        name_tokens.as_str(),
        tags.as_str(),
        station.language.as_deref().unwrap_or_default(),
        station.country_code.as_deref().unwrap_or_default(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

// Finalization only transitions an existing started run, preventing accidental rewrites.
async fn finish_run(
    pool: &PgPool,
    run_id: &str,
    status: &str,
    counts: ImportCounts,
    error_summary: Option<&str>,
) -> Result<(), CatalogImportError> {
    let run_id = parse_run_id(run_id)?;
    let fetched = count_to_i64(counts.fetched)?;
    let imported = count_to_i64(counts.imported)?;
    let skipped = count_to_i64(counts.skipped)?;
    let failed = count_to_i64(counts.failed)?;
    let result = sqlx::query(
        r#"
UPDATE import_runs
SET status = $2,
    fetched_count = $3,
    imported_count = $4,
    skipped_count = $5,
    failed_count = $6,
    error_summary = $7,
    completed_at = now()
WHERE id = $1 AND status = 'started'
"#,
    )
    .bind(run_id)
    .bind(status)
    .bind(fetched)
    .bind(imported)
    .bind(skipped)
    .bind(failed)
    .bind(error_summary)
    .execute(pool)
    .await
    .map_err(|_| CatalogImportError::safe("PostgreSQL import run finalization failed"))?;
    if result.rows_affected() != 1 {
        return Err(CatalogImportError::safe(
            "PostgreSQL import run was not in the started state",
        ));
    }
    Ok(())
}

fn parse_run_id(run_id: &str) -> Result<Uuid, CatalogImportError> {
    Uuid::parse_str(run_id)
        .map_err(|_| CatalogImportError::safe("Import run identifier was invalid"))
}

fn count_to_i64(count: usize) -> Result<i64, CatalogImportError> {
    i64::try_from(count).map_err(|_| CatalogImportError::safe("Import count exceeded bigint"))
}
