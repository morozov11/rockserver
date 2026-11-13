//! PostgreSQL storage for account-owned Home Assistant bridge observations.

use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::ha_bridge::{
    BridgeOverview, BridgePrincipal, BridgeStore, BridgeStoreError, IngestOutcome, SensorBatch,
    SensorReading, StoredWeather, VisibleSensor, WeatherBatch, WeatherReading, connected_status,
    forecast_status, sensor_status, weather_status,
};

/// Durable bridge store sharing the migrated account database pool.
#[derive(Clone)]
pub struct PostgresHaBridgeStore {
    pool: PgPool,
}

impl PostgresHaBridgeStore {
    /// Uses the account store's already migrated PostgreSQL pool.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn failed(_: sqlx::Error) -> BridgeStoreError {
    BridgeStoreError
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("RFC3339 timestamp")
}

fn digest<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, BridgeStoreError> {
    Ok(Sha256::digest(serde_json::to_vec(value).map_err(|_| BridgeStoreError)?).to_vec())
}

async fn claim(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &BridgePrincipal,
    epoch: Uuid,
    revision: u64,
    hash: &[u8],
) -> Result<IngestOutcome, BridgeStoreError> {
    let row = sqlx::query(
        "SELECT i.epoch, i.revision, i.last_payload_hash FROM home_assistant_integrations i \
         JOIN users u ON u.id = i.user_id WHERE i.id = $1 AND i.user_id = $2 AND i.credential_hash = $3 \
         AND i.revoked_at IS NULL AND u.status = 'active' FOR UPDATE OF i",
    )
    .bind(principal.integration_id)
    .bind(principal.user_id)
    .bind(principal.credential_hash.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(failed)?;
    let Some(row) = row else {
        return Ok(IngestOutcome::Stale);
    };
    let stored_epoch: Option<Uuid> = row.get("epoch");
    if stored_epoch != Some(epoch) {
        return Ok(IngestOutcome::Stale);
    }
    let stored_revision: i64 = row.get("revision");
    let prior_hash: Option<Vec<u8>> = row.get("last_payload_hash");
    if revision == stored_revision as u64 && prior_hash.as_deref() == Some(hash) {
        return Ok(IngestOutcome::Replay);
    }
    if revision != stored_revision as u64 + 1 {
        return Ok(IngestOutcome::Stale);
    }
    sqlx::query("UPDATE home_assistant_integrations SET revision = $2, last_payload_hash = $3, last_ingest_at = now() WHERE id = $1")
        .bind(principal.integration_id).bind(revision as i64).bind(hash)
        .execute(&mut **tx).await.map_err(failed)?;
    Ok(IngestOutcome::Accepted)
}

#[async_trait]
impl BridgeStore for PostgresHaBridgeStore {
    async fn provision(
        &self,
        user_id: Uuid,
        token_hash: &[u8; 32],
        sensors: &[String],
        weather_entity: &str,
    ) -> Result<bool, BridgeStoreError> {
        let mut tx = self.pool.begin().await.map_err(failed)?;
        let row = sqlx::query(
            "INSERT INTO home_assistant_integrations (id, user_id, credential_hash, allowed_sensors, weather_entity) \
             SELECT $1, id, $3, $4, $5 FROM users WHERE id = $2 AND status = 'active' \
             ON CONFLICT (user_id) DO UPDATE SET credential_hash = EXCLUDED.credential_hash, \
             allowed_sensors = EXCLUDED.allowed_sensors, weather_entity = EXCLUDED.weather_entity, \
             revoked_at = NULL, epoch = NULL, revision = 0, last_payload_hash = NULL, last_ingest_at = NULL \
             RETURNING home_assistant_integrations.id"
        ).bind(Uuid::new_v4()).bind(user_id).bind(token_hash.as_slice()).bind(sensors).bind(weather_entity)
            .fetch_optional(&mut *tx).await.map_err(failed)?;
        let Some(row) = row else {
            return Ok(false);
        };
        let id: Uuid = row.get("id");
        sqlx::query("DELETE FROM home_assistant_sensor_states WHERE integration_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        sqlx::query("DELETE FROM home_assistant_weather_states WHERE integration_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        tx.commit().await.map_err(failed)?;
        Ok(true)
    }

    async fn update_selection(
        &self,
        user_id: Uuid,
        sensors: &[String],
        weather_entity: &str,
    ) -> Result<bool, BridgeStoreError> {
        let mut tx = self.pool.begin().await.map_err(failed)?;
        let row = sqlx::query(
            "UPDATE home_assistant_integrations i SET allowed_sensors = $2, weather_entity = $3, \
             epoch = NULL, revision = 0, last_payload_hash = NULL, last_ingest_at = NULL \
             FROM users u WHERE i.user_id = $1 AND i.user_id = u.id \
             AND i.revoked_at IS NULL AND u.status = 'active' RETURNING i.id",
        )
        .bind(user_id)
        .bind(sensors)
        .bind(weather_entity)
        .fetch_optional(&mut *tx)
        .await
        .map_err(failed)?;
        let Some(row) = row else {
            return Ok(false);
        };
        let id: Uuid = row.get("id");
        sqlx::query("DELETE FROM home_assistant_sensor_states WHERE integration_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        sqlx::query("DELETE FROM home_assistant_weather_states WHERE integration_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(failed)?;
        tx.commit().await.map_err(failed)?;
        Ok(true)
    }

    async fn revoke(&self, user_id: Uuid) -> Result<(), BridgeStoreError> {
        sqlx::query("UPDATE home_assistant_integrations SET revoked_at = now(), epoch = NULL, revision = 0, last_payload_hash = NULL WHERE user_id = $1 AND revoked_at IS NULL")
            .bind(user_id).execute(&self.pool).await.map_err(failed)?;
        Ok(())
    }

    async fn authenticate(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<BridgePrincipal>, BridgeStoreError> {
        let row = sqlx::query(
            "SELECT i.id, i.user_id, i.allowed_sensors, i.weather_entity \
             FROM home_assistant_integrations i JOIN users u ON u.id = i.user_id \
             WHERE i.credential_hash = $1 AND i.revoked_at IS NULL AND u.status = 'active'",
        )
        .bind(token_hash.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(failed)?;
        Ok(row.map(|row| BridgePrincipal {
            integration_id: row.get("id"),
            user_id: row.get("user_id"),
            credential_hash: *token_hash,
            sensors: row
                .get::<Vec<String>, _>("allowed_sensors")
                .into_iter()
                .collect::<HashSet<_>>(),
            weather_entity: row.get("weather_entity"),
        }))
    }

    async fn begin(
        &self,
        principal: &BridgePrincipal,
        epoch: Uuid,
    ) -> Result<(), BridgeStoreError> {
        let result = sqlx::query(
            "UPDATE home_assistant_integrations i SET epoch = $3, revision = 0, last_payload_hash = NULL \
             FROM users u WHERE i.id = $1 AND i.user_id = $2 AND i.user_id = u.id \
             AND i.revoked_at IS NULL AND i.credential_hash = $4 AND u.status = 'active' \
             AND i.weather_entity = $5 AND i.allowed_sensors @> $6 AND i.allowed_sensors <@ $6"
        ).bind(principal.integration_id).bind(principal.user_id).bind(epoch).bind(principal.credential_hash.as_slice())
            .bind(&principal.weather_entity).bind(principal.sensors.iter().cloned().collect::<Vec<_>>())
            .execute(&self.pool).await.map_err(failed)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(BridgeStoreError)
        }
    }

    async fn sensors(
        &self,
        principal: &BridgePrincipal,
        epoch: Uuid,
        batch: SensorBatch,
        full: bool,
    ) -> Result<IngestOutcome, BridgeStoreError> {
        let hash = digest(&(full, &batch.sensors))?;
        let mut tx = self.pool.begin().await.map_err(failed)?;
        let outcome = claim(&mut tx, principal, epoch, batch.revision, &hash).await?;
        if outcome != IngestOutcome::Accepted {
            return Ok(outcome);
        }
        let received = now();
        if full {
            // Mark prior entities absent from this complete snapshot as tombstones.
            sqlx::query(
                r#"UPDATE home_assistant_sensor_states SET reading = jsonb_set(jsonb_set(reading, '{quality}', '"unavailable"'), '{value}', 'null'), received_at = $2 WHERE integration_id = $1"#
            ).bind(principal.integration_id).bind(&received).execute(&mut *tx).await.map_err(failed)?;
        }
        for item in batch.sensors {
            let reading = serde_json::to_value(&item).map_err(|_| BridgeStoreError)?;
            sqlx::query(
                "INSERT INTO home_assistant_sensor_states (integration_id, entity_id, reading, received_at) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (integration_id, entity_id) DO UPDATE SET reading = EXCLUDED.reading, received_at = EXCLUDED.received_at"
            ).bind(principal.integration_id).bind(&item.entity_id).bind(reading).bind(&received)
                .execute(&mut *tx).await.map_err(failed)?;
        }
        tx.commit().await.map_err(failed)?;
        Ok(IngestOutcome::Accepted)
    }

    async fn weather(
        &self,
        principal: &BridgePrincipal,
        epoch: Uuid,
        batch: WeatherBatch,
    ) -> Result<IngestOutcome, BridgeStoreError> {
        let hash = digest(&batch.weather)?;
        let mut tx = self.pool.begin().await.map_err(failed)?;
        let outcome = claim(&mut tx, principal, epoch, batch.revision, &hash).await?;
        if outcome != IngestOutcome::Accepted {
            return Ok(outcome);
        }
        sqlx::query(
            "INSERT INTO home_assistant_weather_states (integration_id, reading, received_at) VALUES ($1, $2, $3) \
             ON CONFLICT (integration_id) DO UPDATE SET reading = EXCLUDED.reading, received_at = EXCLUDED.received_at"
        ).bind(principal.integration_id).bind(serde_json::to_value(batch.weather).map_err(|_| BridgeStoreError)?)
            .bind(now()).execute(&mut *tx).await.map_err(failed)?;
        tx.commit().await.map_err(failed)?;
        Ok(IngestOutcome::Accepted)
    }

    async fn heartbeat(
        &self,
        principal: &BridgePrincipal,
        epoch: Uuid,
    ) -> Result<bool, BridgeStoreError> {
        let result = sqlx::query(
            "UPDATE home_assistant_integrations i SET last_ingest_at = now() FROM users u \
             WHERE i.id = $1 AND i.user_id = $2 AND i.epoch = $3 AND i.user_id = u.id \
             AND i.revoked_at IS NULL AND i.credential_hash = $4 AND u.status = 'active'",
        )
        .bind(principal.integration_id)
        .bind(principal.user_id)
        .bind(epoch)
        .bind(principal.credential_hash.as_slice())
        .execute(&self.pool)
        .await
        .map_err(failed)?;
        Ok(result.rows_affected() == 1)
    }

    async fn read_weather(&self, user_id: Uuid) -> Result<Option<StoredWeather>, BridgeStoreError> {
        let row = sqlx::query(
            "SELECT i.id, w.reading, w.received_at, EXTRACT(EPOCH FROM (now() - i.last_ingest_at))::float8 AS lag_seconds FROM home_assistant_weather_states w \
             JOIN home_assistant_integrations i ON i.id = w.integration_id \
             JOIN users u ON u.id = i.user_id WHERE i.user_id = $1 AND i.revoked_at IS NULL AND u.status = 'active'"
        ).bind(user_id).fetch_optional(&self.pool).await.map_err(failed)?;
        row.map(|row| {
            let value: Value = row.get("reading");
            let mut reading: WeatherReading =
                serde_json::from_value(value).map_err(|_| BridgeStoreError)?;
            let integration_id: Uuid = row.get("id");
            reading.entity_id = format!("home_assistant.{}.weather", integration_id.simple());
            let received_at: String = row.get("received_at");
            let now = OffsetDateTime::now_utc();
            let lag: Option<f64> = row.get("lag_seconds");
            let online = lag.is_some_and(|seconds| (0.0..=60.0).contains(&seconds));
            let current = weather_status(&reading, &received_at, now);
            let forecast = forecast_status(&reading, now);
            Ok(StoredWeather {
                status: connected_status(current, online),
                forecast_status: connected_status(forecast, online),
                online,
                reading,
                received_at,
            })
        })
        .transpose()
    }

    async fn read_overview(&self, user_id: Uuid) -> Result<BridgeOverview, BridgeStoreError> {
        let integration = sqlx::query(
            "SELECT i.id, i.allowed_sensors, i.weather_entity, EXTRACT(EPOCH FROM (now() - i.last_ingest_at))::float8 AS lag_seconds \
             FROM home_assistant_integrations i JOIN users u ON u.id = i.user_id \
             WHERE i.user_id = $1 AND i.revoked_at IS NULL AND u.status = 'active'"
        ).bind(user_id).fetch_optional(&self.pool).await.map_err(failed)?;
        let Some(integration) = integration else {
            return Ok(BridgeOverview {
                connected: false,
                online: false,
                allowed_sensors: Vec::new(),
                weather_entity: None,
                sensors: Vec::new(),
                weather: None,
            });
        };
        let integration_id: Uuid = integration.get("id");
        let allowed: Vec<String> = integration.get("allowed_sensors");
        let weather_entity: String = integration.get("weather_entity");
        let lag: Option<f64> = integration.get("lag_seconds");
        let online = lag.is_some_and(|seconds| (0.0..=60.0).contains(&seconds));
        let rows = sqlx::query(
            "SELECT entity_id, reading, received_at FROM home_assistant_sensor_states \
             WHERE integration_id = $1 AND entity_id = ANY($2) ORDER BY entity_id LIMIT 128",
        )
        .bind(integration_id)
        .bind(&allowed)
        .fetch_all(&self.pool)
        .await
        .map_err(failed)?;
        let now = OffsetDateTime::now_utc();
        let sensors = rows
            .into_iter()
            .map(|row| {
                let native_id: String = row.get("entity_id");
                let reading: SensorReading =
                    serde_json::from_value(row.get("reading")).map_err(|_| BridgeStoreError)?;
                let received_at: String = row.get("received_at");
                let short_hash = Sha256::digest(native_id.as_bytes())[..8]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                Ok(VisibleSensor {
                    entity_id: format!("home_assistant.{}.{}", integration_id.simple(), short_hash),
                    display_name: reading.display_name.clone(),
                    value: reading.value,
                    unit: reading.unit.clone(),
                    device_class: reading.device_class.clone(),
                    quality: reading.quality,
                    observed_at: reading.observed_at.clone(),
                    status: sensor_status(&reading, &received_at, now, online),
                    received_at,
                })
            })
            .collect::<Result<Vec<_>, BridgeStoreError>>()?;
        Ok(BridgeOverview {
            connected: true,
            online,
            allowed_sensors: allowed,
            weather_entity: Some(weather_entity),
            sensors,
            weather: self.read_weather(user_id).await?,
        })
    }
}
