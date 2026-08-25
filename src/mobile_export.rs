//! Deterministic, sanitized SQLite release export for RockMobile.
//!
//! The export deliberately contains only client discovery and playback fields. It never copies
//! health/probe data, embeddings, import-run metadata, credentials, or provider-private
//! operational data from PostgreSQL.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{
    Connection, FromRow, PgPool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};

/// SQLite user-version and compatible RockMobile schema revision.
pub const MOBILE_SCHEMA_VERSION: u32 = 1;
/// Default minimum eligible row count required before a full mobile release is emitted.
pub const DEFAULT_MIN_STATION_COUNT: usize = 16_000;

/// Immutable settings for one explicit mobile export action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MobileExportConfig {
    /// Separately versioned extended mobile release identifier.
    pub catalog_version: String,
    /// Directory in which immutable release artifacts are created.
    pub output_dir: PathBuf,
    /// Fixed guard against publishing a development-sized database as the complete catalog.
    ///
    /// This must equal [`DEFAULT_MIN_STATION_COUNT`]; it is explicit here so callers and tests
    /// cannot silently weaken the release policy.
    pub minimum_station_count: usize,
}

/// Outcome of checking an active PostgreSQL catalog for mobile-export eligibility.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MobileExportOutcome {
    /// A complete prebuilt SQLite release and its deterministic reports were written.
    Released(MobileReleaseArtifacts),
    /// The database is below the configured full-catalog gate; no SQLite artifact was created.
    Gap(MobileGapReport),
}

/// Paths and verified metadata for an immutable mobile release artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MobileReleaseArtifacts {
    /// Android/Room-bundleable SQLite database.
    pub database_path: PathBuf,
    /// JSON manifest that hashes the exact SQLite bytes.
    pub manifest_path: PathBuf,
    /// Machine-readable eligibility and exclusion report.
    pub eligibility_report_path: PathBuf,
    /// Number of exported stations.
    pub station_count: usize,
    /// SHA-256 of the exact database file bytes.
    pub sha256: String,
}

/// Machine-readable evidence that no full release could truthfully be generated.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MobileGapReport {
    /// Stable report schema revision.
    pub report_schema_version: u32,
    /// Requested extended release identifier.
    pub catalog_version: String,
    /// Actual active/playable station count in the queried PostgreSQL catalog.
    pub eligible_station_count: usize,
    /// Required count for a full release.
    pub required_minimum_station_count: usize,
    /// Explicit result that prevents a partial database from being mislabeled as complete.
    pub status: String,
    /// Deterministic SQL eligibility, primary-selection, and coexistence policy.
    pub eligibility: EligibilityPolicy,
    /// Server-only fields intentionally absent from any SQLite output.
    pub excluded_server_only_fields: Vec<String>,
}

/// Deterministic eligibility policy recorded with every result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EligibilityPolicy {
    /// Station must not be provider-scoped soft-retired.
    pub station_active: bool,
    /// Primary stream must not be provider-scoped soft-retired.
    pub primary_stream_active: bool,
    /// Exactly one explicit active primary stream is required.
    pub exactly_one_primary_stream: bool,
    /// Stream URL must be non-empty and use HTTP or HTTPS.
    pub http_stream_url: bool,
    /// Rows are ordered by immutable RockServer station ID.
    pub ordering: String,
    /// No name/URL cross-provider merge is performed.
    pub deduplication: String,
}

impl EligibilityPolicy {
    fn strict() -> Self {
        Self {
            station_active: true,
            primary_stream_active: true,
            exactly_one_primary_stream: true,
            http_stream_url: true,
            ordering: "stations.id ASC".to_owned(),
            deduplication: "one row per stable stations.id; no cross-provider name or URL merge"
                .to_owned(),
        }
    }
}

