//! Semantic-mode helpers of the E0 evaluator: measurement metrics, the
//! pinned E0 baselines with the regression assertion, table-fill helpers for
//! the gRPC-embedder and local-ONNX modes, and the ANN-vs-exact recall and
//! EXPLAIN evidence collection.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use rockserver::search::EmbeddingProvider;
use sqlx::PgPool;

use rockserver::persistence::{
    EMBEDDINGS_DIMENSION, EMBEDDINGS_MODEL, EMBEDDINGS_VERSION, EmbeddingVersionStore,
    MAX_BATCH_STATIONS,
};
#[cfg(feature = "onnx-local")]
use rockserver::persistence::{WrittenStationEmbedding, station_fields_hash};
#[cfg(feature = "onnx-local")]
use sqlx::Row as _;

use super::e0_full_catalog::E0Fixture;

/// Candidate budget of the ANN branch in the production search SQL.
pub(super) const ANN_CANDIDATE_LIMIT: i64 = 200;

/// E0 semantic baseline (local E5, roadmap «Результат выполнения E0»); the
/// embedding service is bit-identical, so the gRPC path must reproduce it.
struct SemanticBaseline {
    language: &'static str,
    top1: f64,
    top3: f64,
    top5: f64,
    recall5: f64,
    recall10: f64,
    mrr: f64,
    empty: usize,
}

const SEMANTIC_BASELINES: [SemanticBaseline; 2] = [
    SemanticBaseline {
        language: "en",
        top1: 0.277,
        top3: 0.404,
        top5: 0.447,
        recall5: 0.385,
        recall10: 0.449,
        mrr: 0.342,
        empty: 0,
    },
    SemanticBaseline {
        language: "ru",
        top1: 0.205,
        top3: 0.256,
        top5: 0.308,
        recall5: 0.269,
        recall10: 0.359,
        mrr: 0.250,
        empty: 0,
    },
];

#[derive(Default)]
pub(super) struct Metrics {
    cases: usize,
    top1_hits: usize,
    top3_hits: usize,
    top5_hits: usize,
    recall_5_sum: f64,
    recall_10_sum: f64,
    reciprocal_rank_sum: f64,
    empty_results: usize,
    latencies_ms: Vec<f64>,
}

