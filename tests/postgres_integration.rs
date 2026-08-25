//! Opt-in integration coverage against a real PostgreSQL test database.

use std::{collections::BTreeSet, env, sync::Arc};

use async_trait::async_trait;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use rockserver::{
    catalog::{
        CatalogImportError, CatalogImportProvider, CatalogImporter, ImportLimits, ImportPage,
        ImportedStation, ImportedStream, PinnedSharedCatalog,
    },
    http::{HealthResponse, HealthStatus, router_with_repository, router_with_search_service},
    persistence::{
        OwnedCatalogReplacement, PostgresEmbeddingStore, PostgresImportStore,
        PostgresStationRepository,
    },
    providers::radio_browser::SOURCE,
    search::{
        DeterministicQueryParser, Embedding, EmbeddingProvider, EmbeddingProviderError,
        EmbeddingStore, SearchAction, SearchConstraints, SearchQuery, SearchService,
        normalize_query,
    },
};
use serde_json::Value;
use sha2::Digest;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

/// Exercises migrations, seed data, search semantics, and live database readiness.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn postgres_migrations_seed_search_and_readiness() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("set TEST_DATABASE_URL to an isolated PostgreSQL database");
    let repository = PostgresStationRepository::connect(&database_url)
        .await
        .expect("migrations and seed must succeed");
    let service = SearchService::new(Arc::new(repository.clone()));
    let extension_pool = repository_pool(&database_url).await;
    let extension_version = sqlx::query_scalar::<_, String>(
        "SELECT extversion FROM pg_extension WHERE extname = 'vector'",
    )
    .fetch_one(&extension_pool)
    .await
    .expect("pgvector migration must enable the extension");
    assert!(!extension_version.is_empty());
    extension_pool.close().await;

    let rock_query = normalize_query("rock".to_owned(), "en-US".to_owned());
    let rock_results = service
        .search(
            &rock_query,
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .expect("seeded search must succeed");
    assert!(
        rock_results
            .iter()
            .any(|ranked| ranked.station.id == "rock-antenne")
    );

    let limited_results = service
        .search(
            &rock_query,
            &SearchConstraints {
                limit: 1,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .expect("limited search must succeed");
    assert_eq!(
        station_ids(&limited_results),
        station_ids(&rock_results)[..1]
    );

    let metal_query = normalize_query("metal".to_owned(), "en-US".to_owned());
    let exclusion_results = service
        .search(
            &metal_query,
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::from(["somafm-metal-detector".to_owned()]),
            },
        )
        .await
        .expect("excluded search must succeed");
    assert!(!station_ids(&exclusion_results).contains(&"somafm-metal-detector"));

    let embedding_store = PostgresEmbeddingStore::connect(&database_url)
        .await
        .expect("embedding store migrations must succeed");
    embedding_store
        .upsert_embedding(
            "rock-antenne",
            &Embedding::new("integration-model", "1", 3, vec![0.0, 1.0, 0.0]).unwrap(),
        )
        .await
        .expect("first embedding insert must succeed");
    embedding_store
        .upsert_embedding(
            "rock-antenne",
            &Embedding::new("integration-model", "1", 3, vec![1.0, 0.0, 0.0]).unwrap(),
        )
        .await
        .expect("repeat embedding update must succeed");
    for (station_id, values) in [
        ("radio-record-rock", vec![0.0, 1.0, 0.0]),
        ("gotradio-punk-rock", vec![0.0, 0.0, 1.0]),
        ("rock-antenne-alternative", vec![0.0, 0.0, 1.0]),
    ] {
        embedding_store
            .upsert_embedding(
                station_id,
                &Embedding::new("integration-model", "1", 3, values).unwrap(),
            )
            .await
            .expect("station embedding insert must succeed");
    }

    let semantic_service = SearchService::with_providers(
        Arc::new(repository.clone()),
        Arc::new(DeterministicQueryParser),
        Some(Arc::new(FixedEmbeddingProvider)),
    );
    let semantic_query = SearchQuery {
        action: SearchAction::Play,
        original: "rock".to_owned(),
        locale: "en-US".to_owned(),
        terms: vec!["rock".to_owned()],
        tags: Vec::new(),
        language: None,
        country_code: None,
        core_term_count: 1,
        raw_query: "rock".to_owned(),
        prefer_station_name: false,
        station_name_hint_queries: Vec::new(),
    };
    let semantic_results = semantic_service
        .search(
            &semantic_query,
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .expect("semantic search must succeed");
    assert!(!semantic_results.is_empty());

    let british_semantic_results = semantic_service
        .search(
            &SearchQuery {
                country_code: Some("GB".to_owned()),
                ..semantic_query.clone()
            },
            &SearchConstraints {
                limit: 1,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .expect("hard-filtered semantic search must succeed");
    assert!(
        british_semantic_results
            .iter()
            .all(|station| station.station.country_code.as_deref() == Some("GB"))
    );

    let tie_results = semantic_service
        .search(
            &semantic_query,
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::from([
                    "rock-antenne".to_owned(),
                    "radio-record-rock".to_owned(),
                ]),
            },
        )
        .await
        .expect("semantic tie search must succeed");
    assert!(!tie_results.is_empty());

    let fallback_service = SearchService::with_providers(
        Arc::new(repository.clone()),
        Arc::new(DeterministicQueryParser),
        Some(Arc::new(FailingEmbeddingProvider)),
    );
    let fallback_results = fallback_service
        .search(
            &rock_query,
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .expect("provider failure must retain metadata search");
    assert!(!station_ids(&fallback_results).is_empty());

    let embedding_pool = repository_pool(&database_url).await;
    let persisted_count = sqlx::query_scalar::<_, i64>(
        r#"
SELECT COUNT(*)
FROM station_embeddings
WHERE station_id = 'rock-antenne'
  AND model = 'integration-model'
  AND version = '1'
"#,
    )
    .fetch_one(&embedding_pool)
    .await
    .expect("embedding count must be inspectable");
    let persisted = sqlx::query_as::<_, (i32, f64, bool)>(
        r#"
SELECT dimension, embedding <=> '[1,0,0]'::vector, updated_at >= created_at
FROM station_embeddings
WHERE station_id = 'rock-antenne'
  AND model = 'integration-model'
  AND version = '1'
"#,
    )
    .fetch_one(&embedding_pool)
    .await
    .expect("embedding persistence must be inspectable");
    assert_eq!(persisted_count, 1);
    assert_eq!(persisted.0, 3);
    assert!(persisted.1.abs() < 0.000_001);
    assert!(persisted.2);
    embedding_pool.close().await;
    embedding_store.close().await;

    let import_store = PostgresImportStore::connect(&database_url)
        .await
        .expect("import store migrations must succeed");
    let source_station_id = "01234567-89ab-cdef-0123-456789abcdef";
    let first_import = CatalogImporter::new(
        OnePageProvider::station(imported_station(
            source_station_id,
            "Integration Rock Radio",
            "https://streams.example.com/imported-first.mp3",
            &["rock"],
        )),
        import_store.clone(),
        ImportLimits {
            page_size: 10,
            max_pages: 2,
        },
    )
    .run()
    .await
    .expect("first import must succeed");
    let repeat_import = CatalogImporter::new(
        OnePageProvider::station(imported_station(
            source_station_id,
            "Integration Rock Radio Updated",
            "https://streams.example.com/imported-updated.mp3",
            &["rock", "upbeat"],
        )),
        import_store.clone(),
        ImportLimits {
            page_size: 10,
            max_pages: 2,
        },
    )
    .run()
    .await
    .expect("repeat import must update in place");
    let failed_import = CatalogImporter::new(
        OnePageProvider::failure(),
        import_store.clone(),
        ImportLimits {
            page_size: 10,
            max_pages: 2,
        },
    )
    .run()
    .await
    .expect_err("provider failure must fail the run");
    assert_eq!(failed_import.safe_summary(), "mock provider unavailable");

    let inspection_pool = PgPool::connect(&database_url)
        .await
        .expect("inspection connection must succeed");
    let shared_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM stations WHERE source = 'rockcatalog'")
            .fetch_one(&inspection_pool)
            .await
            .unwrap();
    assert_eq!(
        shared_count, 41,
        "shared release must remain active beside imports"
    );
    let provider_station_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM stations WHERE source = $1 AND source_station_id = $2",
    )
    .bind(SOURCE)
    .bind(source_station_id)
    .fetch_one(&inspection_pool)
    .await
    .unwrap();
    assert_eq!(
        provider_station_count, 1,
        "repeat import must not duplicate"
    );
    let provider_stream = sqlx::query_as::<_, (String, String)>(
        r#"
SELECT s.name, ss.stream_url
FROM stations AS s
JOIN station_streams AS ss ON ss.station_id = s.id
WHERE s.source = $1 AND s.source_station_id = $2
"#,
    )
    .bind(SOURCE)
    .bind(source_station_id)
    .fetch_one(&inspection_pool)
    .await
    .unwrap();
    assert_eq!(provider_stream.0, "Integration Rock Radio Updated");
    assert_eq!(
        provider_stream.1,
        "https://streams.example.com/imported-updated.mp3"
    );

    assert_run(
        &inspection_pool,
        &first_import.run_id,
        "completed",
        (1, 1, 0, 0),
        false,
    )
    .await;
    assert_run(
        &inspection_pool,
        &repeat_import.run_id,
        "completed",
        (1, 1, 0, 0),
        false,
    )
    .await;
    let failed_run_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM import_runs WHERE source = $1 AND status = 'failed' ORDER BY started_at DESC LIMIT 1",
    )
    .bind(SOURCE)
    .fetch_one(&inspection_pool)
    .await
    .unwrap()
    .to_string();
    assert_run(
        &inspection_pool,
        &failed_run_id,
        "failed",
        (0, 0, 0, 0),
        true,
    )
    .await;

    let imported_query = normalize_query("upbeat".to_owned(), "en-US".to_owned());
    let imported_results = service
        .search(
            &imported_query,
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .expect("search over imported metadata must succeed");
    assert!(
        imported_results
            .iter()
            .any(|ranked| ranked.station.id == format!("rb-{source_station_id}")),
        "imported station must be searchable"
    );
    inspection_pool.close().await;
    import_store.close().await;

    let app = router_with_repository(Arc::new(repository.clone()));
    let ready = app
        .clone()
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), axum::http::StatusCode::OK);

    let provider_independent_ready = router_with_search_service(fallback_service)
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        provider_independent_ready.status(),
        axum::http::StatusCode::OK
    );

    repository.close().await;

    let unavailable = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        unavailable.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let body = unavailable.into_body().collect().await.unwrap().to_bytes();
    let payload: HealthResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload.status, HealthStatus::NotReady);

    let response = router_with_repository(Arc::new(
        PostgresStationRepository::connect(&database_url)
            .await
            .expect("repository should reconnect"),
    ))
    .oneshot(
        Request::post("/v1/search")
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!("Bearer {}", rockserver::http::TEST_API_BEARER_TOKEN),
            )
            .body(Body::from(r#"{"query":"rock"}"#))
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(
        payload["stations"][0]["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
}

/// Exercises canonical retirement metadata through transactional activation, search, rollback, and
/// provider coexistence against a disposable database.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn shared_catalog_tombstones_are_active_idempotent_and_rollback_safe() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("set TEST_DATABASE_URL to an isolated PostgreSQL database");
    let repository = PostgresStationRepository::connect(&database_url)
        .await
        .expect("baseline activation must succeed");
    let store = PostgresImportStore::connect(&database_url)
        .await
        .expect("catalog store must connect");
    let before = lifecycle_catalog("test-before", false);
    let after = lifecycle_catalog("test-after", true);

    store
        .activate_shared_catalog(&before)
        .await
        .expect("initial lifecycle release must activate");
    let radio_browser_import = CatalogImporter::new(
        OnePageProvider::station(imported_station(
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "Provider ownership sentinel",
            "https://streams.example.com/ownership-sentinel.mp3",
            &["rock"],
        )),
        store.clone(),
        ImportLimits {
            page_size: 10,
            max_pages: 1,
        },
    );
    radio_browser_import
        .run()
        .await
        .expect("Radio Browser sentinel must import");

    store
        .activate_shared_catalog(&after)
        .await
        .expect("retirement release must activate");
    store
        .activate_shared_catalog(&after)
        .await
        .expect("re-import of the same release must be idempotent");

    assert_eq!(
        store
            .lookup_shared_catalog_replacement("merge-old")
            .await
            .unwrap(),
        OwnedCatalogReplacement::Redirect("merge-target".to_owned())
    );
    assert_eq!(
        store
            .lookup_shared_catalog_replacement("split-old")
            .await
            .unwrap(),
        OwnedCatalogReplacement::Ambiguous(vec!["split-one".to_owned(), "split-two".to_owned()])
    );
    assert_eq!(
        store
            .lookup_shared_catalog_replacement("removed-old")
            .await
            .unwrap(),
        OwnedCatalogReplacement::Removed
    );

    let service = SearchService::new(Arc::new(repository.clone()));
    let retired_results = service
        .search(
            &normalize_query("legacy retired marker".to_owned(), "en-US".to_owned()),
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .expect("retired rows must be filtered rather than breaking search");
    assert!(retired_results.iter().all(|station| !matches!(
        station.station.id.as_str(),
        "removed-old" | "merge-old" | "split-old"
    )));

    let pool = repository_pool(&database_url).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM catalog_tombstones WHERE source = 'rockcatalog'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM stations WHERE source = 'radio_browser' AND source_station_id = 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1,
        "RockCatalog activation must not change Radio Browser ownership"
    );

    store
        .activate_shared_catalog(&before)
        .await
        .expect("previous release rollback must reactivate its rows");
    assert_eq!(
        store
            .lookup_shared_catalog_replacement("merge-old")
            .await
            .unwrap(),
        OwnedCatalogReplacement::Unknown
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM stations WHERE source = 'rockcatalog' AND source_station_id = 'merge-old' AND retired_at IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    pool.close().await;
    store.close().await;
    repository.close().await;
}

/// Builds two immutable fixtures that model a prior release and a replacement release.
fn lifecycle_catalog(version: &str, with_tombstones: bool) -> PinnedSharedCatalog {
    let retired = if with_tombstones {
        ""
    } else {
        r#",
    {"id":"removed-old","name":"Legacy retired marker","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{"id":"main","url":"https://example.com/removed-old","codec":"mp3","bitrateKbps":128,"primary":true}]},
    {"id":"merge-old","name":"Legacy merge marker","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{"id":"main","url":"https://example.com/merge-old","codec":"mp3","bitrateKbps":128,"primary":true}]},
    {"id":"split-old","name":"Legacy split marker","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{"id":"main","url":"https://example.com/split-old","codec":"mp3","bitrateKbps":128,"primary":true}]}"#
    };
    let tombstones = if with_tombstones {
        r#"[{"id":"removed-old","reason":"removed","replacementIds":[]},{"id":"merge-old","reason":"merged","replacementIds":["merge-target"]},{"id":"split-old","reason":"split","replacementIds":["split-one","split-two"]}]"#
    } else {
        "[]"
    };
    let document = format!(
        r#"{{"schemaVersion":1,"catalogVersion":"{version}","stations":[
    {{"id":"merge-target","name":"Merge target","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{{"id":"main","url":"https://example.com/merge-target","codec":"mp3","bitrateKbps":128,"primary":true}}]}},
    {{"id":"split-one","name":"Split one","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{{"id":"main","url":"https://example.com/split-one","codec":"mp3","bitrateKbps":128,"primary":true}}]}},
    {{"id":"split-two","name":"Split two","aliases":[],"legacyIds":[],"tags":["rock"],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{{"id":"main","url":"https://example.com/split-two","codec":"mp3","bitrateKbps":128,"primary":true}}]}}{retired}],"tombstones":{tombstones}}}"#,
    );
    let digest = format!("{:x}", sha2::Sha256::digest(document.as_bytes()));
    PinnedSharedCatalog::from_bytes(document.as_bytes(), version, &digest).unwrap()
}

fn station_ids(results: &[rockserver::search::RankedStation]) -> Vec<&str> {
    results
        .iter()
        .map(|ranked| ranked.station.id.as_str())
        .collect()
}

async fn repository_pool(database_url: &str) -> PgPool {
    PgPool::connect(database_url)
        .await
        .expect("inspection connection must succeed")
}

struct FixedEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for FixedEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
        Ok(Embedding::new("integration-model", "1", 3, vec![1.0, 0.0, 0.0]).unwrap())
    }
}

struct FailingEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for FailingEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Embedding, EmbeddingProviderError> {
        Err(EmbeddingProviderError::safe(
            "scripted integration provider failure",
        ))
    }
}

struct OnePageProvider {
    station: Option<ImportedStation>,
    fail: bool,
}

impl OnePageProvider {
    fn station(station: ImportedStation) -> Self {
        Self {
            station: Some(station),
            fail: false,
        }
    }

    fn failure() -> Self {
        Self {
            station: None,
            fail: true,
        }
    }
}

#[async_trait]
impl CatalogImportProvider for OnePageProvider {
    fn source(&self) -> &'static str {
        SOURCE
    }

    async fn fetch_page(
        &self,
        _offset: usize,
        _limit: usize,
    ) -> Result<ImportPage, CatalogImportError> {
        if self.fail {
            return Err(CatalogImportError::safe("mock provider unavailable"));
        }
        Ok(ImportPage {
            fetched: usize::from(self.station.is_some()),
            stations: self.station.clone().into_iter().collect(),
            skipped: 0,
        })
    }
}

fn imported_station(
    source_station_id: &str,
    name: &str,
    stream_url: &str,
    tags: &[&str],
) -> ImportedStation {
    ImportedStation {
        source: SOURCE,
        source_station_id: source_station_id.to_owned(),
        id: format!("rb-{source_station_id}"),
        name: name.to_owned(),
        homepage_url: Some("https://example.com/imported-radio".to_owned()),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        language: Some("en".to_owned()),
        country_code: Some("US".to_owned()),
        streams: vec![ImportedStream {
            source_stream_id: source_station_id.to_owned(),
            stream_url: stream_url.to_owned(),
            codec: Some("MP3".to_owned()),
            bitrate_kbps: Some(192),
            is_primary: true,
        }],
    }
}

async fn assert_run(
    pool: &PgPool,
    run_id: &str,
    expected_status: &str,
    expected_counts: (i64, i64, i64, i64),
    expects_error: bool,
) {
    let run_id = Uuid::parse_str(run_id).unwrap();
    let row = sqlx::query_as::<_, (String, i64, i64, i64, i64, Option<String>, bool)>(
        r#"
SELECT status, fetched_count, imported_count, skipped_count, failed_count, error_summary,
       completed_at IS NOT NULL
FROM import_runs
WHERE id = $1
"#,
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(row.0, expected_status);
    assert_eq!((row.1, row.2, row.3, row.4), expected_counts);
    assert_eq!(row.5.is_some(), expects_error);
    assert!(row.6, "terminal run must have completed_at");
}
