//! Read-only adapter for the checksum-pinned complete station release.

use std::{env, path::PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{Connection, FromRow, sqlite::SqliteConnectOptions};

use super::{
    CatalogImportError, CatalogImportProvider, ImportPage, ImportedStation, ImportedStream,
};

/// One provider namespace for the immutable complete release imported at deployment time.
pub const FULL_RELEASE_SOURCE: &str = "rockserver-full-release";
/// Environment variable used by the production image to locate the bundled SQLite snapshot.
pub const FULL_RELEASE_PATH_ENV: &str = "ROCKSERVER_FULL_CATALOG_PATH";
const DEFAULT_RELEASE_PATH: &str =
    "release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.sqlite";
const MANIFEST_BYTES: &str = include_str!(
    "../../release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.manifest.json"
);
const MINIMUM_COMPLETE_STATION_COUNT: usize = 16_000;

/// Fully preflighted complete release, exposed through the normal bounded import boundary.
#[derive(Clone, Debug)]
pub struct FullReleaseCatalogAdapter {
    catalog_version: String,
    stations: Vec<ImportedStation>,
}

impl FullReleaseCatalogAdapter {
    /// Opens and validates the immutable release before any PostgreSQL write can start.
    pub async fn pinned() -> Result<Self, CatalogImportError> {
        let manifest: FullReleaseManifest = serde_json::from_str(MANIFEST_BYTES)
            .map_err(|_| CatalogImportError::safe("full catalog manifest is invalid"))?;
        if manifest.manifest_schema_version != 1
            || manifest.database_schema_version != 1
            || manifest.station_count < MINIMUM_COMPLETE_STATION_COUNT as u64
            || !valid_release_file_name(&manifest.file)
            || !valid_sha256(&manifest.sha256)
        {
            return Err(CatalogImportError::safe("full catalog manifest is invalid"));
        }
        let path = env::var(FULL_RELEASE_PATH_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_RELEASE_PATH));
        if path.file_name().and_then(|name| name.to_str()) != Some(manifest.file.as_str()) {
            return Err(CatalogImportError::safe(
                "full catalog path does not match the pinned release",
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|_| CatalogImportError::safe("full catalog release file could not be read"))?;
        if format!("{:x}", Sha256::digest(&bytes)) != manifest.sha256 {
            return Err(CatalogImportError::safe(
                "full catalog release checksum mismatch",
            ));
        }

        let options = SqliteConnectOptions::new().filename(&path).read_only(true);
        let mut connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .map_err(|_| CatalogImportError::safe("full catalog SQLite could not be opened"))?;
        let integrity = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&mut connection)
            .await
            .map_err(|_| CatalogImportError::safe("full catalog SQLite integrity check failed"))?;
        let schema_version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(&mut connection)
            .await
            .map_err(|_| CatalogImportError::safe("full catalog SQLite schema check failed"))?;
        let metadata = sqlx::query_as::<_, (String, i64)>(
            "SELECT catalog_version, station_count FROM catalog_metadata",
        )
        .fetch_one(&mut connection)
        .await
        .map_err(|_| CatalogImportError::safe("full catalog metadata could not be read"))?;
        if integrity != "ok"
            || schema_version != manifest.database_schema_version as i64
            || metadata.0 != manifest.catalog_version
            || metadata.1 != manifest.station_count as i64
        {
            connection.close().await.ok();
            return Err(CatalogImportError::safe(
                "full catalog SQLite metadata is invalid",
            ));
        }
        let rows = sqlx::query_as::<_, FullReleaseStation>(
            "SELECT station_id, source, source_station_id, name, tags_json, country_code, language, homepage_url, stream_url, codec, bitrate_kbps FROM stations ORDER BY station_id ASC",
        )
        .fetch_all(&mut connection)
        .await
        .map_err(|_| CatalogImportError::safe("full catalog stations could not be read"))?;
        connection.close().await.ok();
        if rows.len() != manifest.station_count as usize {
            return Err(CatalogImportError::safe(
                "full catalog station count is invalid",
            ));
        }
        let stations = rows
            .into_iter()
            .map(ImportedStation::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            catalog_version: manifest.catalog_version,
            stations,
        })
    }

    /// Immutable release identifier recorded in import logs.
    pub fn catalog_version(&self) -> &str {
        &self.catalog_version
    }

    /// Number of fully eligible stations available to the import run.
    pub fn station_count(&self) -> usize {
        self.stations.len()
    }
}

