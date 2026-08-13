//! Deterministic search domain and its catalog boundary.

use std::{
    collections::{BTreeSet, HashSet},
    error::Error,
    fmt,
    sync::Arc,
};

use async_trait::async_trait;

/// A normalized station-search query understood by the deterministic search service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchQuery {
    /// Original request text after trimming leading and trailing whitespace.
    pub original: String,
    /// Locale used while interpreting the request.
    pub locale: String,
    /// Lowercase terms extracted from the original request.
    pub terms: Vec<String>,
    /// Recognized catalog tags extracted from the request.
    pub tags: Vec<String>,
    /// Language constraint inferred from the locale, when available.
    pub language: Option<String>,
    /// Country constraint inferred from request terms, when available.
    pub country_code: Option<String>,
}

/// Constraints that affect which and how many stations are returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchConstraints {
    /// Maximum number of ranked stations to return.
    pub limit: usize,
    /// Station identifiers that must not appear in the result.
    pub excluded_station_ids: BTreeSet<String>,
}

/// A station record exposed by a catalog repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Station {
    /// Stable RockServer station identifier.
    pub id: String,
    /// Human-readable station name.
    pub name: String,
    /// Direct, playable stream URL.
    pub stream_url: String,
    /// Optional public home page for the station.
    pub homepage_url: Option<String>,
    /// Normalized searchable station tags.
    pub tags: Vec<String>,
    /// ISO 639 language code, when known.
    pub language: Option<String>,
    /// ISO 3166-1 alpha-2 country code, when known.
    pub country_code: Option<String>,
    /// Audio codec, when known.
    pub codec: Option<String>,
    /// Stream bitrate in kilobits per second, when known.
    pub bitrate_kbps: Option<u32>,
    /// Current catalog health classification.
    pub health: StationHealth,
}

/// Health information stored with a station record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationHealth {
    /// The catalog considers the station healthy.
    Healthy,
    /// The catalog considers the station degraded.
    Degraded,
    /// The catalog has no health information for the station.
    Unknown,
}

/// A station selected by the search service with its deterministic score.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedStation {
    /// The matching catalog record.
    pub station: Station,
    /// Match score in the inclusive range from zero to one.
    pub score: f64,
    /// Short explanation of the metadata that matched the request.
    pub reason: String,
}

/// Catalog access boundary used by the search domain.
#[async_trait]
pub trait StationRepository {
    /// Searches the catalog using domain-normalized input and deterministic ordering rules.
    async fn search(
        &self,
        query: &SearchQuery,
        constraints: &SearchConstraints,
    ) -> Result<Vec<RankedStation>, RepositoryError>;

    /// Verifies that the repository dependency can currently serve requests.
    async fn check_readiness(&self) -> Result<(), RepositoryError>;
}

/// An operational repository failure safe to map at service boundaries.
#[derive(Debug)]
pub struct RepositoryError {
    operation: &'static str,
    source: Box<dyn Error + Send + Sync>,
}

impl RepositoryError {
    /// Wraps an implementation error without exposing provider details to HTTP clients.
    pub(crate) fn new(operation: &'static str, source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            operation,
            source: Box::new(source),
        }
    }
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "station repository {} failed", self.operation)
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// A small, built-in station catalog for deterministic local search.
#[derive(Clone, Debug, Default)]
pub struct InMemoryStationRepository {
    stations: Vec<Station>,
}

