//! Search-service regression tests kept beside the domain behavior they exercise.

use std::{
    collections::BTreeSet,
    io,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use super::{
    DeterministicQueryParser, Embedding, EmbeddingProvider, EmbeddingProviderError,
    InMemoryStationRepository, QueryIntent, QueryParser, QueryParserError, QueryParserInput,
    RankedStation, RepositoryError, SearchAction, SearchConstraints, SearchQuery, SearchService,
    SemanticLanguageClassifier, Station, StationHealth, StationRepository,
    UnavailableStationRepository, normalize_query,
};

#[tokio::test]
async fn equal_scores_are_ordered_by_station_id() {
    let service = SearchService::new(Arc::new(
        InMemoryStationRepository::with_legacy_fixture_catalog(),
    ));
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

    // "Радио Рок" scores higher because transliteration adds "рок"
    // which substring-matches in its name, boosting its score.
    assert_eq!(
        ids,
        [
            "station-rock-ru-001",
            "station-rock-001",
            "station-rock-002",
        ]
    );
}

#[tokio::test]
async fn exclusions_are_applied_before_ranking() {
    let service = SearchService::new(Arc::new(
        InMemoryStationRepository::with_legacy_fixture_catalog(),
    ));
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

#[tokio::test]
async fn unavailable_catalog_is_explicitly_unready_and_never_serves_a_fixture() {
    let repository = UnavailableStationRepository::from_preflight_error(RepositoryError::new(
        "fixture preflight",
        io::Error::other("invalid catalog fixture"),
    ));
    let constraints = SearchConstraints {
        limit: 10,
        excluded_station_ids: BTreeSet::new(),
    };

    assert!(repository.check_readiness().await.is_err());
    assert!(
        repository
            .search(
                &normalize_query("rock".to_owned(), "en-US".to_owned()),
                &constraints,
                None,
            )
            .await
            .is_err()
    );
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
            action: SearchAction::Play,
            terms: vec!["rock".to_owned()],
            tags: vec!["rock".to_owned()],
            language: Some("en".to_owned()),
            country_code: None,
            core_term_count: 1,
            raw_query: "rock".to_owned(),
        })
    }
}

#[tokio::test]
async fn query_parser_receives_only_request_input_and_returns_structured_intent() {
    let input_seen = Arc::new(Mutex::new(None));
    let service = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_legacy_fixture_catalog()),
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
    assert!(outcome.query.terms.contains(&"rock".to_owned()));
    assert!(outcome.query.terms.contains(&"рок".to_owned()));
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
        Arc::new(InMemoryStationRepository::with_legacy_fixture_catalog()),
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
        [
            "station-rock-ru-001",
            "station-rock-001",
            "station-rock-002",
        ]
    );
}

struct InvalidIntentParser;

#[async_trait]
impl QueryParser for InvalidIntentParser {
    async fn parse(&self, _input: &QueryParserInput) -> Result<QueryIntent, QueryParserError> {
        Ok(QueryIntent {
            action: SearchAction::Play,
            terms: vec!["jazz".to_owned()],
            tags: Vec::new(),
            language: Some("english".to_owned()),
            country_code: None,
            core_term_count: 1,
            raw_query: "jazz".to_owned(),
        })
    }
}

#[tokio::test]
async fn invalid_hard_filter_from_parser_uses_deterministic_fallback() {
    let service = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_builtin_catalog().unwrap()),
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

    assert_eq!(outcome.query.language, None);
    assert!(outcome.query.terms.contains(&"rock".to_owned()));
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
async fn heavy_metal_query_finds_rock_stations_via_genre_hierarchy() {
    let service = SearchService::new(Arc::new(
        InMemoryStationRepository::with_legacy_fixture_catalog(),
    ));
    let query = SearchQuery {
        action: SearchAction::Play,
        original: "heavy metal".to_owned(),
        locale: "en-US".to_owned(),
        terms: vec!["heavy".to_owned(), "metal".to_owned()],
        tags: vec!["heavy metal".to_owned()],
        language: None,
        country_code: None,
        core_term_count: 2,
        raw_query: "heavy metal".to_owned(),
        prefer_station_name: false,
        station_name_hint_queries: Vec::new(),
    };
    let constraints = SearchConstraints {
        limit: 10,
        excluded_station_ids: BTreeSet::new(),
    };

    let ids: Vec<_> = service
        .search(&query, &constraints)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.station.id)
        .collect();

    // metal-001 matches exactly; rock stations match via hierarchy fallback.
    assert!(ids.contains(&"station-metal-001".to_owned()));
    assert!(!ids.is_empty());
}

