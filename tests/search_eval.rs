//! Opt-in quality evaluation against the real PostgreSQL search path and pinned RockCatalog.
//!
//! The regular test validates the labels without a database. The ignored evaluation requires
//! an explicitly disposable PostgreSQL database; optional ONNX mode generates every vector
//! from the same local model assets before running the queries.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    sync::Arc,
    time::Instant,
};

use rockserver::{
    catalog::PinnedSharedCatalog,
    persistence::{PostgresImportStore, PostgresStationRepository},
    search::{
        DeterministicQueryParser, EmbeddingProvider, QueryParserInput, SearchConstraints,
        SearchService, StationRepository, normalize_query,
    },
};
use serde::Deserialize;
use sqlx::PgPool;

#[cfg(feature = "onnx-local")]
use rockserver::providers::onnx_e5::{
    DIMENSION, MODEL, OnnxE5Config, OnnxE5EmbeddingProvider, VERSION,
};
#[cfg(feature = "onnx-local")]
use sha2::{Digest, Sha256};
#[cfg(feature = "onnx-local")]
use sqlx::Row as _;

const FIXTURE: &str = include_str!("fixtures/search_eval_golden_set.json");
const DISPOSABLE_ENV: &str = "ROCKSERVER_SEARCH_EVAL_DISPOSABLE";
const ONNX_ENV: &str = "ROCKSERVER_SEARCH_EVAL_WITH_ONNX";
#[cfg(feature = "onnx-local")]
const FORMAT_ENV: &str = "ROCKSERVER_SEARCH_EVAL_FORMAT_2X2";

#[cfg(feature = "onnx-local")]
#[path = "search_eval/format_experiment.rs"]
mod format_experiment;

#[path = "search_eval/e0_full_catalog.rs"]
mod e0_full_catalog;

#[path = "search_eval/e0_semantic.rs"]
mod e0_semantic;

#[derive(Deserialize)]
struct GoldenSet {
    catalog_version: String,
    catalog_sha256: String,
    negative_cases: Vec<String>,
    paraphrase_cases: Vec<GoldenCase>,
    cases: Vec<GoldenCase>,
}

#[derive(Deserialize)]
struct GoldenCase {
    category: String,
    query: String,
    relevant: Vec<String>,
}

#[derive(Default)]
struct Metrics {
    cases: usize,
    recall_5_sum: f64,
    recall_10_sum: f64,
    reciprocal_rank_sum: f64,
    empty_results: usize,
    latencies_ms: Vec<f64>,
}

impl Metrics {
    fn record(&mut self, relevant: &[String], result_ids: &[String], elapsed_ms: f64) {
        self.cases += 1;
        let relevant = relevant.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let recall = |limit: usize| {
            result_ids
                .iter()
                .take(limit)
                .filter(|id| relevant.contains(id.as_str()))
                .count() as f64
                / relevant.len() as f64
        };
        self.recall_5_sum += recall(5);
        self.recall_10_sum += recall(10);
        self.reciprocal_rank_sum += result_ids
            .iter()
            .position(|id| relevant.contains(id.as_str()))
            .map_or(0.0, |rank| 1.0 / (rank + 1) as f64);
        self.empty_results += usize::from(result_ids.is_empty());
        self.latencies_ms.push(elapsed_ms);
    }

    fn summary(&mut self) -> String {
        self.latencies_ms.sort_by(f64::total_cmp);
        let percentile = |fraction: f64| {
            let index = ((self.latencies_ms.len() - 1) as f64 * fraction).ceil() as usize;
            self.latencies_ms[index]
        };
        format!(
            "n={} recall@5={:.3} recall@10={:.3} mrr={:.3} empty={} latency_p50_ms={:.1} latency_p95_ms={:.1}",
            self.cases,
            self.recall_5_sum / self.cases as f64,
            self.recall_10_sum / self.cases as f64,
            self.reciprocal_rank_sum / self.cases as f64,
            self.empty_results,
            percentile(0.50),
            percentile(0.95),
        )
    }
}

