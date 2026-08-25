//! PostgreSQL import persistence isolated from the HTTP search repository.

use async_trait::async_trait;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::catalog::{
    CatalogImportError, CatalogImportStore, ImportCounts, ImportedStation, PinnedSharedCatalog,
    ROCKCATALOG_SOURCE,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// PostgreSQL store for provider-owned catalog upserts and import-run bookkeeping.
#[derive(Clone, Debug)]
pub struct PostgresImportStore {
    pool: PgPool,
}

impl PostgresImportStore {
    /// Reuses an already-migrated pool for repository bootstrap activation.
    pub(crate) fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

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

    /// Atomically activates a fully preflighted shared baseline release.
    ///
    /// Successful upserts, provider-scoped retirement, and run completion commit together. A
    /// failed transaction leaves the prior active baseline untouched and records a failed run.
    pub async fn activate_shared_catalog(
        &self,
        catalog: &PinnedSharedCatalog,
    ) -> Result<String, CatalogImportError> {
        let run_id = self.start_run(ROCKCATALOG_SOURCE).await?;
        let counts = ImportCounts {
            fetched: catalog.stations().len(),
            ..ImportCounts::default()
        };
        let result = self
            .activate_shared_catalog_in_transaction(&run_id, catalog, counts)
            .await;
        match result {
            Ok(()) => Ok(run_id),
            Err(error) => {
                self.fail_run(&run_id, counts, error.safe_summary()).await?;
                Err(error)
            }
        }
    }

    /// Looks up the active lifecycle meaning for a retired RockCatalog identity.
    ///
    /// Only merges are returned as redirects. Splits remain explicitly ambiguous, so callers
    /// cannot silently move persisted user state to an arbitrary replacement.
    pub async fn lookup_shared_catalog_replacement(
        &self,
        station_id: &str,
    ) -> Result<OwnedCatalogReplacement, CatalogImportError> {
        let row = sqlx::query_as::<_, TombstoneRow>(
            "SELECT reason, replacement_ids FROM catalog_tombstones WHERE source = $1 AND retired_station_id = $2",
        )
        .bind(ROCKCATALOG_SOURCE)
        .bind(station_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CatalogImportError::safe("PostgreSQL tombstone lookup failed"))?;
        Ok(match row {
            None => OwnedCatalogReplacement::Unknown,
            Some(TombstoneRow { reason, .. }) if reason == "removed" => {
                OwnedCatalogReplacement::Removed
            }
            Some(TombstoneRow {
                reason,
                replacement_ids,
            }) if reason == "merged" => OwnedCatalogReplacement::Redirect(
                replacement_ids
                    .into_iter()
                    .next()
                    .expect("database constraint requires one merge replacement"),
            ),
            Some(TombstoneRow {
                reason,
                replacement_ids,
            }) if reason == "split" => OwnedCatalogReplacement::Ambiguous(replacement_ids),
            Some(_) => {
                return Err(CatalogImportError::safe(
                    "PostgreSQL tombstone data was invalid",
                ));
            }
        })
    }

    async fn activate_shared_catalog_in_transaction(
        &self,
        run_id: &str,
        catalog: &PinnedSharedCatalog,
        mut counts: ImportCounts,
    ) -> Result<(), CatalogImportError> {
        let run_id = parse_run_id(run_id)?;
        let mut transaction = self.pool.begin().await.map_err(|_| {
            CatalogImportError::safe("PostgreSQL shared catalog transaction failed")
        })?;
        ensure_started_run(&mut transaction, run_id, ROCKCATALOG_SOURCE).await?;
        upsert_stations(
            &mut transaction,
            run_id,
            ROCKCATALOG_SOURCE,
            catalog.stations(),
        )
        .await?;
        replace_tombstones(&mut transaction, catalog).await?;
        let station_ids = catalog
            .stations()
            .iter()
            .map(|station| station.source_station_id.as_str())
            .collect::<Vec<_>>();
        let stream_ids = catalog
            .stations()
            .iter()
            .flat_map(|station| {
                station
                    .streams
                    .iter()
                    .map(|stream| stream.source_stream_id.as_str())
            })
            .collect::<Vec<_>>();
        sqlx::query("UPDATE station_streams SET retired_at = now(), is_primary = false, updated_at = now() WHERE source = $1 AND NOT (source_stream_id = ANY($2::text[])) AND retired_at IS NULL")
            .bind(ROCKCATALOG_SOURCE).bind(&stream_ids).execute(&mut *transaction).await
            .map_err(|_| CatalogImportError::safe("PostgreSQL shared stream retirement failed"))?;
        sqlx::query("UPDATE stations SET retired_at = now(), updated_at = now() WHERE source = $1 AND NOT (source_station_id = ANY($2::text[])) AND retired_at IS NULL")
            .bind(ROCKCATALOG_SOURCE).bind(&station_ids).execute(&mut *transaction).await
            .map_err(|_| CatalogImportError::safe("PostgreSQL shared station retirement failed"))?;
        counts.imported = catalog.stations().len();
        finish_run_in_transaction(&mut transaction, run_id, "completed", counts, None).await?;
        transaction.commit().await.map_err(|_| {
            CatalogImportError::safe("PostgreSQL shared catalog transaction commit failed")
        })
    }
}

/// Owned persistence result for a canonical lifecycle lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnedCatalogReplacement {
    /// No active lifecycle entry exists for the supplied ID.
    Unknown,
    /// The ID is retired without a successor.
    Removed,
    /// A merge allows callers to follow this sole successor.
    Redirect(String),
    /// A split requires the caller to select a successor explicitly.
    Ambiguous(Vec<String>),
}

