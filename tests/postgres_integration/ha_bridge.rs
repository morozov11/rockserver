//! Disposable PostgreSQL check for the owner-only HA sensor projection.

use rockserver::ha_bridge::{
    BridgePrincipal, BridgeStore, IngestOutcome, ReadingQuality, SensorBatch, SensorReading,
    credential_hash,
};
use rockserver::persistence::{PostgresAccountStore, PostgresHaBridgeStore};
use sqlx::PgPool;
use std::{collections::HashSet, env};
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
    let principal = BridgePrincipal {
        user_id: owner,
        integration_id: Uuid::new_v4(),
        credential_hash: credential_hash("ha1.disposable-test"),
        sensors: HashSet::from(["sensor.room_temperature".to_owned()]),
        weather_entity: "weather.home".to_owned(),
    };
    sqlx::query("INSERT INTO home_assistant_integrations (id, user_id, credential_hash, allowed_sensors, weather_entity) VALUES ($1, $2, $3, $4, $5)")
        .bind(principal.integration_id).bind(owner).bind(principal.credential_hash.as_slice())
        .bind(vec!["sensor.room_temperature"]).bind("weather.home").execute(&pool).await.unwrap();
    let store = PostgresHaBridgeStore::from_pool(pool.clone());
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
    assert_ne!(visible.sensors[0].entity_id, "sensor.room_temperature");
    assert!(!store.read_overview(other).await.unwrap().connected);
    sqlx::query("UPDATE home_assistant_integrations SET revoked_at = now() WHERE id = $1")
        .bind(principal.integration_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!store.read_overview(owner).await.unwrap().connected);
}