/// Errors that are safe to display from the explicit export command.
#[derive(Debug)]
pub enum MobileExportError {
    /// PostgreSQL could not be queried.
    PostgreSql(sqlx::Error),
    /// SQLite could not be created or verified.
    Sqlite(sqlx::Error),
    /// Artifact directory or manifest could not be written.
    Io(std::io::Error),
    /// JSON serialization of deterministic metadata failed.
    Json(serde_json::Error),
    /// Export settings could create ambiguous or unsafe artifact paths.
    InvalidConfig(&'static str),
    /// The selected immutable artifact path already exists.
    ArtifactAlreadyExists(PathBuf),
    /// SQLite verification found an invariant violation.
    Verification(&'static str),
}

impl fmt::Display for MobileExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PostgreSql(_) => formatter.write_str("PostgreSQL mobile-export query failed"),
            Self::Sqlite(_) => formatter.write_str("SQLite mobile-export write failed"),
            Self::Io(_) => formatter.write_str("mobile-export artifact I/O failed"),
            Self::Json(_) => formatter.write_str("mobile-export metadata serialization failed"),
            Self::InvalidConfig(message) => {
                write!(formatter, "mobile-export configuration invalid: {message}")
            }
            Self::ArtifactAlreadyExists(path) => write!(
                formatter,
                "mobile-export artifact already exists: {}",
                path.display()
            ),
            Self::Verification(message) => {
                write!(formatter, "mobile-export verification failed: {message}")
            }
        }
    }
}

impl std::error::Error for MobileExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PostgreSql(error) | Self::Sqlite(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidConfig(_) | Self::ArtifactAlreadyExists(_) | Self::Verification(_) => None,
        }
    }
}

/// Exports a complete active/playable PostgreSQL catalog or writes a truthful deterministic gap report.
pub async fn export_mobile_catalog(
    pool: &PgPool,
    config: &MobileExportConfig,
) -> Result<MobileExportOutcome, MobileExportError> {
    validate_config(config)?;
    let stations = fetch_eligible_stations(pool).await?;
    if stations.len() < config.minimum_station_count {
        let report = gap_report(config, stations.len());
        let report_path = gap_report_path(config);
        // A gap report describes the current database, so reruns replace it rather than making
        // the release command fail on stale diagnostic evidence. Complete release artifacts stay
        // immutable below.
        write_json_replace(&report_path, &report)?;
        return Ok(MobileExportOutcome::Gap(report));
    }

    let database_path = database_path(config);
    let manifest_path = manifest_path(config);
    let report_path = eligibility_report_path(config);
    for path in [&database_path, &manifest_path, &report_path] {
        if path.exists() {
            return Err(MobileExportError::ArtifactAlreadyExists(path.clone()));
        }
    }
    write_sqlite_database(&database_path, &config.catalog_version, &stations).await?;
    let sha256 = file_sha256(&database_path)?;
    verify_sqlite_database(&database_path, stations.len(), &config.catalog_version).await?;
    let manifest = MobileManifest {
        manifest_schema_version: 1,
        catalog_version: config.catalog_version.clone(),
        database_schema_version: MOBILE_SCHEMA_VERSION,
        file: database_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        sha256: sha256.clone(),
        station_count: stations.len(),
    };
    write_json_new(&manifest_path, &manifest)?;
    let report = release_report(config, stations.len());
    write_json_new(&report_path, &report)?;
    Ok(MobileExportOutcome::Released(MobileReleaseArtifacts {
        database_path,
        manifest_path,
        eligibility_report_path: report_path,
        station_count: stations.len(),
        sha256,
    }))
}

#[derive(Debug, FromRow)]
struct PostgresMobileStation {
    station_id: String,
    source: String,
    source_station_id: String,
    name: String,
    tags: Vec<String>,
    country_code: Option<String>,
    language: Option<String>,
    homepage_url: Option<String>,
    stream_url: String,
    codec: Option<String>,
    bitrate_kbps: Option<i32>,
}

const ELIGIBLE_STATIONS_SQL: &str = r#"
SELECT
    s.id AS station_id,
    s.source,
    s.source_station_id,
    s.name,
    s.tags,
    s.country_code,
    s.language,
    s.homepage_url,
    ss.stream_url,
    ss.codec,
    ss.bitrate_kbps
FROM stations AS s
JOIN station_streams AS ss ON ss.station_id = s.id
WHERE s.retired_at IS NULL
  AND ss.retired_at IS NULL
  AND ss.is_primary
  AND btrim(ss.stream_url) <> ''
  AND lower(ss.stream_url) ~ '^https?://'
  AND 1 = (
      SELECT COUNT(*)
      FROM station_streams AS candidate
      WHERE candidate.station_id = s.id
        AND candidate.retired_at IS NULL
        AND candidate.is_primary
  )
