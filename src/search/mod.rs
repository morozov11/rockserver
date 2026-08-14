//! Search domain, provider boundaries, fallback semantics, and catalog boundary.

mod embedding;
mod llm;
mod query;
mod ranking;

use std::{collections::BTreeSet, error::Error, fmt, sync::Arc};

use async_trait::async_trait;

pub use embedding::{
    Embedding, EmbeddingBackfill, EmbeddingBackfillResult, EmbeddingProvenance, EmbeddingProvider,
    EmbeddingProviderError, EmbeddingStore, EmbeddingStoreError, EmbeddingValidationError,
    MAX_EMBEDDING_DIMENSION, StationEmbeddingDocument,
};
pub use llm::{
    LlmProvider, LlmProviderError, LlmQueryParser, LlmRequest, MAX_LLM_INTENT_JSON_BYTES,
};
pub use query::{
    DeterministicQueryParser, QueryIntent, QueryParser, QueryParserError, QueryParserInput,
    normalize_query,
};
pub use ranking::{METADATA_WEIGHT, SEMANTIC_WEIGHT, hybrid_score};

use query::validate_intent;
use ranking::rank_stations;

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

impl SearchQuery {
    fn from_intent(original: String, locale: String, intent: QueryIntent) -> Self {
        Self {
            original,
            locale,
            terms: intent.terms,
            tags: intent.tags,
            language: intent.language,
            country_code: intent.country_code,
        }
    }
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
        embedding: Option<&Embedding>,
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
        _embedding: Option<&Embedding>,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        Ok(rank_stations(&self.stations, query, constraints))
    }

    async fn check_readiness(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

/// Interpreted search result returned to the HTTP transport layer.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchOutcome {
    /// Normalized structured interpretation returned to the caller.
    pub query: SearchQuery,
    /// Ranked stations returned by the repository.
    pub stations: Vec<RankedStation>,
}

/// Search orchestration with deterministic parser and metadata fallbacks.
#[derive(Clone)]
pub struct SearchService {
    repository: Arc<dyn StationRepository + Send + Sync>,
    query_parser: Arc<dyn QueryParser>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
}

impl SearchService {
    /// Creates metadata-only search using the deterministic query parser.
    pub fn new(repository: Arc<dyn StationRepository + Send + Sync>) -> Self {
        Self {
            repository,
            query_parser: Arc::new(DeterministicQueryParser),
            embedding_provider: None,
        }
    }

    /// Creates search with replaceable parser and optional embedding provider boundaries.
    ///
    /// Any parser or embedding failure degrades to deterministic metadata behavior.
    pub fn with_providers(
        repository: Arc<dyn StationRepository + Send + Sync>,
        query_parser: Arc<dyn QueryParser>,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self {
            repository,
            query_parser,
            embedding_provider,
        }
    }

    /// Returns matching stations ordered by score descending and station ID ascending.
    ///
    /// Implementations execute the domain-owned hard filters, scoring, stable tie-break, and limit.
    pub async fn search(
        &self,
        query: &SearchQuery,
        constraints: &SearchConstraints,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        let embedding = self.query_embedding(&query.original).await;
        self.repository
            .search(query, constraints, embedding.as_ref())
            .await
    }

    /// Interprets request-only input and searches without exposing catalog data to providers.
    pub async fn interpret_and_search(
        &self,
        input: QueryParserInput,
        constraints: &SearchConstraints,
    ) -> Result<SearchOutcome, RepositoryError> {
        let intent = match self
            .query_parser
            .parse(&input)
            .await
            .and_then(validate_intent)
        {
            Ok(intent) => intent,
            Err(error) => {
                tracing::warn!(%error, "query parser failed; using deterministic metadata fallback");
                DeterministicQueryParser
                    .parse(&input)
                    .await
                    .expect("deterministic query parser cannot fail")
            }
        };
        let query = SearchQuery::from_intent(input.query, input.locale, intent);
        let stations = self.search(&query, constraints).await?;
        Ok(SearchOutcome { query, stations })
    }

    /// Checks whether the configured catalog backend is currently available.
    pub async fn check_readiness(&self) -> Result<(), RepositoryError> {
        self.repository.check_readiness().await
    }