#[async_trait]
impl CatalogImportProvider for FullReleaseCatalogAdapter {
    fn source(&self) -> &'static str {
        FULL_RELEASE_SOURCE
    }

    async fn fetch_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<ImportPage, CatalogImportError> {
        if limit == 0 {
            return Err(CatalogImportError::safe(
                "full catalog page limit must be positive",
            ));
        }
        let stations = self
            .stations
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok(ImportPage {
            fetched: stations.len(),
            stations,
            skipped: 0,
        })
    }
}

#[derive(Deserialize)]
struct FullReleaseManifest {
    manifest_schema_version: u32,
    catalog_version: String,
    database_schema_version: u32,
    file: String,
    sha256: String,
    station_count: u64,
}

#[derive(FromRow)]
struct FullReleaseStation {
    station_id: String,
    source: String,
    source_station_id: String,
    name: String,
    tags_json: String,
    country_code: Option<String>,
    language: Option<String>,
    homepage_url: Option<String>,
    stream_url: String,
    codec: Option<String>,
    bitrate_kbps: Option<i64>,
}

impl TryFrom<FullReleaseStation> for ImportedStation {
    type Error = CatalogImportError;

    fn try_from(row: FullReleaseStation) -> Result<Self, Self::Error> {
        let tags: Vec<String> = serde_json::from_str(&row.tags_json)
            .map_err(|_| CatalogImportError::safe("full catalog tags are invalid"))?;
        let name = row.name.trim().to_owned();
        let stream_is_http = row.stream_url.to_ascii_lowercase().starts_with("http://")
            || row.stream_url.to_ascii_lowercase().starts_with("https://");
        if row.station_id.is_empty()
            || row.source.is_empty()
            || row.source_station_id.is_empty()
            || name.is_empty()
            || !stream_is_http
        {
            return Err(CatalogImportError::safe(
                "full catalog station data is invalid",
            ));
        }
        Ok(Self {
            source: FULL_RELEASE_SOURCE,
            source_station_id: format!("{}:{}", row.source, row.source_station_id),
            id: row.station_id.clone(),
            name,
            homepage_url: row.homepage_url.filter(|url| {
                url.to_ascii_lowercase().starts_with("http://")
                    || url.to_ascii_lowercase().starts_with("https://")
            }),
            tags,
            language: row.language,
            country_code: row.country_code,
            streams: vec![ImportedStream {
                source_stream_id: format!("{}:primary", row.station_id),
                stream_url: row.stream_url,
                codec: row.codec,
                bitrate_kbps: row
                    .bitrate_kbps
                    .and_then(|bitrate| u32::try_from(bitrate).ok()),
                is_primary: true,
            }],
        })
    }
}

fn valid_release_file_name(value: &str) -> bool {
    value == "rockmobile-extended-2026.08.2-mobile.1.sqlite"
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{FULL_RELEASE_SOURCE, FullReleaseCatalogAdapter};
    use crate::catalog::CatalogImportProvider;

    #[tokio::test]
    async fn pinned_complete_release_is_checksum_valid_and_pageable() {
        let adapter = FullReleaseCatalogAdapter::pinned().await.unwrap();
        assert_eq!(adapter.catalog_version(), "2026.08.2-mobile.1");
        assert_eq!(adapter.station_count(), 16_825);
        assert_eq!(adapter.source(), FULL_RELEASE_SOURCE);
        assert_eq!(
            adapter.fetch_page(0, 500).await.unwrap().stations.len(),
            500
        );
    }
}
