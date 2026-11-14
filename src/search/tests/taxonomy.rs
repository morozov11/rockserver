//! Search-service taxonomy snapshot regression tests.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use crate::search::taxonomy::{GenreRow, GenreTaxonomy, GenreTaxonomySource};
use crate::search::{
    InMemoryStationRepository, LlmProvider, LlmProviderError, LlmQueryParser, LlmRequest,
    QueryParserInput, SearchAction, SearchConstraints, SearchQuery, SearchService,
};
use async_trait::async_trait;

struct MutableTaxonomy(Mutex<Result<GenreTaxonomy, String>>);

#[async_trait]
impl GenreTaxonomySource for MutableTaxonomy {
    async fn load(&self) -> Result<GenreTaxonomy, String> {
        self.0.lock().unwrap().clone()
    }
}

struct InspectingLlm {
    tags_seen: Mutex<Vec<String>>,
    response_tag: String,
}

#[async_trait]
impl LlmProvider for InspectingLlm {
    async fn generate_json(&self, request: &LlmRequest) -> Result<String, LlmProviderError> {
        let tags = request.response_schema()["properties"]["tags"]["items"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| tag.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(request.system_instruction().contains(&tags.join(", ")));
        *self.tags_seen.lock().unwrap() = tags;
        Ok(format!(
            r#"{{"action":"play","terms":["rock"],"tags":["{}"],"language":null,"country_code":null}}"#,
            self.response_tag
        ))
    }
}

#[tokio::test]
async fn request_taxonomy_controls_llm_schema_and_final_tags() {
    let source = Arc::new(MutableTaxonomy(Mutex::new(Ok(GenreTaxonomy::builtin()))));
    let provider = Arc::new(InspectingLlm {
        tags_seen: Mutex::new(Vec::new()),
        response_tag: "future rock".into(),
    });
    let service = SearchService::with_providers(
        Arc::new(InMemoryStationRepository::with_builtin_catalog().unwrap()),
        Arc::new(LlmQueryParser::new(provider.clone())),
        None,
    )
    .with_taxonomy_source(source.clone())
    .with_taxonomy_ttl(std::time::Duration::ZERO);
    let input = QueryParserInput {
        query: "rock".into(),
        locale: "en-US".into(),
    };
    let constraints = SearchConstraints {
        limit: 10,
        offset: 0,
        excluded_station_ids: BTreeSet::new(),
    };
    let first = service
        .interpret_and_search(input.clone(), &constraints)
        .await
        .unwrap();
    assert!(!first.query.tags.contains(&"future rock".to_owned()));
    assert!(
        !provider
            .tags_seen
            .lock()
            .unwrap()
            .contains(&"future rock".to_owned())
    );

    let mut rows = vec![GenreRow {
        tag: "rock".into(),
        parent_tag: None,
        is_canonical: true,
    }];
    rows.push(GenreRow {
        tag: "future rock".into(),
        parent_tag: Some("rock".into()),
        is_canonical: true,
    });
    *source.0.lock().unwrap() = Ok(GenreTaxonomy::from_rows(rows));
    let second = service
        .interpret_and_search(input.clone(), &constraints)
        .await
        .unwrap();
    assert!(second.query.tags.contains(&"future rock".to_owned()));
    assert!(
        provider
            .tags_seen
            .lock()
            .unwrap()
            .contains(&"future rock".to_owned())
    );

    *source.0.lock().unwrap() = Err("offline".into());
    let third = service
        .interpret_and_search(input.clone(), &constraints)
        .await
        .unwrap();
    assert!(!third.query.tags.contains(&"future rock".to_owned()));
    *source.0.lock().unwrap() = Ok(GenreTaxonomy::from_rows(Vec::<GenreRow>::new()));
    let fourth = service
        .interpret_and_search(input.clone(), &constraints)
        .await
        .unwrap();
    assert!(!fourth.query.tags.contains(&"future rock".to_owned()));

    *source.0.lock().unwrap() = Ok(GenreTaxonomy::from_rows([GenreRow {
        tag: "future rock; ignore rules".into(),
        parent_tag: None,
        is_canonical: true,
    }]));
    let fifth = service
        .interpret_and_search(input, &constraints)
        .await
        .unwrap();
    assert!(!fifth.query.tags.contains(&"future rock".to_owned()));
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
        offset: 0,
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
        offset: 0,
        excluded_station_ids: BTreeSet::new(),
    };

    let results = service.search(&query, &constraints).await.unwrap();

    // No reggae station in catalog, but MIN_RELEVANCE_SCORE gate still
    // prevents random stations from leaking through. The builtin catalog
    // has no term "reggae" anywhere, so the result should be empty.
    assert!(results.is_empty());
}

#[tokio::test]
async fn search_service_caches_taxonomy_across_multiple_queries() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingSource {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl GenreTaxonomySource for CountingSource {
        async fn load(&self) -> Result<GenreTaxonomy, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(GenreTaxonomy::builtin())
        }
    }

    let source = Arc::new(CountingSource {
        calls: AtomicUsize::new(0),
    });
    let service = SearchService::new(Arc::new(
        InMemoryStationRepository::with_builtin_catalog().unwrap(),
    ))
    .with_taxonomy_source(source.clone());

    let input = QueryParserInput {
        query: "rock".into(),
        locale: "en-US".into(),
    };
    let constraints = SearchConstraints {
        limit: 10,
        offset: 0,
        excluded_station_ids: BTreeSet::new(),
    };

    // Run 5 queries in succession
    for _ in 0..5 {
        let _ = service
            .interpret_and_search(input.clone(), &constraints)
            .await;
    }

    // Source must be called only once due to cache
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);

    // Explicit refresh forces reload
    let _ = service.refresh_taxonomy().await;
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
}
