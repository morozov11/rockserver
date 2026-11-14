//! Search orchestration with deterministic parser and metadata fallbacks.

use std::{sync::Arc, time::Instant};

use super::{
    domain::{
        MIN_RELEVANCE_SCORE, RankedStation, RepositoryError, SearchConstraints, SearchOutcome,
        SearchQuery, Station, StationRepository,
    },
    embedding::{Embedding, EmbeddingProvider},
    language::explicit_language_code,
    query::{
        DeterministicQueryParser, QueryParser, QueryParserInput, tokenize,
        validate_intent_with_taxonomy,
    },
    taxonomy::{GenreTaxonomy, GenreTaxonomySource},
};

/// Search orchestration with deterministic parser and metadata fallbacks.
#[derive(Clone)]
pub struct SearchService {
    repository: Arc<dyn StationRepository + Send + Sync>,
    query_parser: Arc<dyn QueryParser>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    taxonomy_source: Option<Arc<dyn GenreTaxonomySource>>,
}

impl SearchService {
    /// Creates metadata-only search using the deterministic query parser.
    pub fn new(repository: Arc<dyn StationRepository + Send + Sync>) -> Self {
        Self {
            repository,
            query_parser: Arc::new(DeterministicQueryParser),
            embedding_provider: None,
            taxonomy_source: None,
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
            taxonomy_source: None,
        }
    }

    /// Uses a live controlled genre vocabulary for parsing, validation, and fallback.
    pub fn with_taxonomy_source(mut self, source: Arc<dyn GenreTaxonomySource>) -> Self {
        self.taxonomy_source = Some(source);
        self
    }

    async fn taxonomy(&self) -> GenreTaxonomy {
        match &self.taxonomy_source {
            Some(source) => match source.load().await {
                Ok(taxonomy) if taxonomy.is_usable() => taxonomy,
                Ok(_) => {
                    tracing::warn!("genre taxonomy is unusable; using builtin fallback");
                    GenreTaxonomy::builtin()
                }
                Err(_) => {
                    tracing::warn!("genre taxonomy unavailable; using builtin fallback");
                    GenreTaxonomy::builtin()
                }
            },
            None => GenreTaxonomy::builtin(),
        }
    }

