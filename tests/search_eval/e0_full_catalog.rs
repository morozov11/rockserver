//! E0 full-catalog evaluation against the pinned complete station release.
//!
//! Roadmap `ai-provider-architecture` stages E0/E2: reference query sets for
//! ru/en with relevance marked by `station_id`, evaluated through the real
//! PostgreSQL search path on the checksum-pinned 16 825-station snapshot.
//! Reports Top-1/3/5, Recall@5/10, MRR, empty answers and latency per language
//! and category, and asserts a second pass returns identical ordering
//! (determinism). Semantic modes fill `station_embeddings_v1`: locally with
//! ONNX (`ROCKSERVER_SEARCH_EVAL_WITH_ONNX=1`) or through the gRPC embedder
//! (`ROCKSERVER_SEARCH_EVAL_WITH_EMBEDDER=1`); both must reproduce the E0
//! semantic baseline (the service is bit-identical to `onnx-v1`) and also
//! report ANN recall versus exact kNN plus an EXPLAIN proof of the HNSW scan.
//! The regular test validates labels against the pinned release without a
//! database; the ignored evaluation additionally requires an explicitly
//! disposable PostgreSQL.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    sync::Arc,
    time::Instant,
};

use rockserver::{
    catalog::{CatalogImportProvider, CatalogImporter, FullReleaseCatalogAdapter, ImportLimits},
    persistence::{PostgresImportStore, PostgresStationRepository},
    search::{
        DeterministicQueryParser, EmbeddingProvider, QueryParserInput, SearchConstraints,
        SearchService, normalize_query,
    },
};
use serde::Deserialize;
use sqlx::PgPool;

use super::e0_semantic::{self, Metrics};

const FIXTURE: &str = include_str!("../fixtures/search_eval_e0_full_catalog.json");
const DISPOSABLE_ENV: &str = "ROCKSERVER_SEARCH_EVAL_DISPOSABLE";
const ONNX_ENV: &str = "ROCKSERVER_SEARCH_EVAL_WITH_ONNX";
const EMBEDDER_ENV: &str = "ROCKSERVER_SEARCH_EVAL_WITH_EMBEDDER";
const CATALOG_SHA256: &str = "ad469d405f177d7e476cf9b3d9985497d0e2c6132ac0f3ce14485f4eab402073";
/// The `rockcast/src/voice/rank.rs` regression cases must stay in the ru set (RC2 gate).
const MANDATORY_RU_SPOKEN: &[(&str, &str)] = &[
    (
        "включи радио рок",
        "rb-1380f2d6-2312-49c0-a618-f44ff83c3f75",
    ),
    (
        "включи радио рокс",
        "rb-1380f2d6-2312-49c0-a618-f44ff83c3f75",
    ),
];
const CATEGORIES: &[&str] = &[
    "exact",
    "genre",
    "mood",
    "synonym",
    "typo",
    "spoken",
    "language_country",
    "semantic_probe",
];

#[derive(Deserialize)]
pub(super) struct E0Fixture {
    pub(super) catalog_version: String,
    pub(super) catalog_sha256: String,
    pub(super) sets: Vec<E0Set>,
}

