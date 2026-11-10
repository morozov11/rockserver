//! Opt-in SRCH-006 factorial comparison; no production preprocessing is changed.

use std::{collections::BTreeSet, sync::Arc, time::Instant};

use async_trait::async_trait;
use rockserver::{
    persistence::{PostgresEmbeddingStore, PostgresStationRepository},
    search::{
        DeterministicQueryParser, Embedding, EmbeddingProvenance, EmbeddingProvider,
        EmbeddingProviderError, EmbeddingStore, QueryParserInput, SearchConstraints, SearchService,
        StationRepository,
    },
};
use sqlx::{PgPool, Row};

use super::{DIMENSION, GoldenSet, MODEL, Metrics, VERSION};

const CONNECTED_VERSION: &str = "onnx-v1-srch006-connected-v1";

struct EvalProvider {
    inner: Arc<dyn EmbeddingProvider>,
    cleaned: bool,
    connected: bool,
    station_names: Arc<BTreeSet<String>>,
}

impl EvalProvider {
    fn version(&self) -> &'static str {
        if self.connected {
            CONNECTED_VERSION
        } else {
            VERSION
        }
    }

    fn with_version(&self, embedding: Embedding) -> Result<Embedding, EmbeddingProviderError> {
        Embedding::new(
            MODEL,
            self.version(),
            DIMENSION,
            embedding.values().to_vec(),
        )
        .map_err(|_| EmbeddingProviderError::safe("evaluation embedding version invalid"))
    }
}

#[async_trait]
impl EmbeddingProvider for EvalProvider {
    fn provenance(&self) -> Option<EmbeddingProvenance> {
        Some(EmbeddingProvenance {
            model: MODEL.to_owned(),
            version: self.version().to_owned(),
            dimension: DIMENSION,
        })
    }

    async fn embed(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        let input = if self.cleaned {
            clean_command(text, &self.station_names)
        } else {
            text
        };
        self.with_version(self.inner.embed(input).await?)
    }

    async fn embed_document(&self, text: &str) -> Result<Embedding, EmbeddingProviderError> {
        self.with_version(self.inner.embed_document(text).await?)
    }
}

// Remove only a leading command; preserve complete station names and empty remainders.
fn clean_command<'a>(text: &'a str, station_names: &BTreeSet<String>) -> &'a str {
    let trimmed = text.trim();
    if station_names.contains(&trimmed.to_lowercase()) {
        return trimmed;
    }
    let lower = trimmed.to_lowercase();
    for prefix in ["put on ", "play ", "find ", "включи ", "поставь ", "найди "] {
        if lower.starts_with(prefix) {
            let rest = trimmed[prefix.len()..].trim();
            if !rest.is_empty() {
                return rest;
            }
        }
    }
    trimmed
}

#[test]
fn command_cleaning_preserves_names_and_noncommands() {
    let names = BTreeSet::from(["play radio".to_owned(), "metal rock radio".to_owned()]);
    assert_eq!(
        clean_command("play Radio Paradise Rock Mix", &names),
        "Radio Paradise Rock Mix"
    );
    assert_eq!(
        clean_command("включи Радио Каприз нью метал", &names),
        "Радио Каприз нью метал"
    );
    assert_eq!(clean_command("Play Radio", &names), "Play Radio");
    assert_eq!(clean_command("play ", &names), "play");
    assert_eq!(
        clean_command("Metal Rock Radio", &names),
        "Metal Rock Radio"
    );
    assert_eq!(
        clean_command("music to get energized", &names),
        "music to get energized"
    );
}

