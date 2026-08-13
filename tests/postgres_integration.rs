//! Opt-in integration coverage against a real PostgreSQL test database.

use std::{collections::BTreeSet, env, sync::Arc};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use rockserver::{
    http::{HealthResponse, HealthStatus, router_with_repository},
    persistence::PostgresStationRepository,
    search::{SearchConstraints, SearchService, normalize_query},
};
use serde_json::Value;
use tower::ServiceExt;

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

    let rock_query = normalize_query("rock".to_owned(), "en-US".to_owned());
    let tie_results = service
        .search(
            &rock_query,
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::new(),
            },
        )
        .await
        .expect("seeded search must succeed");
    assert_eq!(
        station_ids(&tie_results),
        ["station-rock-001", "station-rock-002"]
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
    assert_eq!(station_ids(&limited_results), ["station-rock-001"]);

    let jazz_query = normalize_query("jazz".to_owned(), "en-US".to_owned());
    let exclusion_results = service
        .search(
            &jazz_query,
            &SearchConstraints {
                limit: 10,
                excluded_station_ids: BTreeSet::from(["station-jazz-001".to_owned()]),
            },
        )
        .await
        .expect("excluded search must succeed");
    assert_eq!(station_ids(&exclusion_results), ["station-jazz-002"]);

    let app = router_with_repository(Arc::new(repository.clone()));
    let ready = app
        .clone()
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), axum::http::StatusCode::OK);

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
            .body(Body::from(r#"{"query":"calm instrumental jazz"}"#))
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["stations"][0]["id"], "station-jazz-001");
}

fn station_ids(results: &[rockserver::search::RankedStation]) -> Vec<&str> {
    results
        .iter()
        .map(|ranked| ranked.station.id.as_str())
        .collect()
}