ORDER BY s.id ASC
"#;

async fn fetch_eligible_stations(
    pool: &PgPool,
) -> Result<Vec<PostgresMobileStation>, MobileExportError> {
    sqlx::query_as(ELIGIBLE_STATIONS_SQL)
        .fetch_all(pool)
        .await
        .map_err(MobileExportError::PostgreSql)
}

async fn write_sqlite_database(
    path: &Path,
    catalog_version: &str,
    stations: &[PostgresMobileStation],
) -> Result<(), MobileExportError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Delete)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .map_err(MobileExportError::Sqlite)?;
    sqlx::query(
        "PRAGMA page_size = 4096; PRAGMA application_id = 1381124436; PRAGMA user_version = 1;",
    )
    .execute(&mut connection)
    .await
    .map_err(MobileExportError::Sqlite)?;
    sqlx::query(
        r#"
CREATE TABLE catalog_metadata (
    catalog_version TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    station_count INTEGER NOT NULL CHECK (station_count >= 0)
) STRICT;
CREATE TABLE stations (
    station_id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    source_station_id TEXT NOT NULL,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    tags_json TEXT NOT NULL,
    normalized_tags TEXT NOT NULL,
    country_code TEXT,
    language TEXT,
    homepage_url TEXT,
    favicon_url TEXT,
    stream_url TEXT NOT NULL,
    codec TEXT,
    bitrate_kbps INTEGER
) STRICT;
CREATE UNIQUE INDEX stations_source_identity_idx ON stations(source, source_station_id);
CREATE INDEX stations_normalized_name_idx ON stations(normalized_name, station_id);
CREATE INDEX stations_normalized_tags_idx ON stations(normalized_tags, station_id);
CREATE VIRTUAL TABLE station_search USING fts5(
    station_id UNINDEXED,
    normalized_name,
    normalized_tags,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#,
    )
    .execute(&mut connection)
    .await
    .map_err(MobileExportError::Sqlite)?;
    let mut transaction = connection
        .begin()
        .await
        .map_err(MobileExportError::Sqlite)?;
    sqlx::query("INSERT INTO catalog_metadata (catalog_version, schema_version, station_count) VALUES (?, ?, ?)")
        .bind(catalog_version)
        .bind(MOBILE_SCHEMA_VERSION as i64)
        .bind(stations.len() as i64)
        .execute(&mut *transaction)
        .await
        .map_err(MobileExportError::Sqlite)?;
    for station in stations {
        let normalized_name = normalize_name(&station.name);
        let tags = normalize_tags(&station.tags);
        let tags_json = serde_json::to_string(&tags).map_err(MobileExportError::Json)?;
        let normalized_tags = tags.join(" ");
        sqlx::query(
            "INSERT INTO stations (station_id, source, source_station_id, name, normalized_name, tags_json, normalized_tags, country_code, language, homepage_url, favicon_url, stream_url, codec, bitrate_kbps) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)",
        )
        .bind(&station.station_id).bind(&station.source).bind(&station.source_station_id)
        .bind(&station.name).bind(&normalized_name).bind(&tags_json).bind(&normalized_tags)
        .bind(&station.country_code).bind(&station.language).bind(&station.homepage_url)
        .bind(&station.stream_url).bind(&station.codec).bind(station.bitrate_kbps)
        .execute(&mut *transaction).await.map_err(MobileExportError::Sqlite)?;
        sqlx::query("INSERT INTO station_search (station_id, normalized_name, normalized_tags) VALUES (?, ?, ?)")
            .bind(&station.station_id).bind(&normalized_name).bind(&normalized_tags)
            .execute(&mut *transaction).await.map_err(MobileExportError::Sqlite)?;
    }
    transaction
        .commit()
        .await
        .map_err(MobileExportError::Sqlite)?;
    sqlx::query("VACUUM")
        .execute(&mut connection)
        .await
        .map_err(MobileExportError::Sqlite)?;
    connection.close().await.map_err(MobileExportError::Sqlite)
}

