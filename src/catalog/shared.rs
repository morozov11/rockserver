//! Offline, pinned adapter for the reviewed RockCatalog baseline release.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use super::{
    CatalogImportError, CatalogImportProvider, ImportPage, ImportedStation, ImportedStream,
    ImportedTombstone, TombstoneReason,
};

/// Provider ownership namespace for records from the shared curated catalog.
pub const ROCKCATALOG_SOURCE: &str = "rockcatalog";
/// Immutable reviewed release version compiled into this RockServer revision.
pub const PINNED_CATALOG_VERSION: &str = "2026.08.2";
/// SHA-256 of the exact canonical `stations.v1.json` release artifact.
pub const PINNED_CATALOG_SHA256: &str =
    "3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d";

const PINNED_CATALOG_BYTES: &[u8] = include_bytes!("../../catalog/rockcatalog/stations.v1.json");

/// A validated immutable shared-catalog release ready for a provider-scoped import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinnedSharedCatalog {
    catalog_version: String,
    stations: Vec<ImportedStation>,
    tombstones: Vec<ImportedTombstone>,
}

impl PinnedSharedCatalog {
    /// Loads the release vendored with RockServer after checksum, structural, and semantic checks.
    pub fn load() -> Result<Self, SharedCatalogError> {
        Self::from_bytes(
            PINNED_CATALOG_BYTES,
            PINNED_CATALOG_VERSION,
            PINNED_CATALOG_SHA256,
        )
    }

    /// Validates an explicit immutable artifact. This is exposed for deterministic fixture tests.
    pub fn from_bytes(
        bytes: &[u8],
        expected_version: &str,
        expected_sha256: &str,
    ) -> Result<Self, SharedCatalogError> {
        // Git may check the vendored text artifact out with CRLF on Windows. The release manifest
        // hashes the canonical LF JSON bytes emitted by the catalog formatter.
        let canonical = std::str::from_utf8(bytes)
            .map_err(|_| SharedCatalogError::InvalidStructure("document is not UTF-8"))?
            .replace("\r\n", "\n");
        let actual_sha256 = format!("{:x}", Sha256::digest(canonical.as_bytes()));
        if actual_sha256 != expected_sha256 {
            return Err(SharedCatalogError::ChecksumMismatch);
        }
        let document: CatalogDocument = serde_json::from_str(&canonical)
            .map_err(|_| SharedCatalogError::InvalidStructure("document is not valid JSON"))?;
        if document.schema_version != 1 {
            return Err(SharedCatalogError::InvalidStructure(
                "schemaVersion must equal 1",
            ));
        }
        if document.catalog_version != expected_version {
            return Err(SharedCatalogError::InvalidStructure(
                "catalogVersion does not match the pinned release",
            ));
        }
        validate_document(&document)?;
        Ok(Self {
            catalog_version: document.catalog_version,
            stations: document
                .stations
                .into_iter()
                .map(ImportedStation::from)
                .collect(),
            tombstones: document
                .tombstones
                .into_iter()
                .map(ImportedTombstone::from)
                .collect(),
        })
    }

    /// Returns the immutable release version after successful preflight.
    pub fn catalog_version(&self) -> &str {
        &self.catalog_version
    }

    /// Returns all active baseline records in their reviewed deterministic order.
    pub fn stations(&self) -> &[ImportedStation] {
        &self.stations
    }

    /// Returns all lifecycle records for inactive canonical identities.
    pub fn tombstones(&self) -> &[ImportedTombstone] {
        &self.tombstones
    }

    /// Resolves a retired ID only when the lifecycle contract permits an automatic redirect.
    ///
    /// A split is intentionally returned as ambiguous rather than choosing a user-state target.
    pub fn replacement_for(&self, id: &str) -> CatalogReplacement<'_> {
        match self.tombstones.iter().find(|tombstone| tombstone.id == id) {
            Some(tombstone) if tombstone.reason == TombstoneReason::Merged => {
                CatalogReplacement::Redirect(&tombstone.replacement_ids[0])
            }
            Some(tombstone) if tombstone.reason == TombstoneReason::Split => {
                CatalogReplacement::Ambiguous(&tombstone.replacement_ids)
            }
            Some(_) => CatalogReplacement::Removed,
            None => CatalogReplacement::Unknown,
        }
    }
}

/// Resolution result for a retired canonical station identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogReplacement<'a> {
    /// No active or retired canonical record is known for this ID.
    Unknown,
    /// The station was removed and has no successor.
    Removed,
    /// A merge permits automatic redirection to this sole active successor.
    Redirect(&'a str),
    /// A split has several valid successors and requires an explicit caller choice.
    Ambiguous(&'a [String]),
}

/// Provider-neutral adapter that exposes the pinned release through the import boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedCatalogAdapter {
    catalog: PinnedSharedCatalog,
}