/// Runs all four query/document combinations against unchanged SQL metadata.
pub(super) async fn run(
    database_url: &str,
    pool: &PgPool,
    repository: &PostgresStationRepository,
    inner: Arc<dyn EmbeddingProvider>,
    fixture: &GoldenSet,
) {
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        database, "rockeval_srch006_tests",
        "use the isolated SRCH-006 DB"
    );
    let rows = sqlx::query(
        "SELECT id, name, tags, language, country_code, searchable_text FROM stations WHERE retired_at IS NULL ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 41);
    let station_names = Arc::new(
        rows.iter()
            .map(|row| row.get::<String, _>("name").to_lowercase())
            .collect::<BTreeSet<_>>(),
    );
    let store = PostgresEmbeddingStore::connect(database_url).await.unwrap();
    let connected_provider = EvalProvider {
        inner: Arc::clone(&inner),
        cleaned: false,
        connected: true,
        station_names: Arc::clone(&station_names),
    };
    for row in &rows {
        let name: String = row.get("name");
        let tags: Vec<String> = row.get("tags");
        let language: Option<String> = row.get("language");
        let country: Option<String> = row.get("country_code");
        let document = format!(
            "{} is an internet radio station. It plays {} music. Broadcast language: {}. Country: {}.",
            name,
            tags.join(", "),
            language.as_deref().unwrap_or("unspecified"),
            country.as_deref().unwrap_or("unspecified"),
        );
        let embedding = connected_provider.embed_document(&document).await.unwrap();
        store
            .upsert_embedding(row.get("id"), &embedding)
            .await
            .unwrap();
    }
    println!(
        "format_example current={:?} connected={:?}",
        rows[0].get::<String, _>("searchable_text"),
        {
            let name: String = rows[0].get("name");
            let tags: Vec<String> = rows[0].get("tags");
            format!(
                "{} is an internet radio station. It plays {} music. Broadcast language: {}. Country: {}.",
                name,
                tags.join(", "),
                rows[0]
                    .get::<Option<String>, _>("language")
                    .as_deref()
                    .unwrap_or("unspecified"),
                rows[0]
                    .get::<Option<String>, _>("country_code")
                    .as_deref()
                    .unwrap_or("unspecified")
            )
        }
    );
    for (connected, document_label) in [(false, "current"), (true, "connected")] {
        for (cleaned, query_label) in [(false, "raw"), (true, "cleaned")] {
            let provider = Arc::new(EvalProvider {
                inner: Arc::clone(&inner),
                cleaned,
                connected,
                station_names: Arc::clone(&station_names),
            });
            evaluate_cell(repository, provider, fixture, query_label, document_label).await;
        }
    }
    store.close().await;
}

async fn evaluate_cell(
    repository: &PostgresStationRepository,
    provider: Arc<EvalProvider>,
    fixture: &GoldenSet,
    query_label: &str,
    document_label: &str,
) {
    let service = SearchService::with_providers(
        Arc::new(repository.clone()),
        Arc::new(DeterministicQueryParser),
        Some(provider.clone()),
    );
    let constraints = SearchConstraints {
        limit: 10,
        offset: 0,
        excluded_station_ids: BTreeSet::new(),
    };
    let mut service_metrics = Metrics::default();
    let mut candidate_metrics = Metrics::default();
    let mut categories = std::collections::BTreeMap::<String, Metrics>::new();
    for case in fixture.cases.iter().chain(&fixture.paraphrase_cases) {
        let started = Instant::now();
        let outcome = service
            .interpret_and_search_private(
                QueryParserInput {
                    query: case.query.clone(),
                    locale: "en-US".to_owned(),
                },
                &constraints,
            )
            .await
            .unwrap();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let ids = outcome
            .stations
            .iter()
            .map(|row| row.station.id.clone())
            .collect::<Vec<_>>();
        service_metrics.record(&case.relevant, &ids, elapsed);
        categories
            .entry(case.category.clone())
            .or_default()
            .record(&case.relevant, &ids, elapsed);
        let embedding = provider.embed(&case.query).await.unwrap();
        let candidates = repository
            .search(
                &outcome.query,
                &SearchConstraints {
                    limit: 100,
                    ..constraints.clone()
                },
                Some(&embedding),
            )
            .await
            .unwrap();
        let candidate_ids = candidates
            .iter()
            .map(|row| row.station.id.clone())
            .collect::<Vec<_>>();
        candidate_metrics.record(&case.relevant, &candidate_ids, 0.0);
        let embedded_input = if provider.cleaned {
            clean_command(&case.query, &provider.station_names)
        } else {
            &case.query
        };
        println!(
            "format_case doc={document_label} query={query_label} category={} input={:?} embedded={:?} service={:?} candidates={:?}",
            case.category, case.query, embedded_input, ids, candidate_ids
        );
    }
    println!(
        "format_summary doc={document_label} query={query_label} service={} candidates={}",
        service_metrics.summary(),
        candidate_metrics.summary()
    );
    for (category, mut metrics) in categories {
        println!(
            "format_category doc={document_label} query={query_label} {category} {}",
            metrics.summary()
        );
    }
    let mut false_positives = 0;
    for text in &fixture.negative_cases {
        let visible = service
            .interpret_and_search_private(
                QueryParserInput {
                    query: text.clone(),
                    locale: "en-US".to_owned(),
                },
                &constraints,
            )
            .await
            .unwrap();
        if !visible.stations.is_empty() {
            false_positives += 1;
            println!(
                "format_false_positive doc={document_label} query={query_label} input={text:?} ids={:?}",
                visible
                    .stations
                    .iter()
                    .map(|row| row.station.id.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
    println!(
        "format_ood doc={document_label} query={query_label} false_positives={false_positives}/{}",
        fixture.negative_cases.len()
    );
}