#[tokio::test]
async fn english_heavy_query_prefers_english_stations() {
    let service = SearchService::new(Arc::new(
        InMemoryStationRepository::with_legacy_fixture_catalog(),
    ));
    let query = SearchQuery {
        action: SearchAction::Play,
        original: "english heavy".to_owned(),
        locale: "en-US".to_owned(),
        terms: vec!["english".to_owned(), "heavy".to_owned()],
        tags: vec!["heavy metal".to_owned()],
        language: Some("en".to_owned()),
        country_code: None,
        core_term_count: 2,
        raw_query: "english heavy".to_owned(),
        prefer_station_name: false,
        station_name_hint_queries: Vec::new(),
    };
    let constraints = SearchConstraints {
        limit: 10,
        excluded_station_ids: BTreeSet::new(),
    };

    let results = service.search(&query, &constraints).await.unwrap();

    assert!(!results.is_empty());
    // All returned stations must be English-language (language hard filter).
    for station in &results {
        assert_eq!(station.station.language.as_deref(), Some("en"));
    }
    // The exact heavy-metal station must appear.
    assert!(results.iter().any(|s| s.station.id == "station-metal-001"));
}

#[tokio::test]
async fn genre_fallback_drops_filter_when_no_hierarchy_match() {
    let service = SearchService::new(Arc::new(
        InMemoryStationRepository::with_builtin_catalog().unwrap(),
    ));
    let query = SearchQuery {
        action: SearchAction::Play,
        original: "reggae".to_owned(),
        locale: "en-US".to_owned(),
        terms: vec!["reggae".to_owned()],
        tags: vec!["reggae".to_owned()],
        language: None,
        country_code: None,
        core_term_count: 1,
        raw_query: "reggae".to_owned(),
        prefer_station_name: false,
        station_name_hint_queries: Vec::new(),
    };
    let constraints = SearchConstraints {
        limit: 10,
        excluded_station_ids: BTreeSet::new(),
    };

    let results = service.search(&query, &constraints).await.unwrap();

    // No reggae station in catalog, but MIN_RELEVANCE_SCORE gate still
    // prevents random stations from leaking through. The builtin catalog
    // has no term "reggae" anywhere, so the result should be empty.
    assert!(results.is_empty());
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

#[tokio::test]
async fn semantic_language_classifier_is_not_invoked_in_search_path() {
    let classifier = SemanticLanguageClassifier::from_embeddings(vec![
        (
            "en",
            Embedding::new("fake", "1", 2, vec![1.0, 0.0]).unwrap(),
        ),
        (
            "es",
            Embedding::new("fake", "1", 2, vec![0.0, 1.0]).unwrap(),
        ),
    ]);
    let service = SearchService::with_providers_and_language_classifier(
        Arc::new(InMemoryStationRepository::with_builtin_catalog().unwrap()),
        Arc::new(DeterministicQueryParser),
        Some(Arc::new(FixedEmbeddingProvider)),
        Some(Arc::new(classifier)),
    );

    let outcome = service
        .interpret_and_search(
            QueryParserInput {
                query: "Включи английский рок".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .unwrap();

    // Per SRCH-004, the semantic language classifier is bypassed in the search request path.
    assert_eq!(outcome.query.language, None);
    assert_eq!(outcome.query.country_code, None);
}

fn acceptance_c_fixture() -> Vec<Station> {
    vec![
        Station {
            id: "s-generic-de".to_owned(),
            name: "Rock Antenne Test".to_owned(),
            stream_url: "https://streams.example.com/de.mp3".to_owned(),
            homepage_url: None,
            favicon_url: None,
            tags: vec!["rock".to_owned()],
            language: Some("de".to_owned()),
            country_code: Some("DE".to_owned()),
            codec: Some("MP3".to_owned()),
            bitrate_kbps: Some(192),
            health: StationHealth::Healthy,
        },
        Station {
            id: "s-at-rock".to_owned(),
            name: "Radio Wien Rock Test".to_owned(),
            stream_url: "https://streams.example.com/at.mp3".to_owned(),
            homepage_url: None,
            favicon_url: None,
            tags: vec!["rock".to_owned()],
            language: Some("de".to_owned()),
            country_code: Some("AT".to_owned()),
            codec: Some("MP3".to_owned()),
            bitrate_kbps: Some(192),
            health: StationHealth::Healthy,
        },
        Station {
            id: "s-gb-rock".to_owned(),
            name: "Planet Rock Test".to_owned(),
            stream_url: "https://streams.example.com/gb.mp3".to_owned(),
            homepage_url: None,
            favicon_url: None,
            tags: vec!["rock".to_owned()],
            language: Some("en".to_owned()),
            country_code: Some("GB".to_owned()),
            codec: Some("MP3".to_owned()),
            bitrate_kbps: Some(192),
            health: StationHealth::Healthy,
        },
        Station {
            id: "s-jazz".to_owned(),
            name: "Jazz FM Test".to_owned(),
            stream_url: "https://streams.example.com/jazz.mp3".to_owned(),
            homepage_url: None,
            favicon_url: None,
            tags: vec!["jazz".to_owned()],
            language: Some("en".to_owned()),
            country_code: Some("GB".to_owned()),
            codec: Some("MP3".to_owned()),
            bitrate_kbps: Some(192),
            health: StationHealth::Healthy,
        },
    ]
}

#[tokio::test]
async fn table_a_search_service_deterministic_query_parser_acceptance() {
    let repo = Arc::new(InMemoryStationRepository::from_stations(
        acceptance_c_fixture(),
    ));
    let service = SearchService::new(repo);
    let constraints = SearchConstraints {
        limit: 10,
        excluded_station_ids: BTreeSet::new(),
    };

    let cases: &[(&str, Option<&str>, Option<&str>, usize)] = &[
        ("вруби станцию из германии", None, Some("DE"), 3),
        ("станции германии", None, Some("DE"), 2),
        ("рок из австрии", None, Some("AT"), 3),
        ("вруби немецкий рок", None, None, 2),
        ("рок на немецком", Some("de"), None, 3),
        ("рок по-немецки", Some("de"), None, 3),
        ("немецкоязычное радио", Some("de"), None, 2),
        ("рок in German", Some("de"), None, 3),
        ("включи джаз", None, None, 1),
        ("американский рок", None, None, 2),
        ("Включи английский рок", None, None, 2),
        ("русскоязычный рок", Some("ru"), None, 2),
        ("рок из прошлого", None, None, 3),
        ("немецкий рок из австрии", None, Some("AT"), 4),
    ];

    for &(query_str, expected_lang, expected_country, expected_core_terms) in cases {
        let outcome = service
            .interpret_and_search(
                QueryParserInput {
                    query: query_str.to_owned(),
                    locale: "ru-RU".to_owned(),
                },
                &constraints,
            )
            .await
            .unwrap();

        assert_eq!(
            outcome.query.language.as_deref(),
            expected_lang,
            "SearchService language mismatch for: {query_str}"
        );
        assert_eq!(
            outcome.query.country_code.as_deref(),
            expected_country,
            "SearchService country mismatch for: {query_str}"
        );
        assert_eq!(
            outcome.query.core_term_count, expected_core_terms,
            "SearchService core_term_count mismatch for: {query_str}"
        );
    }

    // Also verify the negative prepositional context queries through SearchService
    let outcome_neg1 = service
        .interpret_and_search(
            QueryParserInput {
                query: "рок на немецком фестивале".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &constraints,
        )
        .await
        .unwrap();
    assert_eq!(outcome_neg1.query.language, None);

    let outcome_neg2 = service
        .interpret_and_search(
            QueryParserInput {
                query: "включи радио Tune In German Rock".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &constraints,
        )
        .await
        .unwrap();
    assert_eq!(outcome_neg2.query.language, None);
}

#[tokio::test]
async fn acceptance_c_search_service_in_memory_suite() {
    let repo = Arc::new(InMemoryStationRepository::from_stations(
        acceptance_c_fixture(),
    ));
    let service = SearchService::new(repo.clone());
    let constraints = SearchConstraints {
        limit: 10,
        excluded_station_ids: BTreeSet::new(),
    };

    // 1. «вруби немецкий рок»: в выдаче все три рок-станции (включая s-gb-rock — доказательство отсутствия фильтра), нет s-jazz.
    let outcome1 = service
        .interpret_and_search(
            QueryParserInput {
                query: "вруби немецкий рок".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &constraints,
        )
        .await
        .unwrap();
    assert_eq!(outcome1.query.language, None);
    assert_eq!(outcome1.query.country_code, None);
    let ids1 = outcome1
        .stations
        .iter()
        .map(|s| s.station.id.as_str())
        .collect::<Vec<_>>();
    assert!(ids1.contains(&"s-generic-de"));
    assert!(ids1.contains(&"s-at-rock"));
    assert!(ids1.contains(&"s-gb-rock"));
    assert!(!ids1.contains(&"s-jazz"));
    assert_eq!(ids1.len(), 3);

    // 2. «рок из германии»: только s-generic-de.
    let outcome2 = service
        .interpret_and_search(
            QueryParserInput {
                query: "рок из германии".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &constraints,
        )
        .await
        .unwrap();
    assert_eq!(outcome2.query.country_code.as_deref(), Some("DE"));
    let ids2 = outcome2
        .stations
        .iter()
        .map(|s| s.station.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids2, ["s-generic-de"]);

    // 3. «вруби станцию из германии»: разбор даёт country_code = DE; выдача на этой фикстуре пустая.
    let outcome3 = service
        .interpret_and_search(
            QueryParserInput {
                query: "вруби станцию из германии".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &constraints,
        )
        .await
        .unwrap();
    assert_eq!(outcome3.query.country_code.as_deref(), Some("DE"));
    assert!(
        outcome3.stations.is_empty(),
        "expected empty result for 'вруби станцию из германии' on this fixture"
    );

    // 4. «рок на немецком»: s-generic-de и s-at-rock, без s-gb-rock.
    let outcome4 = service
        .interpret_and_search(
            QueryParserInput {
                query: "рок на немецком".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &constraints,
        )
        .await
        .unwrap();
    assert_eq!(outcome4.query.language.as_deref(), Some("de"));
    assert_eq!(outcome4.query.country_code, None);
    let ids4 = outcome4
        .stations
        .iter()
        .map(|s| s.station.id.as_str())
        .collect::<Vec<_>>();
    assert!(ids4.contains(&"s-generic-de"));
    assert!(ids4.contains(&"s-at-rock"));
    assert!(!ids4.contains(&"s-gb-rock"));
    assert_eq!(ids4.len(), 2);

    // 5. Тот же результат через normalize_query + SearchService::search (голосовой путь).
    let q1 = normalize_query("вруби немецкий рок".to_owned(), "ru-RU".to_owned());
    let v_ids1 = service
        .search(&q1, &constraints)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.station.id)
        .collect::<Vec<_>>();
    assert!(v_ids1.contains(&"s-generic-de".to_owned()));
    assert!(v_ids1.contains(&"s-at-rock".to_owned()));
    assert!(v_ids1.contains(&"s-gb-rock".to_owned()));
    assert!(!v_ids1.contains(&"s-jazz".to_owned()));
    assert_eq!(v_ids1.len(), 3);

    let q2 = normalize_query("рок из германии".to_owned(), "ru-RU".to_owned());
    let v_ids2 = service
        .search(&q2, &constraints)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.station.id)
        .collect::<Vec<_>>();
    assert_eq!(v_ids2, ["s-generic-de"]);

    let q3 = normalize_query("вруби станцию из германии".to_owned(), "ru-RU".to_owned());
    let v_ids3 = service.search(&q3, &constraints).await.unwrap();
    assert!(v_ids3.is_empty());

    let q4 = normalize_query("рок на немецком".to_owned(), "ru-RU".to_owned());
    let v_ids4 = service
        .search(&q4, &constraints)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.station.id)
        .collect::<Vec<_>>();
    assert!(v_ids4.contains(&"s-generic-de".to_owned()));
    assert!(v_ids4.contains(&"s-at-rock".to_owned()));
    assert!(!v_ids4.contains(&"s-gb-rock".to_owned()));
    assert_eq!(v_ids4.len(), 2);

    // 6. Классификатор не влияет на результат: настоящий SemanticLanguageClassifier::from_embeddings
    // с контролируемыми эмбеддингами, который дал бы уверенный en для запроса, передан в SearchService — language остаётся None.
    let classifier = SemanticLanguageClassifier::from_embeddings(vec![
        (
            "en",
            Embedding::new("fake", "1", 2, vec![1.0, 0.0]).unwrap(),
        ),
        (
            "es",
            Embedding::new("fake", "1", 2, vec![0.0, 1.0]).unwrap(),
        ),
    ]);
    let service_with_classifier = SearchService::with_providers_and_language_classifier(
        repo.clone(),
        Arc::new(DeterministicQueryParser),
        Some(Arc::new(FixedEmbeddingProvider)),
        Some(Arc::new(classifier)),
    );
    let outcome_classifier = service_with_classifier
        .interpret_and_search(
            QueryParserInput {
                query: "Включи английский рок".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &constraints,
        )
        .await
        .unwrap();
    assert_eq!(outcome_classifier.query.language, None);
}

struct FakeIntentParser {
    intent: QueryIntent,
}

#[async_trait]
impl QueryParser for FakeIntentParser {
    async fn parse(&self, _input: &QueryParserInput) -> Result<QueryIntent, QueryParserError> {
        Ok(self.intent.clone())
    }
}

struct AlwaysFailingQueryParser;

#[async_trait]
impl QueryParser for AlwaysFailingQueryParser {
    async fn parse(&self, _input: &QueryParserInput) -> Result<QueryIntent, QueryParserError> {
        Err(QueryParserError::safe("upstream provider unreachable"))
    }
}

#[tokio::test]
async fn deterministic_and_fallback_paths_produce_identical_terms_and_counts() {
    let service_det = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_legacy_fixture_catalog()),
        Arc::new(DeterministicQueryParser),
        None,
    );
    let constraints = SearchConstraints {
        limit: 10,
        excluded_station_ids: BTreeSet::new(),
    };
    let input = QueryParserInput {
        query: "включи немецкий рок".to_owned(),
        locale: "ru-RU".to_owned(),
    };

    let outcome_det = service_det
        .interpret_and_search(input.clone(), &constraints)
        .await
        .unwrap();

    assert_eq!(outcome_det.query.core_term_count, 2);
    assert_eq!(outcome_det.query.raw_query, "немецкий рок");
    assert!(outcome_det.query.terms.contains(&"рок".to_owned()));
    assert!(outcome_det.query.terms.contains(&"rock".to_owned()));
    assert!(!outcome_det.query.terms.contains(&"рокк".to_owned()));

    let service_fail = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_legacy_fixture_catalog()),
        Arc::new(AlwaysFailingQueryParser),
        None,
    );
    let outcome_fail = service_fail
        .interpret_and_search(input.clone(), &constraints)
        .await
        .unwrap();

    // Verify complete terms vector equality between deterministic and emergency fallback
    assert_eq!(outcome_fail.query.terms, outcome_det.query.terms);
    assert_eq!(
        outcome_fail.query.core_term_count,
        outcome_det.query.core_term_count
    );
    assert_eq!(outcome_fail.query.raw_query, outcome_det.query.raw_query);

    // Verify complete terms vector equality with normalize_query
    let query_norm = normalize_query("включи немецкий рок".to_owned(), "ru-RU".to_owned());
    assert_eq!(query_norm.terms, outcome_det.query.terms);
    assert_eq!(
        query_norm.core_term_count,
        outcome_det.query.core_term_count
    );
    assert_eq!(query_norm.raw_query, outcome_det.query.raw_query);
}

#[tokio::test]
async fn deterministic_query_parser_normalizes_single_term_jazz() {
    let service = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_legacy_fixture_catalog()),
        Arc::new(DeterministicQueryParser),
        None,
    );
    let outcome = service
        .interpret_and_search(
            QueryParserInput {
                query: "поставь джаз".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.query.core_term_count, 1);
    assert_eq!(outcome.query.raw_query, "джаз");
    assert!(outcome.query.terms.contains(&"джаз".to_owned()));
    assert!(outcome.query.terms.contains(&"jazz".to_owned()));
}

#[tokio::test]
async fn partial_fallback_preserves_provider_tags_and_sets_correct_core_term_count() {
    let service = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_legacy_fixture_catalog()),
        Arc::new(FakeIntentParser {
            intent: QueryIntent {
                action: SearchAction::Play,
                terms: Vec::new(),
                // Provider returned a non-deterministic tag ("jazz") alongside "rock"
                tags: vec!["jazz".to_owned(), "rock".to_owned()],
                language: None,
                country_code: None,
                core_term_count: 0,
                raw_query: String::new(),
            },
        }),
        None,
    );
    let outcome = service
        .interpret_and_search(
            QueryParserInput {
                query: "включи немецкий рок".to_owned(),
                locale: "ru-RU".to_owned(),
            },
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.query.core_term_count, 2);
    assert_eq!(outcome.query.raw_query, "немецкий рок");
    // Verify provider's distinct tag ("jazz") is preserved alongside "rock"
    assert_eq!(outcome.query.tags, ["jazz", "rock"]);
    assert!(outcome.query.terms.contains(&"рок".to_owned()));
    assert!(outcome.query.terms.contains(&"rock".to_owned()));
    assert!(!outcome.query.terms.contains(&"рокк".to_owned()));
}