impl SharedCatalogAdapter {
    /// Constructs the adapter only after the vendored artifact passes complete preflight.
    pub fn pinned() -> Result<Self, SharedCatalogError> {
        Ok(Self {
            catalog: PinnedSharedCatalog::load()?,
        })
    }

    /// Returns the validated release used by this adapter.
    pub fn catalog(&self) -> &PinnedSharedCatalog {
        &self.catalog
    }
}

#[async_trait]
impl CatalogImportProvider for SharedCatalogAdapter {
    fn source(&self) -> &'static str {
        ROCKCATALOG_SOURCE
    }

    async fn fetch_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<ImportPage, CatalogImportError> {
        if limit == 0 {
            return Err(CatalogImportError::safe(
                "Shared catalog page limit must be positive",
            ));
        }
        let stations = self
            .catalog
            .stations()
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

/// A non-sensitive failure discovered before shared-catalog activation writes anything.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SharedCatalogError {
    /// Artifact bytes differ from the release manifest checksum.
    ChecksumMismatch,
    /// JSON cannot satisfy the v1 structural contract.
    InvalidStructure(&'static str),
    /// A semantic invariant required by the v1 contract is violated.
    InvalidInvariant(&'static str),
}

impl fmt::Display for SharedCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChecksumMismatch => formatter.write_str("shared catalog checksum mismatch"),
            Self::InvalidStructure(message) => {
                write!(formatter, "shared catalog structure invalid: {message}")
            }
            Self::InvalidInvariant(message) => {
                write!(formatter, "shared catalog invariant invalid: {message}")
            }
        }
    }
}

