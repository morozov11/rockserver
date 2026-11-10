//! PostgreSQL regressions for the bounded lexical search prefilter.

use std::{collections::BTreeSet, env};

use rockserver::persistence::PostgresStationRepository;
use rockserver::search::{SearchAction, SearchConstraints, SearchQuery, StationRepository};
use sqlx::PgPool;

/// Degraded matches must not consume the 200-row prefilter ahead of a playable station.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to an isolated disposable PostgreSQL database"]
async fn prefilter_excludes_degraded_streams_before_limit() {
    let (repository, pool) = setup().await;
    insert_group(&pool, "bad", 220, "degraded").await;
    insert_target(&pool, "zz-srch003-playable", "Generic Radio", "healthy").await;

    let results = repository
        .search(&query(None), &constraints(10, 0), None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].station.id, "zz-srch003-playable");
    assert_eq!(results[0].total_matches, Some(1));

    pool.close().await;
    repository.close().await;
}

/// Trigram strength must rank a late-ID candidate ahead of 220 equal-tag matches.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to an isolated disposable PostgreSQL database"]
async fn prefilter_scores_trigrams_before_limit_and_preserves_pagination() {
    let (repository, pool) = setup().await;
    insert_group(&pool, "generic", 220, "healthy").await;
    insert_target(&pool, "zz-srch003-trigram", "Sundown", "healthy").await;

    let query = query(Some("sundowm"));
    let first = repository
        .search(&query, &constraints(1, 0), None)
        .await
        .unwrap();
    let second = repository
        .search(&query, &constraints(1, 1), None)
        .await
        .unwrap();
    let page = repository
        .search(&query, &constraints(2, 0), None)
        .await
        .unwrap();
    assert_eq!(first[0].station.id, "zz-srch003-trigram");
    assert_eq!(first[0].station.id, page[0].station.id);
    assert_eq!(second[0].station.id, page[1].station.id);
    assert_eq!(first[0].total_matches, second[0].total_matches);

    let excluded = SearchConstraints {
        excluded_station_ids: BTreeSet::from(["zz-srch003-trigram".to_owned()]),
        ..constraints(1, 0)
    };
    let excluded_results = repository.search(&query, &excluded, None).await.unwrap();
    assert_ne!(excluded_results[0].station.id, "zz-srch003-trigram");

    pool.close().await;
    repository.close().await;
}

async fn setup() -> (PostgresStationRepository, PgPool) {
    let url = env::var("TEST_DATABASE_URL").expect("set isolated TEST_DATABASE_URL");
    assert_eq!(
        env::var("ROCKSERVER_SEARCH_EVAL_DISPOSABLE").as_deref(),
        Ok("1")
    );
    let repository = PostgresStationRepository::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query("TRUNCATE TABLE stations CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    (repository, pool)
}

async fn insert_group(pool: &PgPool, label: &str, count: i32, health: &str) {
    sqlx::query(
        "INSERT INTO stations (id, source_station_id, name, tags, searchable_text) \
         SELECT 'aa-srch003-' || $1 || '-' || lpad(n::text, 3, '0'), \
                'aa-srch003-' || $1 || '-' || lpad(n::text, 3, '0'), \
                'Generic Radio', ARRAY['srch003marker'], 'Generic Radio srch003marker' \
         FROM generate_series(1, $2) AS n",
    )
    .bind(label)
    .bind(count)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO station_streams (station_id, source_stream_id, stream_url, health, is_primary) \
         SELECT id, id, 'https://example.com/' || id, $1, true \
         FROM stations WHERE id LIKE 'aa-srch003-' || $2 || '-%'",
    )
    .bind(health)
    .bind(label)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_target(pool: &PgPool, id: &str, name: &str, health: &str) {
    sqlx::query("INSERT INTO stations (id, source_station_id, name, tags, searchable_text) VALUES ($1, $1, $2, ARRAY['srch003marker'], $2 || ' srch003marker')")
        .bind(id).bind(name).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO station_streams (station_id, source_stream_id, stream_url, health, is_primary) VALUES ($1, $1, 'https://example.com/' || $1, $2, true)")
        .bind(id).bind(health).execute(pool).await.unwrap();
}

fn query(term: Option<&str>) -> SearchQuery {
    SearchQuery {
        action: SearchAction::Play,
        original: term.unwrap_or("srch003marker").to_owned(),
        locale: "en-US".to_owned(),
        terms: term.into_iter().map(str::to_owned).collect(),
        tags: vec!["srch003marker".to_owned()],
        language: None,
        country_code: None,
        core_term_count: usize::from(term.is_some()),
        raw_query: term.unwrap_or("").to_owned(),
        prefer_station_name: false,
        station_name_hint_queries: Vec::new(),
    }
}

fn constraints(limit: usize, offset: usize) -> SearchConstraints {
    SearchConstraints {
        limit,
        offset,
        excluded_station_ids: BTreeSet::new(),
    }
}