impl InMemoryStationRepository {
    /// Creates the fixed catalog used by the service until persistent storage is introduced.
    pub fn with_builtin_catalog() -> Self {
        Self {
            stations: vec![
                station(
                    "station-ambient-001",
                    "Arctic Ambient",
                    "https://streams.example.com/arctic-ambient.mp3",
                    StationMetadata {
                        homepage_url: Some("https://example.com/arctic-ambient"),
                        tags: &["ambient", "calm", "electronic", "instrumental"],
                        language: Some("en"),
                        country_code: Some("IS"),
                        codec: Some("MP3"),
                        bitrate_kbps: Some(160),
                    },
                ),
                station(
                    "station-jazz-001",
                    "Quiet Jazz Radio",
                    "https://streams.example.com/quiet-jazz.mp3",
                    StationMetadata {
                        homepage_url: Some("https://example.com/quiet-jazz"),
                        tags: &["calm", "instrumental", "jazz"],
                        language: Some("en"),
                        country_code: Some("US"),
                        codec: Some("MP3"),
                        bitrate_kbps: Some(192),
                    },
                ),
                station(
                    "station-jazz-002",
                    "Midnight Jazz Lounge",
                    "https://streams.example.com/midnight-jazz.aac",
                    StationMetadata {
                        homepage_url: None,
                        tags: &["jazz", "smooth"],
                        language: Some("en"),
                        country_code: Some("GB"),
                        codec: Some("AAC"),
                        bitrate_kbps: Some(128),
                    },
                ),
                station(
                    "station-rock-001",
                    "Highway Rock",
                    "https://streams.example.com/highway-rock.aac",
                    StationMetadata {
                        homepage_url: None,
                        tags: &["classic rock", "rock", "upbeat"],
                        language: Some("en"),
                        country_code: Some("GB"),
                        codec: Some("AAC"),
                        bitrate_kbps: Some(128),
                    },
                ),
                station(
                    "station-rock-002",
                    "Heritage Rock",
                    "https://streams.example.com/heritage-rock.mp3",
                    StationMetadata {
                        homepage_url: Some("https://example.com/heritage-rock"),
                        tags: &["classic rock", "rock"],
                        language: Some("en"),
                        country_code: Some("US"),
                        codec: Some("MP3"),
                        bitrate_kbps: Some(192),
                    },
                ),
                station(
                    "station-rock-ru-001",
                    "Радио Рок",
                    "https://streams.example.com/radio-rock-ru.mp3",
                    StationMetadata {
                        homepage_url: Some("https://example.com/radio-rock-ru"),
                        tags: &["classic rock", "rock"],
                        language: Some("ru"),
                        country_code: Some("RU"),
                        codec: Some("MP3"),
                        bitrate_kbps: Some(128),
                    },
                ),
            ],
        }
    }
}

#[async_trait]
impl StationRepository for InMemoryStationRepository {
    async fn search(
        &self,
        query: &SearchQuery,
        constraints: &SearchConstraints,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        Ok(rank_stations(&self.stations, query, constraints))
    }

    async fn check_readiness(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

/// Deterministic metadata-only station search service.
#[derive(Clone)]
pub struct SearchService {
    repository: Arc<dyn StationRepository + Send + Sync>,
}

impl SearchService {
    /// Creates a service using the supplied station repository.
    pub fn new(repository: Arc<dyn StationRepository + Send + Sync>) -> Self {
        Self { repository }
    }

    /// Returns matching stations ordered by score descending and station ID ascending.
    ///
    /// Implementations execute the domain-owned hard filters, scoring, stable tie-break, and limit.
    pub async fn search(
        &self,
        query: &SearchQuery,
        constraints: &SearchConstraints,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        self.repository.search(query, constraints).await
    }

    /// Checks whether the configured catalog backend is currently available.
    pub async fn check_readiness(&self) -> Result<(), RepositoryError> {
        self.repository.check_readiness().await
    }
}

/// Builds a normalized query from validated HTTP request values.
pub fn normalize_query(original: String, locale: String) -> SearchQuery {
    let terms = tokenize(&original);
    let tags = recognized_tags(&terms);
    let country_code = infer_country_code(&terms);
    let language = locale.split('-').next().map(str::to_lowercase);

    SearchQuery {
        original,
        locale,
        terms,
        tags,
        language,
        country_code,
    }
}

struct StationMetadata<'a> {
    homepage_url: Option<&'a str>,
    tags: &'a [&'a str],
    language: Option<&'a str>,
    country_code: Option<&'a str>,
    codec: Option<&'a str>,
    bitrate_kbps: Option<u32>,
}

fn station(id: &str, name: &str, stream_url: &str, metadata: StationMetadata<'_>) -> Station {
    Station {
        id: id.to_owned(),
        name: name.to_owned(),
        stream_url: stream_url.to_owned(),
        homepage_url: metadata.homepage_url.map(str::to_owned),
        tags: metadata.tags.iter().map(|tag| (*tag).to_owned()).collect(),
        language: metadata.language.map(str::to_owned),
        country_code: metadata.country_code.map(str::to_owned),
        codec: metadata.codec.map(str::to_owned),
        bitrate_kbps: metadata.bitrate_kbps,
        health: StationHealth::Healthy,
    }
}