impl Error for SharedCatalogError {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogDocument {
    schema_version: u32,
    catalog_version: String,
    stations: Vec<CatalogStation>,
    tombstones: Vec<CatalogTombstone>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogStation {
    id: String,
    name: String,
    aliases: Vec<String>,
    legacy_ids: Vec<String>,
    tags: Vec<String>,
    country_code: Option<String>,
    language: Option<String>,
    homepage_url: Option<String>,
    favicon_url: Option<String>,
    streams: Vec<CatalogStream>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogStream {
    id: String,
    url: String,
    codec: Option<String>,
    bitrate_kbps: Option<u32>,
    primary: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogTombstone {
    id: String,
    reason: String,
    replacement_ids: Vec<String>,
}

impl From<CatalogTombstone> for ImportedTombstone {
    fn from(tombstone: CatalogTombstone) -> Self {
        let reason = match tombstone.reason.as_str() {
            "removed" => TombstoneReason::Removed,
            "merged" => TombstoneReason::Merged,
            "split" => TombstoneReason::Split,
            // `validate_document` is called before this conversion.
            _ => unreachable!("validated tombstone reason"),
        };
        Self {
            id: tombstone.id,
            reason,
            replacement_ids: tombstone.replacement_ids,
        }
    }
}

impl From<CatalogStation> for ImportedStation {
    fn from(station: CatalogStation) -> Self {
        Self {
            source: ROCKCATALOG_SOURCE,
            source_station_id: station.id.clone(),
            id: station.id.clone(),
            name: station.name,
            homepage_url: station.homepage_url,
            tags: station.tags,
            language: station.language,
            country_code: station.country_code,
            streams: station
                .streams
                .into_iter()
                .map(|stream| ImportedStream {
                    source_stream_id: format!("{}:{}", station.id, stream.id),
                    stream_url: stream.url,
                    codec: stream.codec,
                    bitrate_kbps: stream.bitrate_kbps,
                    is_primary: stream.primary,
                })
                .collect(),
        }
    }
}

fn validate_document(document: &CatalogDocument) -> Result<(), SharedCatalogError> {
    if document.stations.is_empty() {
        return Err(SharedCatalogError::InvalidInvariant(
            "at least one active station is required",
        ));
    }
    let mut station_ids = BTreeSet::new();
    let mut legacy_ids = BTreeSet::new();
    for station in &document.stations {
        if !station_ids.insert(&station.id) || !valid_id(&station.id, 96) {
            return Err(SharedCatalogError::InvalidInvariant(
                "station IDs must be unique lowercase kebab-case values",
            ));
        }
        if station.name.trim() != station.name
            || station.name.is_empty()
            || station.name.len() > 200
        {
            return Err(SharedCatalogError::InvalidInvariant(
                "station names must be trimmed text",
            ));
        }
        if station
            .aliases
            .iter()
            .any(|value| value.trim() != value || value.is_empty())
        {
            return Err(SharedCatalogError::InvalidInvariant(
                "aliases must be trimmed text",
            ));
        }
        if station
            .legacy_ids
            .iter()
            .any(|value| !legacy_ids.insert(value))
        {
            return Err(SharedCatalogError::InvalidInvariant(
                "legacy IDs must be globally unique",
            ));
        }
        if !normalized_values(&station.tags) {
            return Err(SharedCatalogError::InvalidInvariant(
                "tags must be normalized and sorted",
            ));
        }
        if station.country_code.as_deref().is_some_and(|value| {
            value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_uppercase())
        }) || station.language.as_deref().is_some_and(|value| {
            !(2..=3).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_lowercase())
        }) || !optional_url_is_valid(station.homepage_url.as_deref())
            || !optional_url_is_valid(station.favicon_url.as_deref())
        {
            return Err(SharedCatalogError::InvalidInvariant(
                "station metadata is invalid",
            ));
        }
        if station.streams.is_empty() {
            return Err(SharedCatalogError::InvalidInvariant(
                "stations require streams",
            ));
        }
        let mut stream_ids = BTreeSet::new();
        let mut urls = BTreeSet::new();
        let primary_count = station
            .streams
            .iter()
            .filter(|stream| stream.primary)
            .count();
        if primary_count != 1 {
            return Err(SharedCatalogError::InvalidInvariant(
                "stations require exactly one primary stream",
            ));
        }
        for stream in &station.streams {
            if !stream_ids.insert(&stream.id)
                || !valid_id(&stream.id, 31)
                || !urls.insert(&stream.url)
                || !valid_url(&stream.url)
                || stream.codec.as_deref().is_some_and(|value| {
                    value.is_empty()
                        || value.len() > 32
                        || !value.bytes().all(|byte| {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || matches!(byte, b'.' | b'+' | b'-')
                        })
                })
                || stream
                    .bitrate_kbps
                    .is_some_and(|value| !(1..=2_000).contains(&value))
            {
                return Err(SharedCatalogError::InvalidInvariant(
                    "stream metadata is invalid",
                ));
            }
        }
    }
    let tombstone_ids = document
        .tombstones
        .iter()
        .map(|tombstone| &tombstone.id)
        .collect::<BTreeSet<_>>();
    if tombstone_ids.len() != document.tombstones.len()
        || tombstone_ids.iter().any(|id| station_ids.contains(*id))
    {
        return Err(SharedCatalogError::InvalidInvariant(
            "tombstone identities are invalid",
        ));
    }
    let mut replacements = BTreeMap::new();
    for tombstone in &document.tombstones {
        let valid_shape = valid_id(&tombstone.id, 96)
            && matches!(tombstone.reason.as_str(), "removed" | "merged" | "split")
            && match tombstone.reason.as_str() {
                "removed" => tombstone.replacement_ids.is_empty(),
                "merged" => tombstone.replacement_ids.len() == 1,
                "split" => tombstone.replacement_ids.len() >= 2,
                _ => false,
            };
        if !valid_shape
            || tombstone.replacement_ids.iter().any(|id| {
                !valid_id(id, 96)
                    || id == &tombstone.id
                    || !station_ids.contains(id) && !tombstone_ids.contains(id)
            })
            || tombstone
                .replacement_ids
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != tombstone.replacement_ids.len()
        {
            return Err(SharedCatalogError::InvalidInvariant(
                "tombstone replacement graph is invalid",
            ));
        }
        replacements.insert(
            tombstone.id.as_str(),
            tombstone
                .replacement_ids
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
    }
    for id in replacements.keys() {
        if replacement_graph_has_cycle(id, &replacements, &mut BTreeSet::new()) {
            return Err(SharedCatalogError::InvalidInvariant(
                "tombstone replacement graph contains a cycle",
            ));
        }
    }
    Ok(())
}

fn replacement_graph_has_cycle<'a>(
    id: &'a str,
    graph: &BTreeMap<&'a str, Vec<&'a str>>,
    path: &mut BTreeSet<&'a str>,
) -> bool {
    if !path.insert(id) {
        return true;
    }
    let has_cycle = graph.get(id).is_some_and(|children| {
        children
            .iter()
            .any(|child| replacement_graph_has_cycle(child, graph, path))
    });
    path.remove(id);
    has_cycle
}

fn valid_id(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn normalized_values(values: &[String]) -> bool {
    values
        .iter()
        .all(|value| !value.is_empty() && value.trim() == value && value == &value.to_lowercase())
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn optional_url_is_valid(value: Option<&str>) -> bool {
    value.is_none_or(valid_url)
}

fn valid_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|parsed| {
        matches!(parsed.scheme(), "http" | "https")
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.fragment().is_none()
            && value.len() <= 2_048
    })
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{
        CatalogReplacement, PINNED_CATALOG_SHA256, PINNED_CATALOG_VERSION, PinnedSharedCatalog,
        ROCKCATALOG_SOURCE, SharedCatalogAdapter, SharedCatalogError,
    };
    use crate::catalog::CatalogImportProvider;

    #[tokio::test]
    async fn pinned_catalog_has_stable_provider_and_stream_identities() {
        let adapter = SharedCatalogAdapter::pinned().unwrap();
        let page = adapter.fetch_page(0, 100).await.unwrap();
        assert_eq!(ROCKCATALOG_SOURCE, adapter.source());
        assert_eq!(PINNED_CATALOG_VERSION, adapter.catalog().catalog_version());
        assert_eq!(41, page.stations.len());
        assert_eq!("somafm-metal-detector", page.stations[0].id);
        assert_eq!(
            "somafm-metal-detector:main",
            page.stations[0].streams[0].source_stream_id
        );
    }

    #[test]
    fn checksum_failure_prevents_any_catalog_mapping() {
        let error =
            PinnedSharedCatalog::from_bytes(b"{}", PINNED_CATALOG_VERSION, PINNED_CATALOG_SHA256)
                .unwrap_err();
        assert_eq!(SharedCatalogError::ChecksumMismatch, error);
    }

    #[test]
    fn preflight_maps_multiple_streams_and_rejects_schema_versions() {
        let fixture = br#"{
  "schemaVersion": 1,
  "catalogVersion": "test",
  "stations": [{
    "id": "test-station", "name": "Test station", "aliases": [], "legacyIds": [],
    "tags": ["rock"], "countryCode": null, "language": null, "homepageUrl": null,
    "faviconUrl": null,
    "streams": [
      {"id": "aac", "url": "https://example.com/aac", "codec": "aac", "bitrateKbps": 128, "primary": false},
      {"id": "mp3", "url": "https://example.com/mp3", "codec": "mp3", "bitrateKbps": 192, "primary": true}
    ]
  }], "tombstones": []
}"#;
        let digest = format!("{:x}", Sha256::digest(fixture));
        let catalog = PinnedSharedCatalog::from_bytes(fixture, "test", &digest).unwrap();
        assert_eq!(2, catalog.stations()[0].streams.len());
        assert_eq!(
            "test-station:mp3",
            catalog.stations()[0].streams[1].source_stream_id
        );
        assert!(catalog.stations()[0].streams[1].is_primary);

        let unsupported = std::str::from_utf8(fixture)
            .unwrap()
            .replace("\"schemaVersion\": 1", "\"schemaVersion\": 2");
        let digest = format!("{:x}", Sha256::digest(unsupported.as_bytes()));
        assert_eq!(
            SharedCatalogError::InvalidStructure("schemaVersion must equal 1"),
            PinnedSharedCatalog::from_bytes(unsupported.as_bytes(), "test", &digest).unwrap_err()
        );
    }