    async fn query_embedding(&self, text: &str) -> Option<Embedding> {
        let provider = self.embedding_provider.as_ref()?;
        match provider.embed(text).await {
            Ok(embedding) => Some(embedding),
            Err(error) => {
                tracing::warn!(%error, "embedding provider failed; using metadata fallback");
                None
            }
        }
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

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;

    use super::{
        DeterministicQueryParser, Embedding, EmbeddingProvider, EmbeddingProviderError,
        InMemoryStationRepository, QueryIntent, QueryParser, QueryParserError, QueryParserInput,
        RankedStation, RepositoryError, SearchConstraints, SearchQuery, SearchService,
        StationRepository, normalize_query,
    };

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

    struct RecordingParser {
        input: Arc<Mutex<Option<QueryParserInput>>>,
        fail: bool,
    }

    #[async_trait]
    impl QueryParser for RecordingParser {
        async fn parse(&self, input: &QueryParserInput) -> Result<QueryIntent, QueryParserError> {
            *self.input.lock().unwrap() = Some(input.clone());
            if self.fail {
                return Err(QueryParserError::safe("scripted parser failure"));
            }
            Ok(QueryIntent {
                terms: vec!["rock".to_owned()],
                tags: vec!["rock".to_owned()],
                language: Some("en".to_owned()),
                country_code: None,
            })
        }
    }

    #[tokio::test]
    async fn query_parser_receives_only_request_input_and_returns_structured_intent() {
        let input_seen = Arc::new(Mutex::new(None));
        let service = SearchService::with_providers(
            Arc::new(InMemoryStationRepository::with_builtin_catalog()),
            Arc::new(RecordingParser {
                input: input_seen.clone(),
                fail: false,
            }),
            None,
        );
        let input = QueryParserInput {
            query: "music for driving".to_owned(),
            locale: "en-US".to_owned(),
        };

        let outcome = service
            .interpret_and_search(
                input.clone(),
                &SearchConstraints {
                    limit: 10,
                    excluded_station_ids: BTreeSet::new(),
                },
            )
            .await
            .unwrap();

        assert_eq!(*input_seen.lock().unwrap(), Some(input));
        assert_eq!(outcome.query.terms, ["rock"]);
        assert_eq!(outcome.stations.len(), 2);
    }

    struct FailingEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for FailingEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
            Err(EmbeddingProviderError::safe("scripted embedding failure"))
        }
    }

    #[tokio::test]
    async fn parser_and_embedding_failures_preserve_metadata_fallback() {
        let service = SearchService::with_providers(
            Arc::new(InMemoryStationRepository::with_builtin_catalog()),
            Arc::new(RecordingParser {
                input: Arc::new(Mutex::new(None)),
                fail: true,
            }),
            Some(Arc::new(FailingEmbeddingProvider)),
        );

        let outcome = service
            .interpret_and_search(
                QueryParserInput {
                    query: "rock".to_owned(),
                    locale: "en-US".to_owned(),
                },
                &SearchConstraints {
                    limit: 10,
                    excluded_station_ids: BTreeSet::new(),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            outcome
                .stations
                .iter()
                .map(|station| station.station.id.as_str())
                .collect::<Vec<_>>(),
            ["station-rock-001", "station-rock-002"]
        );
    }

    struct InvalidIntentParser;

    #[async_trait]
    impl QueryParser for InvalidIntentParser {
        async fn parse(&self, _input: &QueryParserInput) -> Result<QueryIntent, QueryParserError> {
            Ok(QueryIntent {
                terms: vec!["jazz".to_owned()],
                tags: Vec::new(),
                language: Some("english".to_owned()),
                country_code: None,
            })
        }
    }

    #[tokio::test]
    async fn invalid_hard_filter_from_parser_uses_deterministic_fallback() {
        let service = SearchService::with_providers(
            Arc::new(InMemoryStationRepository::with_builtin_catalog()),
            Arc::new(InvalidIntentParser),
            None,
        );
        let outcome = service
            .interpret_and_search(
                QueryParserInput {
                    query: "rock".to_owned(),
                    locale: "en-US".to_owned(),
                },
                &SearchConstraints {
                    limit: 10,
                    excluded_station_ids: BTreeSet::new(),
                },
            )
            .await
            .unwrap();

        assert_eq!(outcome.query.language.as_deref(), Some("en"));
        assert_eq!(outcome.query.terms, ["rock"]);
    }

    struct FixedEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for FixedEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
            Ok(Embedding::new("fake", "1", 2, vec![1.0, 0.0]).unwrap())
        }
    }

    #[derive(Default)]
    struct RecordingRepository {
        embedding: Mutex<Option<Embedding>>,
    }

    #[async_trait]
    impl StationRepository for RecordingRepository {
        async fn search(
            &self,
            _query: &SearchQuery,
            _constraints: &SearchConstraints,
            embedding: Option<&Embedding>,
        ) -> Result<Vec<RankedStation>, RepositoryError> {
            *self.embedding.lock().unwrap() = embedding.cloned();
            Ok(Vec::new())
        }

        async fn check_readiness(&self) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn deterministic_fake_embedding_crosses_only_the_repository_boundary() {
        let repository = Arc::new(RecordingRepository::default());
        let service = SearchService::with_providers(
            repository.clone(),
            Arc::new(DeterministicQueryParser),
            Some(Arc::new(FixedEmbeddingProvider)),
        );

        service
            .search(
                &normalize_query("anything".to_owned(), "en-US".to_owned()),
                &SearchConstraints {
                    limit: 1,
                    excluded_station_ids: BTreeSet::new(),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            repository
                .embedding
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .provenance()
                .model,
            "fake"
        );
    }
}