fn language_matches(station: &Station, query: &SearchQuery) -> bool {
    query
        .language
        .as_deref()
        .is_none_or(|language| station.language.as_deref() == Some(language))
}

fn country_matches(station: &Station, query: &SearchQuery) -> bool {
    query
        .country_code
        .as_deref()
        .is_none_or(|country_code| station.country_code.as_deref() == Some(country_code))
}

/// Applies the domain-owned search semantics to an already-loaded catalog.
fn rank_stations(
    stations: &[Station],
    query: &SearchQuery,
    constraints: &SearchConstraints,
) -> Vec<RankedStation> {
    let mut results = stations
        .iter()
        .filter(|station| !constraints.excluded_station_ids.contains(&station.id))
        .filter(|station| language_matches(station, query))
        .filter(|station| country_matches(station, query))
        .filter_map(|station| rank_station(station, query))
        .collect::<Vec<_>>();

    results.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.station.id.cmp(&right.station.id))
    });
    results.truncate(constraints.limit);
    results
}

fn rank_station(station: &Station, query: &SearchQuery) -> Option<RankedStation> {
    let searchable_terms = station_searchable_terms(station);
    let matched_terms = query
        .terms
        .iter()
        .filter(|term| searchable_terms.contains(term.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    let matched_tags = query
        .tags
        .iter()
        .filter(|tag| station.tags.iter().any(|station_tag| station_tag == *tag))
        .cloned()
        .collect::<BTreeSet<_>>();

    if matched_terms.is_empty() && matched_tags.is_empty() {
        return None;
    }

    let matched_count = matched_terms.len() + matched_tags.len();
    let query_count = query.terms.len() + query.tags.len();
    let score = matched_count as f64 / query_count.max(1) as f64;
    let reason_terms = matched_tags
        .into_iter()
        .chain(matched_terms)
        .collect::<Vec<_>>();

    Some(RankedStation {
        station: station.clone(),
        score,
        reason: format!("Matched catalog metadata: {}.", reason_terms.join(", ")),
    })
}

fn station_searchable_terms(station: &Station) -> HashSet<String> {
    tokenize(&station.name)
        .into_iter()
        .chain(station.tags.iter().flat_map(|tag| tokenize(tag)))
        .collect()
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn recognized_tags(terms: &[String]) -> Vec<String> {
    const TAGS: &[&str] = &["ambient", "calm", "instrumental", "jazz", "rock", "upbeat"];
    let term_set = terms.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut tags = TAGS
        .iter()
        .filter(|tag| term_set.contains(**tag))
        .map(|tag| (*tag).to_owned())
        .collect::<Vec<_>>();
    if term_set.contains("classic") && term_set.contains("rock") {
        tags.push("classic rock".to_owned());
    }
    tags
}

fn infer_country_code(terms: &[String]) -> Option<String> {
    let term_set = terms.iter().map(String::as_str).collect::<HashSet<_>>();
    if term_set.contains("russia") || term_set.contains("россия") {
        Some("RU".to_owned())
    } else if term_set.contains("british") || term_set.contains("uk") {
        Some("GB".to_owned())
    } else if term_set.contains("american") || term_set.contains("usa") {
        Some("US".to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use super::{InMemoryStationRepository, SearchConstraints, SearchService, normalize_query};

    #[tokio::test]
    async fn equal_scores_are_ordered_by_station_id() {
        let service =
            SearchService::new(Arc::new(InMemoryStationRepository::with_builtin_catalog()));
        let query = normalize_query("rock".to_owned(), "en-US".to_owned());
        let constraints = SearchConstraints {
            limit: 10,
            excluded_station_ids: BTreeSet::new(),
        };

        let ids = service
            .search(&query, &constraints)
            .await
            .unwrap()
            .into_iter()
            .map(|station| station.station.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, ["station-rock-001", "station-rock-002"]);
    }

    #[tokio::test]
    async fn exclusions_are_applied_before_ranking() {
        let service =
            SearchService::new(Arc::new(InMemoryStationRepository::with_builtin_catalog()));
        let query = normalize_query("jazz".to_owned(), "en-US".to_owned());
        let constraints = SearchConstraints {
            limit: 10,
            excluded_station_ids: BTreeSet::from(["station-jazz-001".to_owned()]),
        };

        let ids = service
            .search(&query, &constraints)
            .await
            .unwrap()
            .into_iter()
            .map(|station| station.station.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, ["station-jazz-002"]);
    }
}