    #[test]
    fn tombstones_preserve_removed_merge_and_split_semantics() {
        let fixture = br#"{
  "schemaVersion": 1,
  "catalogVersion": "test",
  "stations": [
    {"id":"merge-target","name":"Merge target","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{"id":"main","url":"https://example.com/merge","codec":"mp3","bitrateKbps":128,"primary":true}]},
    {"id":"split-one","name":"Split one","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{"id":"main","url":"https://example.com/split-one","codec":"mp3","bitrateKbps":128,"primary":true}]},
    {"id":"split-two","name":"Split two","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{"id":"main","url":"https://example.com/split-two","codec":"mp3","bitrateKbps":128,"primary":true}]}
  ],
  "tombstones": [
    {"id":"removed-old","reason":"removed","replacementIds":[]},
    {"id":"merged-old","reason":"merged","replacementIds":["merge-target"]},
    {"id":"split-old","reason":"split","replacementIds":["split-one","split-two"]}
  ]
}"#;
        let digest = format!("{:x}", Sha256::digest(fixture));
        let catalog = PinnedSharedCatalog::from_bytes(fixture, "test", &digest).unwrap();

        assert_eq!(catalog.tombstones().len(), 3);
        assert_eq!(
            catalog.replacement_for("removed-old"),
            CatalogReplacement::Removed
        );
        assert_eq!(
            catalog.replacement_for("merged-old"),
            CatalogReplacement::Redirect("merge-target")
        );
        assert_eq!(
            catalog.replacement_for("split-old"),
            CatalogReplacement::Ambiguous(&["split-one".to_owned(), "split-two".to_owned()])
        );
        assert_eq!(
            catalog.replacement_for("unknown"),
            CatalogReplacement::Unknown
        );
    }

    #[test]
    fn invalid_catalog_bytes_fail_without_an_implicit_fixture() {
        let malformed = b"not json";
        let digest = format!("{:x}", Sha256::digest(malformed));
        assert_eq!(
            PinnedSharedCatalog::from_bytes(malformed, "test", &digest).unwrap_err(),
            SharedCatalogError::InvalidStructure("document is not valid JSON")
        );
    }
}