#[tokio::test]
async fn llm_path_preserves_single_core_term_count() {
    let service = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_legacy_fixture_catalog()),
        Arc::new(FakeIntentParser {
            intent: QueryIntent {
                action: SearchAction::Play,
                terms: vec!["rock".to_owned()],
                tags: vec!["rock".to_owned()],
                language: None,
                country_code: None,
                core_term_count: 1,
                raw_query: "rock".to_owned(),
            },
        }),
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

    assert_eq!(outcome.query.core_term_count, 1);
    assert_eq!(outcome.query.raw_query, "rock");
}

#[tokio::test]
async fn llm_path_preserves_cased_compound_terms_without_splitting() {
    let service = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_legacy_fixture_catalog()),
        Arc::new(FakeIntentParser {
            intent: QueryIntent {
                action: SearchAction::Play,
                terms: vec!["RockRadio".to_owned()],
                tags: vec!["rock".to_owned()],
                language: None,
                country_code: None,
                core_term_count: 1,
                raw_query: "RockRadio".to_owned(),
            },
        }),
        None,
    );
    let outcome = service
        .interpret_and_search(
            QueryParserInput {
                query: "RockRadio".to_owned(),
                locale: "en-US".to_owned(),
            },
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.query.core_term_count, 1);
    assert_eq!(outcome.query.raw_query, "rockradio");
    assert!(outcome.query.terms.contains(&"rockradio".to_owned()));
}