#[derive(sqlx::FromRow)]
struct TombstoneRow {
    reason: String,
    replacement_ids: Vec<String>,
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
        ensure_started_run(&mut transaction, run_id, source).await?;
        upsert_stations(&mut transaction, run_id, source, stations).await?;

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

async fn ensure_started_run(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    source: &str,
) -> Result<(), CatalogImportError> {
    let owns_run = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM import_runs WHERE id = $1 AND source = $2 AND status = 'started')",
    )
    .bind(run_id)
    .bind(source)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| CatalogImportError::safe("PostgreSQL import ownership check failed"))?;
    if owns_run {
        Ok(())
    } else {
        Err(CatalogImportError::safe(
            "PostgreSQL import run ownership mismatch",
        ))
    }
}

/// Upserts only the supplied source namespace and preserves unrelated provider rows.
async fn upsert_stations(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    source: &str,
    stations: &[ImportedStation],
) -> Result<(), CatalogImportError> {
    for station in stations {
        if station.streams.is_empty()
            || station
                .streams
                .iter()
                .filter(|stream| stream.is_primary)
                .count()
                != 1
        {
            return Err(CatalogImportError::safe(
                "Catalog station streams were invalid",
            ));
        }
        let collision = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM stations WHERE id = $1 AND (source <> $2 OR source_station_id <> $3))",
        )
        .bind(&station.id).bind(source).bind(&station.source_station_id)
        .fetch_one(&mut **transaction).await
        .map_err(|_| CatalogImportError::safe("PostgreSQL station collision check failed"))?;
        if collision {
            return Err(CatalogImportError::safe(
                "Catalog station ID collides with another provider",
            ));
        }
        let metadata_changed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM stations WHERE source = $1 AND source_station_id = $2 AND (name IS DISTINCT FROM $3 OR homepage_url IS DISTINCT FROM $4 OR tags IS DISTINCT FROM $5 OR language IS DISTINCT FROM $6 OR country_code IS DISTINCT FROM $7))",
        )
        .bind(source).bind(&station.source_station_id).bind(&station.name).bind(&station.homepage_url)
        .bind(&station.tags).bind(&station.language).bind(&station.country_code)
        .fetch_one(&mut **transaction).await
        .map_err(|_| CatalogImportError::safe("PostgreSQL station metadata check failed"))?;
        sqlx::query(
            r#"
INSERT INTO stations (id, source, source_station_id, name, homepage_url, tags, language, country_code, searchable_text, last_import_run_id, retired_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL)
ON CONFLICT (source, source_station_id) DO UPDATE SET
    name = EXCLUDED.name, homepage_url = EXCLUDED.homepage_url, tags = EXCLUDED.tags,
    language = EXCLUDED.language, country_code = EXCLUDED.country_code,
    searchable_text = EXCLUDED.searchable_text, last_import_run_id = EXCLUDED.last_import_run_id,
    retired_at = NULL, updated_at = now()
"#,
        )
        .bind(&station.id).bind(source).bind(&station.source_station_id).bind(&station.name)
        .bind(&station.homepage_url).bind(&station.tags).bind(&station.language).bind(&station.country_code)
        .bind(searchable_text(station)).bind(run_id).execute(&mut **transaction).await
        .map_err(|_| CatalogImportError::safe("PostgreSQL station upsert failed"))?;
        if metadata_changed {
            sqlx::query("DELETE FROM station_embeddings WHERE station_id = $1")
                .bind(&station.id)
                .execute(&mut **transaction)
                .await
                .map_err(|_| {
                    CatalogImportError::safe("PostgreSQL embedding invalidation failed")
                })?;
        }
        // Clear current primary flags before choosing the replacement primary stream.
        sqlx::query(
            "UPDATE station_streams SET is_primary = false WHERE station_id = $1 AND source = $2",
        )
        .bind(&station.id)
        .bind(source)
        .execute(&mut **transaction)
        .await
        .map_err(|_| CatalogImportError::safe("PostgreSQL stream primary reset failed"))?;
        for stream in &station.streams {
            sqlx::query(
                r#"
INSERT INTO station_streams (station_id, source, source_stream_id, stream_url, codec, bitrate_kbps, health, is_primary, last_import_run_id, retired_at)
VALUES ($1, $2, $3, $4, $5, $6, 'unknown', $7, $8, NULL)
ON CONFLICT (source, source_stream_id) DO UPDATE SET
    station_id = EXCLUDED.station_id, stream_url = EXCLUDED.stream_url, codec = EXCLUDED.codec,
    bitrate_kbps = EXCLUDED.bitrate_kbps,
    health = CASE WHEN station_streams.stream_url IS DISTINCT FROM EXCLUDED.stream_url THEN 'unknown' ELSE station_streams.health END,
    last_probe_at = CASE WHEN station_streams.stream_url IS DISTINCT FROM EXCLUDED.stream_url THEN NULL ELSE station_streams.last_probe_at END,
    last_probe_error = CASE WHEN station_streams.stream_url IS DISTINCT FROM EXCLUDED.stream_url THEN NULL ELSE station_streams.last_probe_error END,
    is_primary = EXCLUDED.is_primary, last_import_run_id = EXCLUDED.last_import_run_id,
    retired_at = NULL, updated_at = now()
"#,
            )
            .bind(&station.id).bind(source).bind(&stream.source_stream_id).bind(&stream.stream_url)
            .bind(&stream.codec).bind(stream.bitrate_kbps.map(|bitrate| bitrate as i32))
            .bind(stream.is_primary).bind(run_id).execute(&mut **transaction).await
            .map_err(|_| CatalogImportError::safe("PostgreSQL stream upsert failed"))?;
        }
    }
    Ok(())
}