async fn verify_sqlite_database(
    path: &Path,
    expected_count: usize,
    expected_version: &str,
) -> Result<(), MobileExportError> {
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .map_err(MobileExportError::Sqlite)?;
    let integrity = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await
        .map_err(MobileExportError::Sqlite)?;
    if integrity != "ok" {
        return Err(MobileExportError::Verification(
            "PRAGMA integrity_check did not return ok",
        ));
    }
    let version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .map_err(MobileExportError::Sqlite)?;
    if version != MOBILE_SCHEMA_VERSION as i64 {
        return Err(MobileExportError::Verification(
            "SQLite user_version mismatched",
        ));
    }
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM stations")
        .fetch_one(&mut connection)
        .await
        .map_err(MobileExportError::Sqlite)?;
    let metadata = sqlx::query_as::<_, (String, i64)>(
        "SELECT catalog_version, station_count FROM catalog_metadata",
    )
    .fetch_one(&mut connection)
    .await
    .map_err(MobileExportError::Sqlite)?;
    if count != expected_count as i64
        || metadata.0 != expected_version
        || metadata.1 != expected_count as i64
    {
        return Err(MobileExportError::Verification(
            "SQLite metadata or station count mismatched",
        ));
    }
    let fts_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM station_search")
        .fetch_one(&mut connection)
        .await
        .map_err(MobileExportError::Sqlite)?;
    if fts_count != count {
        return Err(MobileExportError::Verification(
            "SQLite FTS index row count mismatched",
        ));
    }
    connection.close().await.map_err(MobileExportError::Sqlite)
}

#[derive(Serialize)]
struct MobileManifest {
    manifest_schema_version: u32,
    catalog_version: String,
    database_schema_version: u32,
    file: String,
    sha256: String,
    station_count: usize,
}

#[derive(Serialize)]
struct MobileReleaseReport {
    report_schema_version: u32,
    catalog_version: String,
    eligible_station_count: usize,
    status: String,
    eligibility: EligibilityPolicy,
    excluded_server_only_fields: Vec<String>,
}

fn release_report(config: &MobileExportConfig, count: usize) -> MobileReleaseReport {
    MobileReleaseReport {
        report_schema_version: 1,
        catalog_version: config.catalog_version.clone(),
        eligible_station_count: count,
        status: "released_complete_active_playable_catalog".to_owned(),
        eligibility: EligibilityPolicy::strict(),
        excluded_server_only_fields: excluded_fields(),
    }
}

fn gap_report(config: &MobileExportConfig, count: usize) -> MobileGapReport {
    MobileGapReport {
        report_schema_version: 1,
        catalog_version: config.catalog_version.clone(),
        eligible_station_count: count,
        required_minimum_station_count: config.minimum_station_count,
        status: "gap_insufficient_eligible_stations_no_sqlite_release_created".to_owned(),
        eligibility: EligibilityPolicy::strict(),
        excluded_server_only_fields: excluded_fields(),
    }
}