#[derive(Deserialize)]
pub(super) struct E0Set {
    pub(super) language: String,
    pub(super) locale: String,
    pub(super) cases: Vec<E0Case>,
    pub(super) paraphrase_cases: Vec<E0Case>,
    pub(super) negative_cases: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct E0Case {
    pub(super) category: String,
    pub(super) query: String,
    pub(super) relevant: Vec<String>,
}

#[derive(Clone)]
pub(super) struct PreparedCase {
    pub(super) category: String,
    pub(super) query: String,
    pub(super) relevant: Vec<String>,
}

async fn validated_fixture() -> (E0Fixture, BTreeSet<String>) {
    let fixture: E0Fixture = serde_json::from_str(FIXTURE).expect("valid E0 fixture JSON");
    let adapter = FullReleaseCatalogAdapter::pinned()
        .await
        .expect("pinned full release must validate");
    assert_eq!(fixture.catalog_version, adapter.catalog_version());
    assert_eq!(fixture.catalog_sha256, CATALOG_SHA256);
    let station_ids = release_station_ids(&adapter).await;
    assert_eq!(station_ids.len(), 16_825);
    let mut queries = BTreeSet::new();
    let mut ru_spoken = BTreeSet::new();
    assert_eq!(fixture.sets.len(), 2, "exactly ru and en sets");
    for set in &fixture.sets {
        assert!(set.cases.len() >= 30 && set.cases.len() <= 50);
        assert!(!set.locale.is_empty());
        for case in set.cases.iter().chain(&set.paraphrase_cases) {
            assert!(!case.query.trim().is_empty());
            assert!(queries.insert(case.query.as_str()), "duplicate query");
            assert!(
                CATEGORIES.contains(&case.category.as_str()),
                "unknown category {}",
                case.category
            );
            assert!(!case.relevant.is_empty(), "positive case needs a label");
            for id in &case.relevant {
                assert!(station_ids.contains(id), "unknown relevant station: {id}");
            }
        }
        for query in &set.negative_cases {
            assert!(!query.trim().is_empty());
            assert!(queries.insert(query.as_str()), "duplicate negative query");
        }
        if set.language == "ru" {
            for case in &set.cases {
                if case.category == "spoken" {
                    ru_spoken.insert(case.query.clone());
                }
            }
        }
    }
    for (query, station) in MANDATORY_RU_SPOKEN {
        let case = fixture
            .sets
            .iter()
            .find(|set| set.language == "ru")
            .expect("ru set")
            .cases
            .iter()
            .find(|case| case.query == *query)
            .unwrap_or_else(|| panic!("mandatory rank.rs case missing: {query}"));
        assert_eq!(case.relevant, [(*station).to_owned()]);
        assert!(ru_spoken.contains(*query));
    }
    (fixture, station_ids)
}

#[tokio::test]
async fn e0_fixture_references_the_pinned_full_release() {
    validated_fixture().await;
}

/// Pages the in-memory pinned release through the public import boundary to
/// collect the exhaustive `station_id` set used for label validation.
async fn release_station_ids(adapter: &FullReleaseCatalogAdapter) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut offset = 0usize;
    loop {
        let page = adapter.fetch_page(offset, 500).await.expect("release page");
        if page.stations.is_empty() {
            break;
        }
        for station in &page.stations {
            ids.insert(station.id.clone());
        }
        offset += page.stations.len();
    }
    ids
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for a disposable PostgreSQL database and explicit destructive opt-in"]
async fn e0_evaluates_full_catalog_through_postgres_search() {
    assert_eq!(
        env::var(DISPOSABLE_ENV).ok().as_deref(),
        Some("1"),
        "set {DISPOSABLE_ENV}=1 only for a disposable database: this test truncates stations"
    );
    let database_url = env::var("TEST_DATABASE_URL").expect("set TEST_DATABASE_URL");
    let (fixture, _station_ids) = validated_fixture().await;
    let with_onnx = env::var(ONNX_ENV).ok().as_deref() == Some("1");
    let with_embedder = env::var(EMBEDDER_ENV).ok().as_deref() == Some("1");
    assert!(
        !with_onnx || !with_embedder,
        "enable at most one of {ONNX_ENV} and {EMBEDDER_ENV}"
    );
    #[cfg(not(feature = "onnx-local"))]
    assert!(
        !with_onnx,
        "enable the onnx-local Cargo feature for {ONNX_ENV}=1"
    );

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
    let adapter = FullReleaseCatalogAdapter::pinned()
        .await
        .expect("pinned full release");
    let import = CatalogImporter::new(
        adapter,
        import_store,
        ImportLimits {
            page_size: 500,
            max_pages: 40,
        },
    )
    .run()
    .await
    .expect("full catalog import");
    assert_eq!(import.counts.imported, 16_825);
    assert_eq!(import.counts.failed, 0);
    assert_eq!(import.counts.skipped, 0);
    let active_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM stations WHERE retired_at IS NULL")
            .fetch_one(&pool)
            .await
            .expect("count active stations");
    assert_eq!(active_count, 16_825);

    // Provider selection: embedder gRPC client, local ONNX (bit-identical
    // vectors), or metadata-only. Semantic modes fill station_embeddings_v1.
    let mut provenance = "none (metadata-only)".to_owned();
    let mut embedding_provider: Option<Arc<dyn EmbeddingProvider>> = None;
    if with_embedder {
        let (provider, label) = e0_semantic::e0_embedder_fill(&database_url).await;
        embedding_provider = Some(provider);
        provenance = label;
    } else if with_onnx {
        #[cfg(feature = "onnx-local")]
        {
            let (provider, label) = e0_semantic::e0_onnx_fill(&database_url).await;
            embedding_provider = Some(provider);
            provenance = label;
        }
    }
    let semantic_enabled = embedding_provider.is_some();
    if semantic_enabled {
        for table in ["stations", "station_embeddings_v1"] {
            sqlx::query(&format!("ANALYZE {table}"))
                .execute(&pool)
                .await
                .expect("planner statistics");
        }
    }
    let service = SearchService::with_providers(
        Arc::new(repository.clone()),
        Arc::new(DeterministicQueryParser),
        embedding_provider.clone(),
    );

    println!(
        "e0 catalog={} sha256={} stations=16825 parser=deterministic embeddings={provenance}",
        fixture.catalog_version, fixture.catalog_sha256
    );
    let mut language_summaries = Vec::new();
    for set in &fixture.sets {
        let prepared = set
            .cases
            .iter()
            .map(|case| PreparedCase {
                category: case.category.clone(),
                query: case.query.clone(),
                relevant: case.relevant.clone(),
            })
            .collect::<Vec<_>>();
        let constraints = SearchConstraints {
            limit: 10,
            offset: 0,
            excluded_station_ids: BTreeSet::new(),
        };
        let first = run_eval_pass(&service, &set.locale, &prepared).await;
        let second = run_eval_pass(&service, &set.locale, &prepared).await;
        for (case, (first_ids, _)) in prepared.iter().zip(&first) {
            println!(
                "ranked lang={} category={} query={:?} ids={:?}",
                set.language, case.category, case.query, first_ids
            );
            if !first_ids
                .iter()
                .take(10)
                .any(|id| case.relevant.contains(id))
            {
                println!(
                    "miss lang={} query={:?} expected={:?} top={:?}",
                    set.language,
                    case.query,
                    case.relevant,
                    first_ids.first()
                );
            }
        }
        let mut nondeterministic = Vec::new();
        for (index, ((first_ids, _), (second_ids, _))) in first.iter().zip(&second).enumerate() {
            if first_ids != second_ids {
                nondeterministic.push(prepared[index].query.clone());
            }
        }
        // The ANN candidate order is tie-broken by station ID (E2), so both
        // the metadata and semantic passes must be order-stable now.
        println!(
            "determinism lang={} stable={} unstable_queries={nondeterministic:?}",
            set.language,
            prepared.len() - nondeterministic.len()
        );
        assert!(
            nondeterministic.is_empty(),
            "non-deterministic ordering for {nondeterministic:?}"
        );
        let mut overall = Metrics::default();
        let mut by_category: BTreeMap<String, Metrics> = BTreeMap::new();
        for (case, (ids, elapsed_ms)) in prepared.iter().zip(&first) {
            overall.record(&case.relevant, ids, *elapsed_ms);
            by_category
                .entry(case.category.clone())
                .or_default()
                .record(&case.relevant, ids, *elapsed_ms);
        }
        if semantic_enabled {
            e0_semantic::assert_against_semantic_baseline(&set.language, &overall);
        }
        let mut paraphrase = Metrics::default();
        for case in &set.paraphrase_cases {
            let started = Instant::now();
            let outcome = service
                .interpret_and_search_private(
                    QueryParserInput {
                        query: case.query.clone(),
                        locale: set.locale.clone(),
                    },
                    &constraints,
                )
                .await
                .expect("paraphrase search must succeed");
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let ids = outcome
                .stations
                .iter()
                .map(|row| row.station.id.clone())
                .collect::<Vec<_>>();
            println!(
                "paraphrase lang={} query={:?} expected={:?} ids={:?}",
                set.language, case.query, case.relevant, ids
            );
            paraphrase.record(&case.relevant, &ids, elapsed_ms);
        }
        let mut negative_nonempty = 0;
        for text in &set.negative_cases {
            let query = normalize_query(text.to_owned(), set.locale.clone());
            let visible = service
                .search(
                    &query,
                    &SearchConstraints {
                        limit: 10,
                        offset: 0,
                        excluded_station_ids: BTreeSet::new(),
                    },
                )
                .await
                .unwrap();
            negative_nonempty += usize::from(!visible.is_empty());
        }
        language_summaries.push(format!(
            "lang={} overall {}",
            set.language,
            overall.summary()
        ));
        for (category, mut metrics) in by_category {
            language_summaries.push(format!(
                "lang={} {category} {}",
                set.language,
                metrics.summary()
            ));
        }
        language_summaries.push(format!(
            "lang={} paraphrase {}",
            set.language,
            paraphrase.summary()
        ));
        language_summaries.push(format!(
            "lang={} negative n={} nonempty={negative_nonempty}",
            set.language,
            set.negative_cases.len()
        ));
    }
    for line in language_summaries {
        println!("{line}");
    }
    if let Some(provider) = &embedding_provider {
        e0_semantic::ann_recall_and_explain(&pool, provider, &fixture).await;
    }
    pool.close().await;
    repository.close().await;
}

/// Runs one pass over the prepared cases; returns the ordered station IDs and latency per case.
async fn run_eval_pass(
    service: &SearchService,
    locale: &str,
    cases: &[PreparedCase],
) -> Vec<(Vec<String>, f64)> {
    let constraints = SearchConstraints {
        limit: 10,
        offset: 0,
        excluded_station_ids: BTreeSet::new(),
    };
    let mut outcomes = Vec::with_capacity(cases.len());
    for case in cases {
        let started = Instant::now();
        let outcome = service
            .interpret_and_search_private(
                QueryParserInput {
                    query: case.query.clone(),
                    locale: locale.to_owned(),
                },
                &constraints,
            )
            .await
            .expect("PostgreSQL search must succeed");
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let ids = outcome
            .stations
            .iter()
            .map(|row| row.station.id.clone())
            .collect::<Vec<_>>();
        outcomes.push((ids, elapsed_ms));
    }
    outcomes
}