/// Replaces the active release's lifecycle view in the activation transaction.
async fn replace_tombstones(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    catalog: &PinnedSharedCatalog,
) -> Result<(), CatalogImportError> {
    sqlx::query("DELETE FROM catalog_tombstones WHERE source = $1")
        .bind(ROCKCATALOG_SOURCE)
        .execute(&mut **transaction)
        .await
        .map_err(|_| CatalogImportError::safe("PostgreSQL tombstone replacement failed"))?;
    for tombstone in catalog.tombstones() {
        sqlx::query(
            "INSERT INTO catalog_tombstones (source, retired_station_id, reason, replacement_ids, catalog_version) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(ROCKCATALOG_SOURCE)
        .bind(&tombstone.id)
        .bind(tombstone.reason.as_str())
        .bind(&tombstone.replacement_ids)
        .bind(catalog.catalog_version())
        .execute(&mut **transaction)
        .await
        .map_err(|_| CatalogImportError::safe("PostgreSQL tombstone upsert failed"))?;
    }
    Ok(())
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

/// Finalizes a run inside the same transaction as shared-catalog activation.
async fn finish_run_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    status: &str,
    counts: ImportCounts,
    error_summary: Option<&str>,
) -> Result<(), CatalogImportError> {
    let result = sqlx::query(
        "UPDATE import_runs SET status = $2, fetched_count = $3, imported_count = $4, skipped_count = $5, failed_count = $6, error_summary = $7, completed_at = now() WHERE id = $1 AND status = 'started'",
    )
    .bind(run_id)
    .bind(status)
    .bind(count_to_i64(counts.fetched)?)
    .bind(count_to_i64(counts.imported)?)
    .bind(count_to_i64(counts.skipped)?)
    .bind(count_to_i64(counts.failed)?)
    .bind(error_summary)
    .execute(&mut **transaction)
    .await
    .map_err(|_| CatalogImportError::safe("PostgreSQL import run finalization failed"))?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(CatalogImportError::safe(
            "PostgreSQL import run was not in the started state",
        ))
    }
}

fn parse_run_id(run_id: &str) -> Result<Uuid, CatalogImportError> {
    Uuid::parse_str(run_id)
        .map_err(|_| CatalogImportError::safe("Import run identifier was invalid"))
}

fn count_to_i64(count: usize) -> Result<i64, CatalogImportError> {
    i64::try_from(count).map_err(|_| CatalogImportError::safe("Import count exceeded bigint"))
}
