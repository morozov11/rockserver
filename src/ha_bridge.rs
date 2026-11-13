//! Owner-bound, read-only Home Assistant bridge model and storage boundary.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

/// Server-derived bridge identity and its operator-approved publication policy.
#[derive(Clone)]
pub struct BridgePrincipal {
    pub user_id: Uuid,
    pub integration_id: Uuid,
    pub credential_hash: [u8; 32],
    pub sensors: HashSet<String>,
    pub weather_entity: String,
}

/// One selected HA sensor observation; native attributes are never accepted.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorReading {
    pub entity_id: String,
    pub display_name: String,
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub device_class: Option<String>,
    pub quality: ReadingQuality,
    pub observed_at: String,
}

/// Quality distinguishes a missing or invalid value from a measured zero.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReadingQuality {
    Ok,
    Unknown,
    Unavailable,
}

/// A full replacement or ordered batch from the current bridge connection.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorBatch {
    pub revision: u64,
    pub sensors: Vec<SensorReading>,
}

/// Current weather and bounded daily forecast from one approved HA weather entity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherReading {
    pub entity_id: String,
    pub condition: Option<String>,
    pub temperature: Option<f64>,
    pub temperature_unit: Option<String>,
    pub humidity: Option<f64>,
    pub wind_speed: Option<f64>,
    pub wind_speed_unit: Option<String>,
    pub quality: ReadingQuality,
    pub observed_at: String,
    pub forecast_observed_at: Option<String>,
    pub forecast: Vec<ForecastDay>,
}

/// One daily forecast period; omitted fields stay absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForecastDay {
    pub datetime: String,
    pub condition: Option<String>,
    pub temperature: Option<f64>,
    pub templow: Option<f64>,
    pub precipitation: Option<f64>,
}

/// One ordered weather update with server-computed receive time and freshness.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherBatch {
    pub revision: u64,
    pub weather: WeatherReading,
}

/// A stored normalized value and the server's receive time.
#[derive(Clone, Debug, Serialize)]
pub struct StoredWeather {
    #[serde(flatten)]
    pub reading: WeatherReading,
    pub received_at: String,
    pub status: &'static str,
    pub forecast_status: &'static str,
    pub online: bool,
}

/// One owner-visible sensor with a stable integration-scoped ID and freshness.
#[derive(Clone, Debug, Serialize)]
pub struct VisibleSensor {
    pub entity_id: String,
    pub display_name: String,
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub device_class: Option<String>,
    pub quality: ReadingQuality,
    pub observed_at: String,
    pub received_at: String,
    pub status: &'static str,
}

/// Account-owned HA read model, including connection and last weather observation.
#[derive(Clone, Debug, Serialize)]
pub struct BridgeOverview {
    pub connected: bool,
    pub online: bool,
    pub allowed_sensors: Vec<String>,
    pub weather_entity: Option<String>,
    pub sensors: Vec<VisibleSensor>,
    pub weather: Option<StoredWeather>,
}

/// Checks one owner-chosen publication policy before issuing a bridge credential.
pub fn valid_selection(sensors: &[String], weather_entity: &str) -> bool {
    let mut unique = HashSet::new();
    sensors.len() <= 128
        && sensors
            .iter()
            .all(|id| valid_entity_id(id, "sensor.") && unique.insert(id))
        && valid_entity_id(weather_entity, "weather.")
}

/// Accepts only bounded HA sensor or weather entity IDs, never device commands.
pub fn valid_entity_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 128
        && value[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Rejects malformed, disallowed or misleading HA observations before persistence.
pub fn validate_sensors(principal: &BridgePrincipal, values: &[SensorReading]) -> bool {
    let mut seen = HashSet::new();
    values.len() <= 128
        && values.iter().all(|item| {
            principal.sensors.contains(&item.entity_id)
                && seen.insert(&item.entity_id)
                && !item.display_name.is_empty()
                && item.display_name.len() <= 128
                && valid_time(&item.observed_at)
                && item.value.is_some_and(f64::is_finite) == (item.quality == ReadingQuality::Ok)
                && (item.quality != ReadingQuality::Ok
                    || matches!(
                        (item.device_class.as_deref(), item.unit.as_deref()),
                        (Some("temperature"), Some("°C"))
                            | (Some("humidity" | "battery"), Some("%"))
                            | (Some("co2" | "carbon_dioxide"), Some("ppm"))
                            | (Some("pressure"), Some("hPa"))
                            | (Some("illuminance"), Some("lx"))
                            | (Some("power"), Some("W"))
                            | (Some("energy"), Some("kWh"))
                            | (Some("voltage"), Some("V"))
                            | (Some("current"), Some("A"))
                    ))
                && item.unit.as_ref().is_none_or(|unit| unit.len() <= 16)
                && item
                    .device_class
                    .as_ref()
                    .is_none_or(|class| class.len() <= 64)
        })
}

/// Checks the selected weather source, units, timestamps and finite numbers.
pub fn validate_weather(principal: &BridgePrincipal, value: &WeatherReading) -> bool {
    value.entity_id == principal.weather_entity
        && valid_time(&value.observed_at)
        && value.forecast.len() <= 7
        && value.forecast_observed_at.as_deref().is_none_or(valid_time)
        && (value.forecast.is_empty() || value.forecast_observed_at.is_some())
        && value.condition.as_ref().is_none_or(|s| s.len() <= 64)
        && value.temperature.is_none_or(f64::is_finite)
        && value
            .humidity
            .is_none_or(|n| n.is_finite() && (0.0..=100.0).contains(&n))
        && value.wind_speed.is_none_or(|n| n.is_finite() && n >= 0.0)
        && value
            .temperature_unit
            .as_deref()
            .is_none_or(|s| s == "°C" || s == "°F")
        && value
            .wind_speed_unit
            .as_deref()
            .is_none_or(|s| matches!(s, "km/h" | "m/s" | "mph"))
        && (value.quality == ReadingQuality::Ok
            || (value.temperature.is_none() && value.forecast.is_empty()))
        && value.forecast.iter().all(|day| {
            valid_time(&day.datetime)
                && day.condition.as_ref().is_none_or(|s| s.len() <= 64)
                && [day.temperature, day.templow, day.precipitation]
                    .iter()
                    .all(|n| n.is_none_or(f64::is_finite))
        })
}

fn valid_time(value: &str) -> bool {
    OffsetDateTime::parse(value, &Rfc3339).is_ok()
}

/// Computes current, stale or unavailable without trusting a client freshness flag.
pub fn weather_status(
    reading: &WeatherReading,
    received_at: &str,
    now: OffsetDateTime,
) -> &'static str {
    observation_status(reading.quality, &reading.observed_at, received_at, now)
}

