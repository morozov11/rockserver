//! Disposable PostgreSQL check for the owner-only HA sensor projection.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use rockserver::ha_bridge::{
    BridgeStore, IngestOutcome, ReadingQuality, SensorBatch, SensorReading, credential_hash,
};
use rockserver::persistence::{PostgresAccountStore, PostgresHaBridgeStore};
use rockserver::{
    auth::{NewBrowserSession, SecretHash},
    http::RouterBuilder,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{env, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn postgres_ha_bridge_owner_projection_and_revocation() {
    let url = env::var("TEST_DATABASE_URL").expect("isolated disposable database URL");
    let accounts = PostgresAccountStore::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    accounts
        .create_user_with_display_name(owner, "HA owner")
        .await
        .unwrap();
    accounts
        .create_user_with_display_name(other, "Other owner")
        .await
        .unwrap();
    let store = PostgresHaBridgeStore::from_pool(pool.clone());
    let hash = credential_hash("ha1.disposable-test");
    assert!(
        store
            .provision(
                owner,
                &hash,
                &["sensor.room_temperature".to_owned()],
                "weather.home"
            )
            .await
            .unwrap()
    );
    let principal = store.authenticate(&hash).await.unwrap().unwrap();
    let epoch = Uuid::new_v4();
    store.begin(&principal, epoch).await.unwrap();
    let sensor = SensorReading {
        entity_id: "sensor.room_temperature".to_owned(),
        display_name: "Room temperature".to_owned(),
        value: Some(21.5),
        unit: Some("°C".to_owned()),
        device_class: Some("temperature".to_owned()),
        quality: ReadingQuality::Ok,
        observed_at: "2026-11-12T12:00:00Z".to_owned(),
    };
    assert_eq!(
        store
            .sensors(
                &principal,
                epoch,
                SensorBatch {
                    revision: 1,
                    sensors: vec![sensor]
                },
                true
            )
            .await
            .unwrap(),
        IngestOutcome::Accepted
    );
    let visible = store.read_overview(owner).await.unwrap();
    assert!(visible.connected);
    assert_eq!(visible.sensors.len(), 1);
    assert!(visible.sensors[0].entity_id.starts_with("home_assistant."));
    assert_eq!(visible.sensors[0].display_name, "Room temperature");
    assert_eq!(visible.allowed_sensors, ["sensor.room_temperature"]);
    assert_eq!(visible.weather_entity.as_deref(), Some("weather.home"));
    assert_ne!(visible.sensors[0].entity_id, "sensor.room_temperature");
    assert!(!store.read_overview(other).await.unwrap().connected);
    assert!(
        store
            .update_selection(owner, &[], "weather.forecast_home_assistant")
            .await
            .unwrap()
    );
    assert!(
        !store
            .update_selection(other, &[], "weather.home")
            .await
            .unwrap()
    );
    let changed = store.authenticate(&hash).await.unwrap().unwrap();
    assert_eq!(changed.weather_entity, "weather.forecast_home_assistant");
    assert!(changed.sensors.is_empty());
    assert!(store.begin(&principal, Uuid::new_v4()).await.is_err());
    store.begin(&changed, Uuid::new_v4()).await.unwrap();
    assert_eq!(
        store
            .sensors(
                &principal,
                epoch,
                SensorBatch {
                    revision: 2,
                    sensors: vec![]
                },
                false
            )
            .await
            .unwrap(),
        IngestOutcome::Stale
    );
    let cleared = store.read_overview(owner).await.unwrap();
    assert!(!cleared.online);
    assert!(cleared.sensors.is_empty());
    assert!(cleared.weather.is_none());
    assert_eq!(
        cleared.weather_entity.as_deref(),
        Some("weather.forecast_home_assistant")
    );
    let replacement = credential_hash("ha1.replacement-test");
    assert!(
        store
            .provision(owner, &replacement, &[], "weather.home")
            .await
            .unwrap()
    );
    assert!(store.authenticate(&hash).await.unwrap().is_none());
    assert!(store.read_overview(owner).await.unwrap().sensors.is_empty());
    store.revoke(owner).await.unwrap();
    assert!(store.authenticate(&replacement).await.unwrap().is_none());
    assert!(!store.read_overview(owner).await.unwrap().connected);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn browser_ha_credential_route_requires_owner_and_csrf() {
    let url = env::var("TEST_DATABASE_URL").expect("isolated disposable database URL");
    let accounts = PostgresAccountStore::connect(&url).await.unwrap();
    let owner = Uuid::new_v4();
    accounts.create_user(owner).await.unwrap();
    let cookie = "ha-browser-cookie";
    let csrf = "ha-browser-csrf";
    let cookie_hash = SecretHash::new(Sha256::digest(cookie.as_bytes()).into());
    let csrf_hash = SecretHash::new(Sha256::digest(csrf.as_bytes()).into());
    assert!(
        accounts
            .create_browser_session(NewBrowserSession {
                session_id: Uuid::new_v4(),
                user_id: owner,
                session_token_hash: &cookie_hash,
                csrf_hash: &csrf_hash,
                passkey_reauthenticated_at_rfc3339: "2035-01-01T00:00:00Z",
                expires_at_rfc3339: "2035-02-01T00:00:00Z",
                absolute_expires_at_rfc3339: "2035-08-01T00:00:00Z",
            })
            .await
            .unwrap()
    );
    let bridge = PostgresHaBridgeStore::from_pool(PgPool::connect(&url).await.unwrap());
    let app = RouterBuilder::new()
        .with_account_store(accounts)
        .with_ha_bridge(Arc::new(bridge.clone()))
        .with_trusted_proxy_token("test-proxy")
        .build();
    let body = r#"{"sensors":["sensor.room_temperature"],"weather_entity":"weather.home"}"#;
    let request = |csrf_header: Option<&str>| {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/v1/browser/home-assistant/credential")
            .header("origin", "https://rockplatform.win")
            .header("x-forwarded-proto", "https")
            .header("x-rockserver-proxy", "test-proxy")
            .header("cookie", format!("rockserver_browser={cookie}"))
            .header("content-type", "application/json");
        if let Some(value) = csrf_header {
            builder = builder.header("x-csrf-token", value);
        }
        builder.body(Body::from(body)).unwrap()
    };
    assert_eq!(
        app.clone().oneshot(request(None)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let response = app.clone().oneshot(request(Some(csrf))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let payload: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let credential = payload["credential"].as_str().unwrap();
    assert!(
        bridge
            .authenticate(&credential_hash(credential))
            .await
            .unwrap()
            .is_some()
    );
    let selection = Request::builder()
        .method("PUT")
        .uri("/api/v1/browser/home-assistant/selection")
        .header("origin", "https://rockplatform.win")
        .header("x-forwarded-proto", "https")
        .header("x-rockserver-proxy", "test-proxy")
        .header("cookie", format!("rockserver_browser={cookie}"))
        .header("x-csrf-token", csrf)
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"sensors":[],"weather_entity":"weather.forecast_home_assistant"}"#,
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(selection).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        bridge
            .authenticate(&credential_hash(credential))
            .await
            .unwrap()
            .unwrap()
            .weather_entity,
        "weather.forecast_home_assistant"
    );
    let response = app.clone().oneshot(request(Some(csrf))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        bridge
            .authenticate(&credential_hash(credential))
            .await
            .unwrap()
            .is_none()
    );
    let delete = Request::builder()
        .method("DELETE")
        .uri("/api/v1/browser/home-assistant")
        .header("origin", "https://rockplatform.win")
        .header("x-forwarded-proto", "https")
        .header("x-rockserver-proxy", "test-proxy")
        .header("cookie", format!("rockserver_browser={cookie}"))
        .header("x-csrf-token", csrf)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.oneshot(delete).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert!(!bridge.read_overview(owner).await.unwrap().connected);
}