    /// Returns a bounded public catalog page without exposing repository internals.
    pub async fn public_catalog(
        &self,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Station>, RepositoryError> {
        self.repository.list_public(after_id, limit).await
    }

    /// Returns a server-filtered administrator catalog page without exposing repository internals.
    pub async fn admin_catalog(
        &self,
        query: Option<&str>,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Station>, RepositoryError> {
        self.repository.list_admin(query, after_id, limit).await
    }

    /// Returns one active public catalog station by stable identifier.
    pub async fn public_station(&self, id: &str) -> Result<Option<Station>, RepositoryError> {
        self.repository.get_public(id).await
    }

    /// Returns matching stations ordered by score descending and station ID ascending.
    ///
    /// When the exact genre filter produces no results, the search progressively
    /// relaxes the filter using the genre hierarchy (e.g. `"heavy metal"` falls
    /// back to stations tagged `"rock"`).  If even the broadest ancestor yields
    /// nothing, the genre constraint is dropped entirely while keeping the
    /// minimum relevance score gate.
    pub async fn search(
        &self,
        query: &SearchQuery,
        constraints: &SearchConstraints,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        let taxonomy = self.taxonomy().await;
        let embedding_started_at = Instant::now();
        let (embedding, _) = self.query_embedding(&query.original).await;
        self.search_with_embedding(
            query,
            constraints,
            embedding.as_ref(),
            embedding_started_at.elapsed().as_millis(),
            &taxonomy,
        )
        .await
    }

    /// Searches with an already computed request embedding to avoid duplicate local inference.
    async fn search_with_embedding(
        &self,
        query: &SearchQuery,
        constraints: &SearchConstraints,
        embedding: Option<&Embedding>,
        embedding_elapsed_ms: u128,
        taxonomy: &GenreTaxonomy,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        let repository_started_at = Instant::now();
        let mut stations = self
            .repository
            .search(query, constraints, embedding)
            .await?;
        tracing::debug!(
            embedding_elapsed_ms,
            repository_elapsed_ms = repository_started_at.elapsed().as_millis(),
            result_count = stations.len(),
            "station search stages completed"
        );
        stations.retain(|station| station.score >= MIN_RELEVANCE_SCORE);

        let with_genre: Vec<_> = stations
            .iter()
            .filter(|s| taxonomy.station_matches_requested_genre(&query.tags, &s.station.tags))
            .cloned()
            .collect();
        if !with_genre.is_empty() {
            return Ok(with_genre);
        }

        // Progressively broaden by walking up the genre hierarchy.
        let parent_tags = broaden_tags(&query.tags, taxonomy);
        if !parent_tags.is_empty() {
            let with_parents: Vec<_> = stations
                .iter()
                .filter(|s| taxonomy.station_matches_requested_genre(&parent_tags, &s.station.tags))
                .cloned()
                .collect();
            if !with_parents.is_empty() {
                return Ok(with_parents);
            }
        }

        // Last resort: drop genre filter, keep only MIN_RELEVANCE_SCORE gate.
        Ok(stations)
    }

    /// Interprets request-only input and searches without logging its text or derived terms.
    pub async fn interpret_and_search(
        &self,
        input: QueryParserInput,
        constraints: &SearchConstraints,
    ) -> Result<SearchOutcome, RepositoryError> {
        self.interpret_and_search_observed(input, constraints).await
    }

    /// Interprets and searches without recording user-supplied text or derived terms.
    pub async fn interpret_and_search_private(
        &self,
        input: QueryParserInput,
        constraints: &SearchConstraints,
    ) -> Result<SearchOutcome, RepositoryError> {
        self.interpret_and_search_observed(input, constraints).await
    }

    async fn interpret_and_search_observed(
        &self,
        input: QueryParserInput,
        constraints: &SearchConstraints,
    ) -> Result<SearchOutcome, RepositoryError> {
        let started_at = Instant::now();
        let taxonomy = self.taxonomy().await;
        let parser_started_at = Instant::now();
        // Query parsing (which may call external LLM) and local text embedding generation
        // are independent: the embedding only needs the raw query text. Run them concurrently
        // via `tokio::join!` so local embedding generation finishes during the network round-trip.
        let (parse_result, ((embedding, embedding_failed), embedding_elapsed_ms)) = tokio::join!(
            self.query_parser.parse_with_taxonomy(&input, &taxonomy),
            async {
                let start = Instant::now();
                let embedding = self.query_embedding(&input.query).await;
                (embedding, start.elapsed().as_millis())
            },
        );
        let parser_elapsed_ms = parser_started_at.elapsed().as_millis();

        let mut parser_fallback = "none";
        let intent = match parse_result
            .and_then(|intent| validate_intent_with_taxonomy(intent, &taxonomy))
        {
            Ok(intent) => intent,
            Err(_) => {
                parser_fallback = "error";
                tracing::warn!("query parser failed; using deterministic metadata fallback");
                DeterministicQueryParser
                    .parse(&input)
                    .await
                    .expect("deterministic query parser cannot fail")
            }
        };
        // Providers (LLMs) may occasionally return:
        // - both empty `terms` and `tags`
        // - only `tags` but no `terms` (hurts station name matching)
        //
        // For station-name matching deterministic tokenization is more reliable.
        let mut intent = intent;
        if intent.terms.is_empty() {
            let deterministic = DeterministicQueryParser
                .parse(&input)
                .await
                .expect("deterministic query parser cannot fail");
            let deterministic = validate_intent_with_taxonomy(deterministic.clone(), &taxonomy)
                .unwrap_or(deterministic);

            if intent.tags.is_empty() {
                parser_fallback = "empty_intent";
                // Full fallback: provider returned nothing actionable.
                intent = deterministic;
            } else {
                parser_fallback = "missing_terms";
                // Partial fallback: keep provider's hard genre tags, but use deterministic
                // `terms` for token/sub-token and trigram name matching.
                intent.terms = deterministic.terms;
                intent.raw_query = deterministic.raw_query;
                intent.core_term_count = deterministic.core_term_count;
            }
        }

        let request_terms = tokenize(&input.query);
        let explicit_language = explicit_language_code(&request_terms).map(str::to_owned);
        intent.language = explicit_language;

        let query = SearchQuery::from_intent(input.query, input.locale, intent);
        let vector_branch = embedding.as_ref().is_some_and(|value| {
            let provenance = value.provenance();
            provenance.model == "intfloat/multilingual-e5-small" && provenance.dimension == 384
        });
        let stations = self
            .search_with_embedding(
                &query,
                constraints,
                embedding.as_ref(),
                embedding_elapsed_ms,
                &taxonomy,
            )
            .await;
        tracing::info!(
            event = "search_observation",
            outcome = if stations.is_ok() {
                "ok"
            } else {
                "repository_error"
            },
            elapsed_ms = started_at.elapsed().as_millis(),
            parser_elapsed_ms,
            embedding_elapsed_ms,
            parser_fallback,
            embedding_failed,
            vector_branch,
            empty = stations.as_ref().is_ok_and(Vec::is_empty),
            result_count = stations.as_ref().map_or(0, Vec::len),
            "search observation"
        );
        let stations = stations?;
        let total = stations
            .first()
            .and_then(|s| s.total_matches)
            .unwrap_or(stations.len());
        Ok(SearchOutcome {
            query,
            stations,
            total,
        })
    }

    /// Checks whether the configured catalog backend is currently available.
    pub async fn check_readiness(&self) -> Result<(), RepositoryError> {
        self.repository.check_readiness().await
    }

    async fn query_embedding(&self, text: &str) -> (Option<Embedding>, bool) {
        let Some(provider) = self.embedding_provider.as_ref() else {
            return (None, false);
        };
        match provider.embed(text).await {
            Ok(embedding) => (Some(embedding), false),
            Err(_) => {
                tracing::warn!("embedding provider failed; using metadata fallback");
                (None, true)
            }
        }
    }
}

/// Replaces each genre tag with its nearest parent from the hierarchy.
///
/// Mood tags pass through unchanged. Tags without a parent are dropped
/// because broadening is only meaningful for hierarchical genres.
fn broaden_tags(tags: &[String], taxonomy: &GenreTaxonomy) -> Vec<String> {
    let mut broadened = std::collections::BTreeSet::new();
    for tag in tags {
        for ancestor in taxonomy.ancestors(tag) {
            broadened.insert(ancestor.to_owned());
        }
    }
    broadened.into_iter().collect()
}