fn excluded_fields() -> Vec<String> {
    [
        "station_streams.health",
        "station_streams.last_probe_at",
        "station_streams.last_probe_error",
        "station_embeddings",
        "import_runs",
        "last_import_run_id",
        "created_at",
        "updated_at",
        "credentials",
        "provider-private operational metadata",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn validate_config(config: &MobileExportConfig) -> Result<(), MobileExportError> {
    if config.catalog_version.is_empty()
        || config.catalog_version.len() > 80
        || !config
            .catalog_version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(MobileExportError::InvalidConfig(
            "catalog version must be a safe release identifier",
        ));
    }
    if config.minimum_station_count != DEFAULT_MIN_STATION_COUNT {
        return Err(MobileExportError::InvalidConfig(
            "minimum station count must equal the fixed complete-catalog release gate",
        ));
    }
    std::fs::create_dir_all(&config.output_dir).map_err(MobileExportError::Io)
}

fn database_path(config: &MobileExportConfig) -> PathBuf {
    config.output_dir.join(format!(
        "rockmobile-extended-{}.sqlite",
        config.catalog_version
    ))
}
fn manifest_path(config: &MobileExportConfig) -> PathBuf {
    config.output_dir.join(format!(
        "rockmobile-extended-{}.manifest.json",
        config.catalog_version
    ))
}
fn eligibility_report_path(config: &MobileExportConfig) -> PathBuf {
    config.output_dir.join(format!(
        "rockmobile-extended-{}.eligibility-report.json",
        config.catalog_version
    ))
}
fn gap_report_path(config: &MobileExportConfig) -> PathBuf {
    config.output_dir.join(format!(
        "rockmobile-extended-{}.gap-report.json",
        config.catalog_version
    ))
}

fn normalize_name(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn normalize_tags(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| normalize_name(value))
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn file_sha256(path: &Path) -> Result<String, MobileExportError> {
    let bytes = std::fs::read(path).map_err(MobileExportError::Io)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn write_json_new(path: &Path, value: &impl Serialize) -> Result<(), MobileExportError> {
    if path.exists() {
        return Err(MobileExportError::ArtifactAlreadyExists(path.to_path_buf()));
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(MobileExportError::Json)?;
    std::fs::write(path, bytes).map_err(MobileExportError::Io)
}

/// Writes a reproducible diagnostic report for the latest observed eligibility state.
fn write_json_replace(path: &Path, value: &impl Serialize) -> Result<(), MobileExportError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(MobileExportError::Json)?;
    std::fs::write(path, bytes).map_err(MobileExportError::Io)
}

#[cfg(test)]
mod tests {
    use sqlx::Connection;

    use super::{
        DEFAULT_MIN_STATION_COUNT, MOBILE_SCHEMA_VERSION, MobileExportConfig, MobileExportError,
        PostgresMobileStation, file_sha256, normalize_name, normalize_tags, validate_config,
        verify_sqlite_database, write_sqlite_database,
    };

    #[test]
    fn normalizers_are_stable_for_mobile_search_keys() {
        assert_eq!("radio rock", normalize_name(" Radio   Rock "));
        assert_eq!(
            vec!["heavy metal", "rock"],
            normalize_tags(&[
                " Rock ".to_owned(),
                "heavy metal".to_owned(),
                "rock".to_owned()
            ])
        );
    }

    #[test]
    fn release_identifier_and_full_catalog_gate_are_strict() {
        let temp = std::env::temp_dir().join("rockserver-mobile-export-test");
        let config = MobileExportConfig {
            catalog_version: "2026.08.2-mobile.1".to_owned(),
            output_dir: temp,
            minimum_station_count: DEFAULT_MIN_STATION_COUNT,
        };
        assert!(validate_config(&config).is_ok());
        let invalid = MobileExportConfig {
            catalog_version: "../../unsafe".to_owned(),
            ..config
        };
        assert!(matches!(
            validate_config(&invalid),
            Err(MobileExportError::InvalidConfig(_))
        ));
        let invalid_gate = MobileExportConfig {
            catalog_version: "2026.08.2-mobile.1".to_owned(),
            output_dir: std::env::temp_dir().join("rockserver-mobile-export-test"),
            minimum_station_count: 1,
        };
        assert!(matches!(
            validate_config(&invalid_gate),
            Err(MobileExportError::InvalidConfig(_))
        ));
    }

    #[tokio::test]
    async fn sqlite_fixture_has_integrity_metadata_and_search_indexes() {
        let directory =
            std::env::temp_dir().join(format!("rockserver-mobile-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("fixture.sqlite");
        let stations = vec![PostgresMobileStation {
            station_id: "fixture-rock".to_owned(),
            source: "fixture".to_owned(),
            source_station_id: "fixture-rock".to_owned(),
            name: "Fixture Rock".to_owned(),
            tags: vec!["hard rock".to_owned(), "rock".to_owned()],
            country_code: Some("US".to_owned()),
            language: Some("en".to_owned()),
            homepage_url: None,
            stream_url: "https://example.com/fixture.mp3".to_owned(),
            codec: Some("mp3".to_owned()),
            bitrate_kbps: Some(128),
        }];
        write_sqlite_database(&path, "fixture.1", &stations)
            .await
            .unwrap();
        verify_sqlite_database(&path, 1, "fixture.1").await.unwrap();
        assert_eq!(64, file_sha256(&path).unwrap().len());
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true);
        let mut connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let index_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name IN ('stations_normalized_name_idx', 'stations_normalized_tags_idx')",
        )
        .fetch_one(&mut connection).await.unwrap();
        let version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(2, index_count);
        assert_eq!(MOBILE_SCHEMA_VERSION as i64, version);
        connection.close().await.unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&directory).unwrap();
    }
}
