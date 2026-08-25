//! Create an explicit, immutable RockMobile extended-catalog release from PostgreSQL.
//!
//! A database with fewer than the configured full-catalog threshold produces only a truthful gap
//! report. It never creates a partial SQLite file that could be bundled as a complete release.

use std::{
    env,
    error::Error,
    io::{Error as IoError, ErrorKind},
    path::PathBuf,
};

use rockserver::{
    mobile_export::{
        DEFAULT_MIN_STATION_COUNT, MobileExportConfig, MobileExportOutcome, export_mobile_catalog,
    },
    persistence::DATABASE_URL_ENV,
    telemetry,
};
use sqlx::postgres::PgPoolOptions;

const VERSION_ENV: &str = "ROCKSERVER_MOBILE_EXPORT_VERSION";
const OUTPUT_DIR_ENV: &str = "ROCKSERVER_MOBILE_EXPORT_DIR";
const DEFAULT_OUTPUT_DIR: &str = "release/mobile-catalog";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // Local development may keep the PostgreSQL URL in an ignored `.env`; its value is never logged.
    dotenvy::dotenv().ok();
    telemetry::init()?;
    let config = config_from_env()?;
    let database_url = required_database_url()?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    match export_mobile_catalog(&pool, &config).await? {
        MobileExportOutcome::Released(artifact) => tracing::info!(
            database = %artifact.database_path.display(),
            manifest = %artifact.manifest_path.display(),
            report = %artifact.eligibility_report_path.display(),
            stations = artifact.station_count,
            sha256 = %artifact.sha256,
            "RockMobile extended SQLite catalog released"
        ),
        MobileExportOutcome::Gap(report) => tracing::warn!(
            report = %config.output_dir.join(format!("rockmobile-extended-{}.gap-report.json", config.catalog_version)).display(),
            stations = report.eligible_station_count,
            required = report.required_minimum_station_count,
            status = %report.status,
            "RockMobile extended SQLite catalog was not created"
        ),
    }
    pool.close().await;
    Ok(())
}

/// Loads the explicit release settings; a version is never inferred from wall-clock time.
fn config_from_env() -> Result<MobileExportConfig, Box<dyn Error + Send + Sync>> {
    let catalog_version =
        env::var(VERSION_ENV).map_err(|_| invalid_input(format!("{VERSION_ENV} is required")))?;
    let output_dir =
        PathBuf::from(env::var(OUTPUT_DIR_ENV).unwrap_or_else(|_| DEFAULT_OUTPUT_DIR.to_owned()));
    Ok(MobileExportConfig {
        catalog_version,
        output_dir,
        minimum_station_count: DEFAULT_MIN_STATION_COUNT,
    })
}

/// Returns a database URL without ever placing its value in an error string.
fn required_database_url() -> Result<String, Box<dyn Error + Send + Sync>> {
    match env::var(DATABASE_URL_ENV) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => {
            Err(invalid_input(format!("{DATABASE_URL_ENV} is required")))
        }
        Err(env::VarError::NotUnicode(_)) => Err(invalid_input(format!(
            "{DATABASE_URL_ENV} must contain valid Unicode"
        ))),
    }
}

/// Converts configuration mistakes to a non-sensitive command-line error.
fn invalid_input(message: String) -> Box<dyn Error + Send + Sync> {
    Box::new(IoError::new(ErrorKind::InvalidInput, message))
}