impl Metrics {
    pub(super) fn record(&mut self, relevant: &[String], result_ids: &[String], elapsed_ms: f64) {
        self.cases += 1;
        let relevant = relevant.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let hit_within = |limit: usize| {
            result_ids
                .iter()
                .take(limit)
                .any(|id| relevant.contains(id.as_str()))
        };
        self.top1_hits += usize::from(hit_within(1));
        self.top3_hits += usize::from(hit_within(3));
        self.top5_hits += usize::from(hit_within(5));
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

    fn top1(&self) -> f64 {
        self.top1_hits as f64 / self.cases.max(1) as f64
    }
    fn top3(&self) -> f64 {
        self.top3_hits as f64 / self.cases.max(1) as f64
    }
    fn top5(&self) -> f64 {
        self.top5_hits as f64 / self.cases.max(1) as f64
    }
    fn recall5(&self) -> f64 {
        self.recall_5_sum / self.cases.max(1) as f64
    }
    fn recall10(&self) -> f64 {
        self.recall_10_sum / self.cases.max(1) as f64
    }
    fn mrr(&self) -> f64 {
        self.reciprocal_rank_sum / self.cases.max(1) as f64
    }

    pub(super) fn summary(&mut self) -> String {
        self.latencies_ms.sort_by(f64::total_cmp);
        let percentile = |fraction: f64| {
            let index = ((self.latencies_ms.len() - 1) as f64 * fraction).ceil() as usize;
            self.latencies_ms[index]
        };
        let n = self.cases.max(1) as f64;
        format!(
            "n={} top1={:.3} top3={:.3} top5={:.3} recall@5={:.3} recall@10={:.3} mrr={:.3} empty={} latency_p50_ms={:.1} latency_p95_ms={:.1}",
            self.cases,
            self.top1_hits as f64 / n,
            self.top3_hits as f64 / n,
            self.top5_hits as f64 / n,
            self.recall_5_sum / n,
            self.recall_10_sum / n,
            self.reciprocal_rank_sum / n,
            self.empty_results,
            percentile(0.50),
            percentile(0.95),
        )
    }
}

/// Checks a semantic run against the E0 regression criterion: Top-1/3/5 not
/// worse, Recall@5/10 and MRR not below baseline − 0.02, no new empty answers.
/// Baselines are published rounded to three decimals, so hit-rate comparisons
/// tolerate half that rounding step.
pub(super) fn assert_against_semantic_baseline(language: &str, metrics: &Metrics) {
    let baseline = SEMANTIC_BASELINES
        .iter()
        .find(|baseline| baseline.language == language)
        .unwrap_or_else(|| panic!("no baseline for {language}"));
    let tolerance = 0.02_f64;
    let rounding = 0.0005_f64;
    assert!(
        metrics.top1() >= baseline.top1 - rounding,
        "{language} top1 {} below baseline {}",
        metrics.top1(),
        baseline.top1
    );
    assert!(
        metrics.top3() >= baseline.top3 - rounding,
        "{language} top3 {} below baseline {}",
        metrics.top3(),
        baseline.top3
    );
    assert!(
        metrics.top5() >= baseline.top5 - rounding,
        "{language} top5 {} below baseline {}",
        metrics.top5(),
        baseline.top5
    );
    assert!(
        metrics.recall5() >= baseline.recall5 - tolerance,
        "{language} recall@5 {} below baseline {} - {tolerance}",
        metrics.recall5(),
        baseline.recall5
    );
    assert!(
        metrics.recall10() >= baseline.recall10 - tolerance,
        "{language} recall@10 {} below baseline {} - {tolerance}",
        metrics.recall10(),
        baseline.recall10
    );
    assert!(
        metrics.mrr() >= baseline.mrr - tolerance,
        "{language} mrr {} below baseline {} - {tolerance}",
        metrics.mrr(),
        baseline.mrr
    );
    assert!(
        metrics.empty_results <= baseline.empty,
        "{language} empty answers {} above baseline {}",
        metrics.empty_results,
        baseline.empty
    );
}

#[test]
fn top_hits_use_the_first_k_results_only() {
    let mut metrics = Metrics::default();
    metrics.record(&["a".into()], &["x".into(), "a".into()], 1.0);
    assert_eq!(metrics.top1_hits, 0);
    assert_eq!(metrics.top3_hits, 1);
    assert_eq!(metrics.recall_5_sum, 1.0);
}

/// Ensures the versioned table and fills it through the real gRPC pipeline.
pub(super) async fn e0_embedder_fill(database_url: &str) -> (Arc<dyn EmbeddingProvider>, String) {
    use rockserver::providers::embedder::{
        EmbedderClient, EmbedderConfig, EmbedderService, ServiceEmbeddingProvider, fill_version,
    };

    let config = EmbedderConfig::from_env()
        .expect("embedder configuration")
        .expect("ROCKSERVER_EMBEDDER_ADDR is required for the embedder evaluation mode");
    let client = Arc::new(EmbedderClient::lazy(config).expect("embedding service client"));
    let provenance = client.info().await.expect("embedding service info");
    assert_eq!(provenance.model, EMBEDDINGS_MODEL);
    assert_eq!(provenance.dimension, EMBEDDINGS_DIMENSION);
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
    let started = Instant::now();
    let stats = fill_version(
        client.as_ref(),
        &mut store,
        EMBEDDINGS_VERSION,
        MAX_BATCH_STATIONS,
    )
    .await
    .expect("embedder backfill");
    let (active, covered) = store.coverage(EMBEDDINGS_VERSION).await.expect("coverage");
    println!(
        "e0 embedder backfill vector_version={} active={active} covered={covered} processed={} written={} in {:.0}s",
        provenance.version,
        stats.processed,
        stats.written,
        started.elapsed().as_secs_f64()
    );
    assert_eq!(covered, active, "the disposable run must cover the catalog");
    let label = format!(
        "embedder-grpc ({} v{} dim {})",
        provenance.model, provenance.version, provenance.dimension
    );
    (
        Arc::new(ServiceEmbeddingProvider::new(client)) as Arc<dyn EmbeddingProvider>,
        label,
    )
}

/// Fills the versioned table from the local ONNX model (bit-identical to the
/// service) so both semantic modes share one storage path.
#[cfg(feature = "onnx-local")]
pub(super) async fn e0_onnx_fill(database_url: &str) -> (Arc<dyn EmbeddingProvider>, String) {
    use rockserver::providers::onnx_e5::{OnnxE5Config, OnnxE5EmbeddingProvider};

    let config = OnnxE5Config::from_env().expect("local ONNX assets configured");
    let provider = Arc::new(OnnxE5EmbeddingProvider::load(&config).expect("local E5 model"));
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
    let pool = PgPool::connect(database_url)
        .await
        .expect("station read pool");
    let rows = sqlx::query(
        "SELECT id, name, tags, language, country_code, searchable_text \
         FROM stations WHERE retired_at IS NULL ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("station rows");
    assert_eq!(rows.len(), 16_825);
    let started = Instant::now();
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
    println!(
        "e0 onnx backfill embedded 16825 stations in {:.0}s",
        started.elapsed().as_secs_f64()
    );
    pool.close().await;
    (
        provider as Arc<dyn EmbeddingProvider>,
        "onnx-e5-local".to_owned(),
    )
}

/// Measures vector-search recall and captures EXPLAIN evidence on the E0
/// snapshot: (a) HNSW kNN recall versus exact kNN (index disabled), (b) the
/// production candidate branch (fields-hash join plus tie-break) versus the
/// same exact baseline, and (c) plan proofs — the pure kNN query must scan
/// the v1 HNSW index, while the joined branch shape is printed as-is (the
/// deterministic two-key ordering forfeits the index-ordered path; at the
/// 16 825-station scale the planner prefers a bounded seq scan + sort).
pub(super) async fn ann_recall_and_explain(
    pool: &PgPool,
    provider: &Arc<dyn EmbeddingProvider>,
    fixture: &E0Fixture,
) {
    let ks = [10usize, 100usize];
    for set in &fixture.sets {
        let mut index_sums = [0.0f64; 2];
        let mut branch_sums = [0.0f64; 2];
        let mut query_count = 0usize;
        for case in &set.cases {
            let embedding = provider.embed(&case.query).await.expect("query embedding");
            let literal = vector_literal(embedding.values());
            // HNSW kNN over the versioned table (the index-ordered shape).
            let index_ann: Vec<String> = sqlx::query_scalar(
                "SELECT station_id FROM station_embeddings_v1                  ORDER BY embedding <=> $1::vector LIMIT $2",
            )
            .bind(&literal)
            .bind(ANN_CANDIDATE_LIMIT)
            .fetch_all(pool)
            .await
            .expect("HNSW kNN");
            // Production branch replica: fields-hash join, tie-break, budget.
            let branch_ann: Vec<String> = sqlx::query_scalar(
                r#"
SELECT e.station_id
FROM station_embeddings_v1 AS e
JOIN stations AS s ON s.id = e.station_id AND e.fields_hash = s.fields_hash
WHERE s.retired_at IS NULL
ORDER BY e.embedding <=> $1::vector, e.station_id ASC
LIMIT $2
"#,
            )
            .bind(&literal)
            .bind(ANN_CANDIDATE_LIMIT)
            .fetch_all(pool)
            .await
            .expect("ANN branch replica");
            // Exact kNN: sequential scan and sort with index scans disabled.
            let mut transaction = pool.begin().await.expect("exact transaction");
            sqlx::query("SET LOCAL enable_indexscan = off")
                .execute(&mut *transaction)
                .await
                .expect("disable index scans");
            sqlx::query("SET LOCAL enable_bitmapscan = off")
                .execute(&mut *transaction)
                .await
                .expect("disable bitmap scans");
            let exact: Vec<String> = sqlx::query_scalar(
                "SELECT station_id FROM station_embeddings_v1                  ORDER BY embedding <=> $1::vector, station_id ASC LIMIT $2",
            )
            .bind(&literal)
            .bind(100_i64)
            .fetch_all(&mut *transaction)
            .await
            .expect("exact kNN");
            transaction.rollback().await.expect("rollback exact read");
            for (index, k) in ks.iter().enumerate() {
                for (sums, candidates) in [
                    (&mut index_sums, &index_ann),
                    (&mut branch_sums, &branch_ann),
                ] {
                    let candidate_set: BTreeSet<&str> =
                        candidates.iter().take(*k).map(String::as_str).collect();
                    let overlap = exact
                        .iter()
                        .take(*k)
                        .filter(|id| candidate_set.contains(id.as_str()))
                        .count();
                    sums[index] += overlap as f64 / *k as f64;
                }
            }
            query_count += 1;
        }
        println!(
            "ann_recall lang={} queries={} hnsw_recall@10={:.4} hnsw_recall@100={:.4} branch_recall@10={:.4} branch_recall@100={:.4}",
            set.language,
            query_count,
            index_sums[0] / query_count as f64,
            index_sums[1] / query_count as f64,
            branch_sums[0] / query_count as f64,
            branch_sums[1] / query_count as f64,
        );
    }

    // Plan proofs for the selective query: the pure kNN must use the v1 HNSW
    // index; the joined production shape is printed for the record.
    let probe = fixture
        .sets
        .iter()
        .find(|set| set.language == "en")
        .unwrap();
    let embedding = provider
        .embed(&probe.cases[0].query)
        .await
        .expect("probe embedding");
    let literal = vector_literal(embedding.values());
    let index_plan: Vec<String> = sqlx::query_scalar(
        "EXPLAIN (COSTS) SELECT station_id FROM station_embeddings_v1          ORDER BY embedding <=> $1::vector LIMIT $2",
    )
    .bind(&literal)
    .bind(ANN_CANDIDATE_LIMIT)
    .fetch_all(pool)
    .await
    .expect("kNN EXPLAIN");
    let joined_index_plan = index_plan.join(
        "
",
    );
    println!(
        "explain_hnsw_knn lang=en
{joined_index_plan}"
    );
    assert!(
        joined_index_plan.contains("station_embeddings_v1_hnsw_cosine_idx"),
        "the pure kNN query must use the v1 HNSW index"
    );
    let branch_plan: Vec<String> = sqlx::query_scalar(
        r#"
EXPLAIN (COSTS)
SELECT e.station_id
FROM station_embeddings_v1 AS e
JOIN stations AS s ON s.id = e.station_id AND e.fields_hash = s.fields_hash
WHERE s.retired_at IS NULL
  AND s.language = 'en'
ORDER BY e.embedding <=> $1::vector, e.station_id ASC
LIMIT $2
"#,
    )
    .bind(&literal)
    .bind(ANN_CANDIDATE_LIMIT)
    .fetch_all(pool)
    .await
    .expect("branch EXPLAIN");
    println!(
        "explain_filtered_ann_branch lang=en
{}",
        branch_plan.join(
            "
"
        )
    );
}

/// Formats finite f32 components as a pgvector input literal.
fn vector_literal(values: &[f32]) -> String {
    let joined = values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("[{joined}]")
}