fn validated_fixture() -> GoldenSet {
    let fixture: GoldenSet = serde_json::from_str(FIXTURE).expect("valid golden-set JSON");
    let catalog = PinnedSharedCatalog::load().expect("pinned catalog checksum must match");
    assert_eq!(fixture.catalog_version, catalog.catalog_version());
    assert_eq!(
        fixture.catalog_sha256,
        rockserver::catalog::shared::PINNED_CATALOG_SHA256
    );
    assert_eq!(
        fixture.cases.len(),
        50,
        "the evaluation set must remain reviewable"
    );
    let station_ids = catalog
        .stations()
        .iter()
        .map(|station| station.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut queries = BTreeSet::new();
    assert_eq!(fixture.paraphrase_cases.len(), 5);
    for case in fixture.cases.iter().chain(&fixture.paraphrase_cases) {
        assert!(!case.query.trim().is_empty());
        assert!(queries.insert(case.query.as_str()), "duplicate query");
        assert!(
            ["exact", "genre", "spoken", "semantic_probe"].contains(&case.category.as_str()),
            "unknown category"
        );
        assert!(!case.relevant.is_empty(), "positive case needs a label");
        for id in &case.relevant {
            assert!(
                station_ids.contains(id.as_str()),
                "unknown relevant station: {id}"
            );
        }
    }
    assert_eq!(fixture.negative_cases.len(), 20);
    for query in &fixture.negative_cases {
        assert!(!query.trim().is_empty());
        assert!(queries.insert(query), "duplicate negative query");
    }
    fixture
}

#[test]
fn golden_queries_reference_the_pinned_catalog() {
    validated_fixture();
}

#[test]
fn german_lyrics_probe_does_not_require_known_broadcast_language_or_country() {
    let fixture = validated_fixture();
    let case = fixture
        .cases
        .iter()
        .find(|case| case.query == "deutschsprachige Rockmusik")
        .expect("German-lyrics probe must be present");
    assert_eq!(case.relevant, ["rock-antenne-deutschrock"]);
    let query = normalize_query(case.query.clone(), "en-US".to_owned());
    assert_eq!(query.language, None);
    assert_eq!(query.country_code, None);
}

#[test]
fn recall_and_reciprocal_rank_use_all_relevant_stations() {
    let mut metrics = Metrics::default();
    metrics.record(
        &["a".into(), "b".into()],
        &["x".into(), "b".into(), "a".into()],
        2.0,
    );
    assert_eq!(metrics.recall_5_sum, 1.0);
    assert_eq!(metrics.reciprocal_rank_sum, 0.5);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for a disposable PostgreSQL database and explicit destructive opt-in"]
async fn evaluates_pinned_catalog_through_postgres_search() {
    assert_eq!(
        env::var(DISPOSABLE_ENV).ok().as_deref(),
        Some("1"),
        "set {DISPOSABLE_ENV}=1 only for a disposable database: this test truncates stations"
    );
    let database_url = env::var("TEST_DATABASE_URL").expect("set TEST_DATABASE_URL");
    let fixture = validated_fixture();
    let catalog = PinnedSharedCatalog::load().unwrap();
    let with_onnx = env::var(ONNX_ENV).ok().as_deref() == Some("1");
    #[cfg(not(feature = "onnx-local"))]
    assert!(
        !with_onnx,
        "enable the onnx-local Cargo feature for {ONNX_ENV}=1"
    );
    #[cfg(feature = "onnx-local")]
    let (prepared_provider, provenance) = if with_onnx {
        let config = OnnxE5Config::from_env().expect("local ONNX assets configured");
        let provenance = format!(
            "{MODEL}/{VERSION}/{DIMENSION} model_sha256={} tokenizer_sha256={}",
            sha256_file(&config.model_path),
            sha256_file(&config.tokenizer_path)
        );
        let provider = Arc::new(OnnxE5EmbeddingProvider::load(&config).expect("local E5 model"));
        (Some(provider), provenance)
    } else {
        (None, String::from("none (metadata-only)"))
    };
    #[cfg(not(feature = "onnx-local"))]
    let provenance = String::from("none (metadata-only)");

    let repository = PostgresStationRepository::connect(&database_url)
        .await
        .expect("PostgreSQL migrations must succeed");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("inspection pool");
    sqlx::query("TRUNCATE TABLE stations CASCADE")
        .execute(&pool)
        .await
        .expect("reset disposable catalog");
    let import_store = PostgresImportStore::connect(&database_url)
        .await
        .expect("import store");
    import_store
        .activate_shared_catalog(&catalog)
        .await
        .expect("import pinned catalog");
    let active_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM stations WHERE retired_at IS NULL")
            .fetch_one(&pool)
            .await
            .expect("count active stations");
    assert_eq!(active_count as usize, catalog.stations().len());

    let embedding_provider: Option<Arc<dyn EmbeddingProvider>> = {
        #[cfg(feature = "onnx-local")]
        {
            if let Some(provider) = prepared_provider {
                fill_versioned_table_with_onnx(&database_url, &provider).await;
                Some(provider as Arc<dyn EmbeddingProvider>)
            } else {
                None
            }
        }
        #[cfg(not(feature = "onnx-local"))]
        {
            let _ = &catalog;
            None
        }
    };
    let diagnose_ann = env::var("ROCKSERVER_SEARCH_EVAL_DIAGNOSE_ANN")
        .ok()
        .as_deref()
        == Some("1");
    assert!(!diagnose_ann || embedding_provider.is_some());
    let diagnostic_provider = embedding_provider.clone();
    let service = SearchService::with_providers(
        Arc::new(repository.clone()),
        Arc::new(DeterministicQueryParser),
        embedding_provider,
    );
    let mut overall = Metrics::default();
    let mut by_category: BTreeMap<String, Metrics> = BTreeMap::new();
    println!(
        "catalog={} sha256={} stations={} parser=deterministic embeddings={}",
        fixture.catalog_version, fixture.catalog_sha256, active_count, provenance
    );
    for case in &fixture.cases {
        let started = Instant::now();
        let outcome = service
            .interpret_and_search_private(
                QueryParserInput {
                    query: case.query.clone(),
                    locale: "en-US".to_owned(),
                },
                &SearchConstraints {
                    limit: 10,
                    offset: 0,
                    excluded_station_ids: BTreeSet::new(),
                },
            )
            .await
            .expect("PostgreSQL search must succeed");
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let result_ids = outcome
            .stations
            .iter()
            .map(|station| station.station.id.clone())
            .collect::<Vec<_>>();
        if diagnose_ann {
            let embedding = diagnostic_provider
                .as_ref()
                .unwrap()
                .embed(&case.query)
                .await
                .unwrap();
            let raw = repository
                .search(
                    &outcome.query,
                    &SearchConstraints {
                        limit: 100,
                        offset: 0,
                        excluded_station_ids: BTreeSet::new(),
                    },
                    Some(&embedding),
                )
                .await
                .unwrap();
            println!(
                "ann_probe query={:?} raw={:?} service={:?}",
                case.query,
                raw.iter()
                    .map(|row| (row.station.id.as_str(), row.score, row.reason.as_str()))
                    .collect::<Vec<_>>(),
                result_ids
            );
        }
        println!(
            "ranked category={} query={:?} ids={:?}",
            case.category, case.query, result_ids
        );
        if !result_ids
            .iter()
            .take(10)
            .any(|id| case.relevant.contains(id))
        {
            println!(
                "miss category={} query={:?} expected={:?} top={:?}",
                case.category,
                case.query,
                case.relevant,
                result_ids.first()
            );
        }
        overall.record(&case.relevant, &result_ids, elapsed_ms);
        by_category
            .entry(case.category.clone())
            .or_default()
            .record(&case.relevant, &result_ids, elapsed_ms);
    }
    println!("overall {}", overall.summary());
    for (category, mut metrics) in by_category {
        println!("{category} {}", metrics.summary());
    }
    let mut paraphrase_metrics = Metrics::default();
    for case in &fixture.paraphrase_cases {
        let started = Instant::now();
        let outcome = service
            .interpret_and_search_private(
                QueryParserInput {
                    query: case.query.clone(),
                    locale: "en-US".to_owned(),
                },
                &SearchConstraints {
                    limit: 10,
                    offset: 0,
                    excluded_station_ids: BTreeSet::new(),
                },
            )
            .await
            .expect("paraphrase search must succeed");
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let ids = outcome
            .stations
            .iter()
            .map(|row| row.station.id.clone())
            .collect::<Vec<_>>();
        paraphrase_metrics.record(&case.relevant, &ids, elapsed_ms);
        println!(
            "paraphrase query={:?} expected={:?} ids={:?}",
            case.query, case.relevant, ids
        );
        if diagnose_ann {
            let embedding = diagnostic_provider
                .as_ref()
                .unwrap()
                .embed(&case.query)
                .await
                .unwrap();
            let raw = repository
                .search(
                    &outcome.query,
                    &SearchConstraints {
                        limit: 100,
                        offset: 0,
                        excluded_station_ids: BTreeSet::new(),
                    },
                    Some(&embedding),
                )
                .await
                .unwrap();
            println!(
                "ann_paraphrase query={:?} raw={:?} service={:?}",
                case.query,
                raw.iter()
                    .map(|row| (row.station.id.as_str(), row.score, row.reason.as_str()))
                    .collect::<Vec<_>>(),
                ids
            );
        }
    }
    println!("paraphrase {}", paraphrase_metrics.summary());
    let mut negative_nonempty = 0;
    for text in &fixture.negative_cases {
        let query = normalize_query(text.to_owned(), "en-US".to_owned());
        let constraints = SearchConstraints {
            limit: 10,
            offset: 0,
            excluded_station_ids: BTreeSet::new(),
        };
        let visible = service.search(&query, &constraints).await.unwrap();
        negative_nonempty += usize::from(!visible.is_empty());
        if diagnose_ann {
            let embedding = diagnostic_provider
                .as_ref()
                .unwrap()
                .embed(text)
                .await
                .unwrap();
            let raw = repository
                .search(
                    &query,
                    &SearchConstraints {
                        limit: 100,
                        ..constraints
                    },
                    Some(&embedding),
                )
                .await
                .unwrap();
            println!(
                "ann_negative query={text:?} raw={:?} service={:?}",
                raw.iter()
                    .map(|row| (row.station.id.as_str(), row.score, row.reason.as_str()))
                    .collect::<Vec<_>>(),
                visible
                    .iter()
                    .map(|row| row.station.id.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
    println!(
        "negative n={} nonempty={negative_nonempty}",
        fixture.negative_cases.len()
    );
    #[cfg(feature = "onnx-local")]
    if env::var(FORMAT_ENV).ok().as_deref() == Some("1") {
        assert!(with_onnx, "{FORMAT_ENV}=1 requires {ONNX_ENV}=1");
        format_experiment::run(
            &database_url,
            &pool,
            &repository,
            diagnostic_provider.expect("loaded E5 provider"),
            &fixture,
        )
        .await;
    }
    import_store.close().await;
    pool.close().await;
    repository.close().await;
}

/// Fills `station_embeddings_v1` from the local model so the semantic pass
/// uses the same versioned storage path as production search.
#[cfg(feature = "onnx-local")]
async fn fill_versioned_table_with_onnx(database_url: &str, provider: &OnnxE5EmbeddingProvider) {
    use rockserver::persistence::{
        EMBEDDINGS_DIMENSION, EMBEDDINGS_VERSION, EmbeddingVersionStore, WrittenStationEmbedding,
        station_fields_hash,
    };

    let mut store = EmbeddingVersionStore::connect(database_url)
        .await
        .expect("versioned embedding store");
    store
        .acquire_advisory_lock(EMBEDDINGS_VERSION)
        .await
        .expect("backfill lock");
    store
        .ensure_table(EMBEDDINGS_VERSION, EMBEDDINGS_DIMENSION)
        .await
        .expect("versioned table");
    let pool = sqlx::PgPool::connect(database_url)
        .await
        .expect("station read pool");
    let rows = sqlx::query(
        "SELECT id, name, tags, language, country_code, searchable_text          FROM stations WHERE retired_at IS NULL ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("station rows");
    let mut batch = Vec::with_capacity(500);
    for row in &rows {
        let name: String = row.get("name");
        let tags: Vec<String> = row.get("tags");
        let language: Option<String> = row.get("language");
        let country_code: Option<String> = row.get("country_code");
        let text: String = row.get("searchable_text");
        let embedding = provider
            .embed_document(&text)
            .await
            .expect("station embedding");
        batch.push(WrittenStationEmbedding {
            station_id: row.get("id"),
            fields_hash: station_fields_hash(
                &name,
                &tags,
                language.as_deref(),
                country_code.as_deref(),
            ),
            values: embedding.values().to_vec(),
        });
        if batch.len() == 500 {
            store
                .write_embeddings(EMBEDDINGS_VERSION, &batch)
                .await
                .expect("persist station embeddings");
            batch.clear();
        }
    }
    if !batch.is_empty() {
        store
            .write_embeddings(EMBEDDINGS_VERSION, &batch)
            .await
            .expect("persist station embeddings");
    }
    pool.close().await;
}

#[cfg(feature = "onnx-local")]
fn sha256_file(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).expect("local model asset readable");
    format!("{:x}", Sha256::digest(bytes))
}