/// Applies server receive time and connection state to one sensor observation.
pub fn sensor_status(
    reading: &SensorReading,
    received_at: &str,
    now: OffsetDateTime,
    online: bool,
) -> &'static str {
    connected_status(
        observation_status(reading.quality, &reading.observed_at, received_at, now),
        online,
    )
}

fn observation_status(
    quality: ReadingQuality,
    observed_at: &str,
    received_at: &str,
    now: OffsetDateTime,
) -> &'static str {
    if quality != ReadingQuality::Ok {
        return "unavailable";
    }
    let Ok(observed) = OffsetDateTime::parse(observed_at, &Rfc3339) else {
        return "unavailable";
    };
    let Ok(received) = OffsetDateTime::parse(received_at, &Rfc3339) else {
        return "unavailable";
    };
    if observed > now + time::Duration::minutes(5)
        || now - observed > time::Duration::minutes(45)
        || now - received > time::Duration::minutes(45)
    {
        "stale"
    } else {
        "fresh"
    }
}

/// Computes forecast freshness from the last successful HA action, not later state updates.
pub fn forecast_status(reading: &WeatherReading, now: OffsetDateTime) -> &'static str {
    let Some(stamp) = reading.forecast_observed_at.as_deref() else {
        return "unavailable";
    };
    let Ok(observed) = OffsetDateTime::parse(stamp, &Rfc3339) else {
        return "unavailable";
    };
    if reading.forecast.is_empty() {
        "unavailable"
    } else if observed > now + time::Duration::minutes(5)
        || now - observed > time::Duration::minutes(45)
    {
        "stale"
    } else {
        "fresh"
    }
}

/// A disconnected integration never presents a previously fresh value as current.
pub fn connected_status(status: &'static str, online: bool) -> &'static str {
    if !online && status == "fresh" {
        "stale"
    } else {
        status
    }
}

/// Hashes a bearer before storage or lookup; raw credentials are never logged.
pub fn credential_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Storage errors are intentionally opaque at the HTTP and WebSocket boundary.
#[derive(Debug)]
pub struct BridgeStoreError;

/// Outcomes for ordered messages from one active integration connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngestOutcome {
    Accepted,
    Replay,
    Stale,
}

/// Durable bridge storage, also implemented by deterministic test fakes.
#[async_trait]
pub trait BridgeStore: Send + Sync {
    /// Creates or replaces one owner's credential and exact publication policy.
    async fn provision(
        &self,
        user_id: Uuid,
        token_hash: &[u8; 32],
        sensors: &[String],
        weather_entity: &str,
    ) -> Result<bool, BridgeStoreError>;
    /// Revokes the owner's credential and immediately hides its observations.
    async fn revoke(&self, user_id: Uuid) -> Result<(), BridgeStoreError>;
    /// Resolves only an active integration credential to its owner and allowlist.
    async fn authenticate(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<BridgePrincipal>, BridgeStoreError>;
    /// Starts a new epoch, invalidating any prior connection for this integration.
    async fn begin(&self, principal: &BridgePrincipal, epoch: Uuid)
    -> Result<(), BridgeStoreError>;
    /// Atomically replaces or updates permitted sensors at one ordered revision.
    async fn sensors(
        &self,
        principal: &BridgePrincipal,
        epoch: Uuid,
        batch: SensorBatch,
        full: bool,
    ) -> Result<IngestOutcome, BridgeStoreError>;
    /// Atomically stores the selected weather observation at one ordered revision.
    async fn weather(
        &self,
        principal: &BridgePrincipal,
        epoch: Uuid,
        batch: WeatherBatch,
    ) -> Result<IngestOutcome, BridgeStoreError>;
    /// Confirms the current epoch is still active and records its heartbeat.
    async fn heartbeat(
        &self,
        principal: &BridgePrincipal,
        epoch: Uuid,
    ) -> Result<bool, BridgeStoreError>;
    /// Reads only the selected owner's last weather observation.
    async fn read_weather(&self, user_id: Uuid) -> Result<Option<StoredWeather>, BridgeStoreError>;
    /// Reads the owner-visible sensor and weather projection without native entity IDs.
    async fn read_overview(&self, user_id: Uuid) -> Result<BridgeOverview, BridgeStoreError>;
}

#[cfg(test)]
#[path = "ha_bridge/tests.rs"]
mod tests;
