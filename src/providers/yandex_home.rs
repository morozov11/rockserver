//! Yandex Smart Home OAuth and read-only temperature sensor client.

use std::{env, fmt, time::Duration};

use reqwest::{Client, StatusCode};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;
use uuid::Uuid;

use crate::auth::webauthn::ORIGIN;

/// Environment variable containing the Yandex OAuth application identifier.
pub const CLIENT_ID_ENV: &str = "YANDEX_HOME_CLIENTID";
/// Environment variable containing the Yandex OAuth application secret.
pub const CLIENT_SECRET_ENV: &str = "YANDEX_HOME_SECRET";
const AUTHORIZE_URL: &str = "https://oauth.yandex.com/authorize";
const TOKEN_URL: &str = "https://oauth.yandex.com/token";
const USER_INFO_URL: &str = "https://api.iot.yandex.net/v1.0/user/info";
const CALLBACK_PATH: &str = "/api/v1/browser/yandex-home/callback";
const TOKEN_AAD: &[u8] = b"rockserver:yandex-home-token:v1";

/// Read-only Yandex OAuth configuration that intentionally redacts its secret in debug output.
#[derive(Clone)]
pub struct YandexHomeClient {
    client_id: String,
    client_secret: String,
    http: Client,
}

impl fmt::Debug for YandexHomeClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("YandexHomeClient")
            .field("client_id", &"[CONFIGURED]")
            .field("client_secret", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// A safe sensor property with its value, unit, and timestamp.
#[derive(Clone, Debug, PartialEq)]
pub struct YandexSensorProperty {
    /// Internal property type, e.g. "devices.properties.float" or "devices.properties.event".
    pub property_type: String,
    /// Property instance identifier, e.g. "temperature", "humidity", "battery_level".
    pub instance: String,
    /// Human-friendly localized name, e.g. "Температура", "Влажность", "Заряд батареи".
    pub name: String,
    /// Numeric or string sensor value.
    pub value: Value,
    /// Human-friendly unit symbol, e.g. "°C", "%", "В", "Вт", "А".
    pub unit: Option<String>,
    /// Pre-formatted string value with unit, e.g. "14.1 °C", "69 %", "90 %", "220 В".
    pub formatted_value: String,
    /// Last update in RFC 3339 format if available.
    pub updated_at: Option<String>,
}

/// A device in the user's Yandex Smart Home with its collected sensor properties.
#[derive(Clone, Debug, PartialEq)]
pub struct YandexHomeDevice {
    /// Device identifier assigned by Yandex.
    pub id: String,
    /// User-assigned device name, e.g. "Климат", "Температура", "Увлажнитель".
    pub name: String,
    /// Yandex device type, e.g. "devices.types.sensor.climate", "devices.types.humidifier".
    pub device_type: Option<String>,
    /// User-visible room name when assigned to a room.
    pub room_name: Option<String>,
    /// All sensor properties with available readings.
    pub properties: Vec<YandexSensorProperty>,
}

/// A safe temperature reading returned to a signed-in Rock account.
#[derive(Clone, Debug, PartialEq)]
pub struct YandexTemperatureSensor {
    /// User-visible device name supplied by Yandex Smart Home.
    pub device_name: String,
    /// User-visible room name when the device belongs to a room.
    pub room_name: Option<String>,
    /// Current numeric temperature in the unit reported by Yandex.
    pub temperature: f64,
    /// Source unit restricted to Celsius or Kelvin.
    pub unit: &'static str,
    /// Last source update in RFC 3339 UTC form when supplied by Yandex.
    pub updated_at: Option<String>,
}

/// Safe failures that never contain OAuth credentials or provider response bodies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum YandexHomeError {
    /// The configuration is absent or only partially supplied.
    Unavailable,
    /// The OAuth callback did not contain a usable authorization code.
    InvalidAuthorizationCode,
    /// OAuth rejected the code or the saved token no longer grants access.
    AuthorizationRejected,
    /// The upstream service failed or returned an unusable payload.
    UpstreamUnavailable,
    /// Stored ciphertext could not be authenticated with the configured application secret.
    StoredTokenInvalid,
}

impl fmt::Display for YandexHomeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "Yandex Smart Home is unavailable",
            Self::InvalidAuthorizationCode => "Yandex OAuth callback is invalid",
            Self::AuthorizationRejected => "Yandex authorization was rejected",
            Self::UpstreamUnavailable => "Yandex Smart Home is temporarily unavailable",
            Self::StoredTokenInvalid => "stored Yandex authorization is invalid",
        })
    }
}

impl std::error::Error for YandexHomeError {}

#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
}

impl YandexHomeClient {
    /// Loads the optional integration only when both OAuth values are configured.
    pub fn optional_from_env() -> Result<Option<Self>, YandexHomeError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    /// Builds an authorization URL with a one-time server-owned state value.
    pub fn authorization_url(&self, state: &str) -> Result<String, YandexHomeError> {
        let mut url = Url::parse(AUTHORIZE_URL).map_err(|_| YandexHomeError::Unavailable)?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &callback_url())
            .append_pair("scope", "iot:view")
            .append_pair("state", state);
        Ok(url.into())
    }

    /// Exchanges a short-lived OAuth confirmation code for an encrypted-at-rest user token.
    pub async fn exchange_code(&self, code: &str) -> Result<(Vec<u8>, [u8; 12]), YandexHomeError> {
        if code.is_empty() || code.len() > 2048 {
            return Err(YandexHomeError::InvalidAuthorizationCode);
        }
        let response = self
            .http
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &callback_url()),
            ])
            .send()
            .await
            .map_err(|_| YandexHomeError::UpstreamUnavailable)?;
        if response.status().is_client_error() {
            return Err(YandexHomeError::AuthorizationRejected);
        }
        if !response.status().is_success() {
            return Err(YandexHomeError::UpstreamUnavailable);
        }
        let token = response
            .json::<OAuthTokenResponse>()
            .await
            .map_err(|_| YandexHomeError::UpstreamUnavailable)?;
        if token.access_token.is_empty() || token.access_token.len() > 4096 {
            return Err(YandexHomeError::AuthorizationRejected);
        }
        Ok(self.encrypt_token(&token.access_token))
    }

    /// Decrypts a token and returns all devices with readable sensor properties in the user's home.
    pub async fn home_devices(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
    ) -> Result<Vec<YandexHomeDevice>, YandexHomeError> {
        let token = self.decrypt_token(ciphertext, nonce)?;
        let response = self
            .http
            .get(USER_INFO_URL)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| YandexHomeError::UpstreamUnavailable)?;
        if response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::FORBIDDEN
        {
            return Err(YandexHomeError::AuthorizationRejected);
        }
        if !response.status().is_success() {
            return Err(YandexHomeError::UpstreamUnavailable);
        }
        let value = response
            .json::<Value>()
            .await
            .map_err(|_| YandexHomeError::UpstreamUnavailable)?;
        Ok(parse_home_devices(&value))
    }

    /// Decrypts a token and returns temperature readings in the user's home.
    pub async fn temperature_sensors(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
    ) -> Result<Vec<YandexTemperatureSensor>, YandexHomeError> {
        let devices = self.home_devices(ciphertext, nonce).await?;
        let mut sensors = Vec::new();
        for device in devices {
            for prop in device.properties {
                if prop.instance == "temperature"
                    && let Some(temp) = prop.value.as_f64()
                {
                    let unit = if prop.unit.as_deref() == Some("K") {
                        "K"
                    } else {
                        "°C"
                    };
                    sensors.push(YandexTemperatureSensor {
                        device_name: device.name.clone(),
                        room_name: device.room_name.clone(),
                        temperature: temp,
                        unit,
                        updated_at: prop.updated_at,
                    });
                }
            }
        }
        sensors.sort_by(|left, right| {
            left.room_name
                .cmp(&right.room_name)
                .then_with(|| left.device_name.cmp(&right.device_name))
        });
        Ok(sensors)
    }

    fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, YandexHomeError> {
        let client_id = lookup(CLIENT_ID_ENV).filter(|value| !value.trim().is_empty());
        let client_secret = lookup(CLIENT_SECRET_ENV).filter(|value| !value.trim().is_empty());
        let (client_id, client_secret) = match (client_id, client_secret) {
            (None, None) => return Ok(None),
            (Some(client_id), Some(client_secret)) => (client_id, client_secret),
            _ => return Err(YandexHomeError::Unavailable),
        };
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| YandexHomeError::Unavailable)?;
        Ok(Some(Self {
            client_id,
            client_secret,
            http,
        }))
    }

    fn encrypt_token(&self, token: &str) -> (Vec<u8>, [u8; 12]) {
        let mut nonce = [0_u8; 12];
        nonce.copy_from_slice(&Uuid::new_v4().into_bytes()[..12]);
        let mut ciphertext = token.as_bytes().to_vec();
        self.aead()
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(TOKEN_AAD),
                &mut ciphertext,
            )
            .expect("AES-GCM accepts a 12-byte nonce and bounded token");
        (ciphertext, nonce)
    }

    fn decrypt_token(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<String, YandexHomeError> {
        if nonce.len() != 12 || ciphertext.len() < 16 || ciphertext.len() > 8192 {
            return Err(YandexHomeError::StoredTokenInvalid);
        }
        let mut nonce_bytes = [0_u8; 12];
        nonce_bytes.copy_from_slice(nonce);
        let mut plaintext = ciphertext.to_vec();
        let plaintext = self
            .aead()
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(TOKEN_AAD),
                &mut plaintext,
            )
            .map_err(|_| YandexHomeError::StoredTokenInvalid)?;
        String::from_utf8(plaintext.to_vec()).map_err(|_| YandexHomeError::StoredTokenInvalid)
    }

    fn aead(&self) -> LessSafeKey {
        let mut hasher = Sha256::new();
        hasher.update(b"rockserver:yandex-home-encryption-key:v1");
        hasher.update(self.client_secret.as_bytes());
        LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, &hasher.finalize())
                .expect("SHA-256 is a valid AES-256 key"),
        )
    }
}

fn callback_url() -> String {
    format!("{ORIGIN}{CALLBACK_PATH}")
}

/// Parses devices and their sensor properties from Yandex Smart Home user/info response.
pub fn parse_home_devices(value: &Value) -> Vec<YandexHomeDevice> {
    let rooms: std::collections::HashMap<&str, &str> = value
        .get("rooms")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|room| Some((room.get("id")?.as_str()?, room.get("name")?.as_str()?)))
        .collect();
    let mut devices = Vec::new();
    for device in value
        .get("devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(device_id) = device.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(device_name) = device.get("name").and_then(Value::as_str) else {
            continue;
        };
        let device_type = device
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let room_name = device
            .get("room")
            .and_then(Value::as_str)
            .and_then(|room| rooms.get(room).copied())
            .map(str::to_owned);

        let mut properties = Vec::new();
        for property in device
            .get("properties")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let property_type = property
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();

            let instance = property
                .pointer("/parameters/instance")
                .or_else(|| property.pointer("/state/instance"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();

            let Some(state_val) = property.pointer("/state/value") else {
                continue;
            };
            if state_val.is_null() {
                continue;
            }

            let raw_unit = property.pointer("/parameters/unit").and_then(Value::as_str);
            let unit = format_unit(raw_unit);
            let name = format_property_name(&instance);
            let formatted_value = format_value(state_val, unit.as_deref());

            let updated_at = property
                .get("last_updated")
                .or_else(|| property.get("state_changed_at"))
                .and_then(Value::as_f64)
                .filter(|&secs| secs > 0.0)
                .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds as i64).ok())
                .and_then(|time| time.format(&Rfc3339).ok());

            properties.push(YandexSensorProperty {
                property_type,
                instance,
                name,
                value: state_val.clone(),
                unit,
                formatted_value,
                updated_at,
            });
        }

        properties.sort_by_key(|p| property_priority(&p.instance));

        if !properties.is_empty() {
            devices.push(YandexHomeDevice {
                id: device_id.to_owned(),
                name: device_name.to_owned(),
                device_type,
                room_name,
                properties,
            });
        }
    }

    devices.sort_by(|left, right| {
        left.room_name
            .cmp(&right.room_name)
            .then_with(|| left.name.cmp(&right.name))
    });
    devices
}

fn format_unit(raw_unit: Option<&str>) -> Option<String> {
    match raw_unit? {
        "unit.temperature.celsius" => Some("°C".to_owned()),
        "unit.temperature.kelvin" => Some("K".to_owned()),
        "unit.percent" => Some("%".to_owned()),
        "unit.volt" => Some("В".to_owned()),
        "unit.watt" => Some("Вт".to_owned()),
        "unit.ampere" => Some("А".to_owned()),
        "unit.pressure.mmhg" => Some("мм рт. ст.".to_owned()),
        "unit.pressure.pascal" => Some("Па".to_owned()),
        "unit.pressure.bar" => Some("бар".to_owned()),
        "unit.ppm" => Some("ppm".to_owned()),
        "unit.density.mcg_m3" => Some("мкг/м³".to_owned()),
        "unit.illumination.lux" => Some("лк".to_owned()),
        "unit.meter.cubic_meter" => Some("м³".to_owned()),
        "unit.meter.kilowatt_hour" => Some("кВт·ч".to_owned()),
        other => Some(other.strip_prefix("unit.").unwrap_or(other).to_owned()),
    }
}

fn format_property_name(instance: &str) -> String {
    match instance {
        "temperature" => "Температура".to_owned(),
        "humidity" => "Влажность".to_owned(),
        "battery_level" => "Заряд батареи".to_owned(),
        "co2_level" => "Уровень CO₂".to_owned(),
        "pressure" => "Давление".to_owned(),
        "voltage" => "Напряжение".to_owned(),
        "power" => "Мощность".to_owned(),
        "amperage" => "Сила тока".to_owned(),
        "pm1_density" => "PM1".to_owned(),
        "pm2.5_density" => "PM2.5".to_owned(),
        "pm10_density" => "PM10".to_owned(),
        "tvoc" => "ЛОВ (TVOC)".to_owned(),
        "water_level" => "Уровень воды".to_owned(),
        "illumination" => "Освещённость".to_owned(),
        "gas_concentration" => "Концентрация газа".to_owned(),
        "smoke_concentration" => "Концентрация дыма".to_owned(),
        "meter" => "Счётчик".to_owned(),
        "vibration" => "Вибрация".to_owned(),
        "open" => "Открытие".to_owned(),
        "motion" => "Движение".to_owned(),
        "leak" => "Протечка".to_owned(),
        "button" => "Кнопка".to_owned(),
        "voice_activity" => "Голосовая активность".to_owned(),
        "signal_level" => "Уровень сигнала".to_owned(),
        other => other.replace('_', " "),
    }
}

fn format_value(value: &Value, unit: Option<&str>) -> String {
    let base = match value {
        Value::Number(num) => num.to_string(),
        Value::String(s) => match s.as_str() {
            "opened" => "Открыто".to_owned(),
            "closed" => "Закрыто".to_owned(),
            "detected" => "Обнаружено".to_owned(),
            "not_detected" => "Не обнаружено".to_owned(),
            "click" => "Нажатие".to_owned(),
            "double_click" => "Двойное нажатие".to_owned(),
            "long_press" => "Удержание".to_owned(),
            "leak" => "Протечка".to_owned(),
            "dry" => "Сухо".to_owned(),
            "vibration" => "Вибрация".to_owned(),
            other => other.to_owned(),
        },
        Value::Bool(b) => {
            if *b {
                "Да".to_owned()
            } else {
                "Нет".to_owned()
            }
        }
        other => other.to_string(),
    };
    if let Some(unit) = unit {
        format!("{base} {unit}")
    } else {
        base
    }
}

fn property_priority(instance: &str) -> u32 {
    match instance {
        "temperature" => 1,
        "humidity" => 2,
        "pressure" => 3,
        "co2_level" => 4,
        "pm2.5_density" => 5,
        "pm10_density" => 6,
        "pm1_density" => 7,
        "tvoc" => 8,
        "battery_level" => 9,
        "voltage" => 10,
        "power" => 11,
        "amperage" => 12,
        "water_level" => 13,
        "illumination" => 14,
        _ => 50,
    }
}

#[cfg(test)]
mod tests {
    use super::{YandexHomeClient, YandexHomeError, parse_home_devices};
    use serde_json::json;

    #[test]
    fn parses_home_devices_and_properties() {
        let devices = parse_home_devices(&json!({
            "rooms": [{"id": "room-1", "name": "Спальня"}],
            "devices": [
                {
                    "id": "dev-1",
                    "name": "Климат",
                    "type": "devices.types.sensor.climate",
                    "room": "room-1",
                    "properties": [
                        {
                            "type": "devices.properties.float",
                            "reportable": true,
                            "retrievable": false,
                            "parameters": {"instance": "battery_level", "unit": "unit.percent"},
                            "state": {"instance": "battery_level", "value": 90},
                            "last_updated": 1700000000
                        },
                        {
                            "type": "devices.properties.float",
                            "reportable": true,
                            "retrievable": false,
                            "parameters": {"instance": "temperature", "unit": "unit.temperature.celsius"},
                            "state": {"instance": "temperature", "value": 14.1},
                            "last_updated": 1700000000
                        },
                        {
                            "type": "devices.properties.float",
                            "reportable": true,
                            "retrievable": false,
                            "parameters": {"instance": "humidity", "unit": "unit.percent"},
                            "state": {"instance": "humidity", "value": 69},
                            "last_updated": 1700000000
                        },
                        {
                            "type": "devices.properties.float",
                            "parameters": {"instance": "signal_level"},
                            "state": null
                        }
                    ]
                },
                {
                    "id": "dev-empty",
                    "name": "Лампочка",
                    "type": "devices.types.light",
                    "properties": []
                }
            ]
        }));
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "Климат");
        assert_eq!(devices[0].room_name.as_deref(), Some("Спальня"));
        assert_eq!(devices[0].properties.len(), 3);
        assert_eq!(devices[0].properties[0].instance, "temperature");
        assert_eq!(devices[0].properties[0].name, "Температура");
        assert_eq!(devices[0].properties[0].formatted_value, "14.1 °C");
        assert_eq!(devices[0].properties[1].instance, "humidity");
        assert_eq!(devices[0].properties[1].name, "Влажность");
        assert_eq!(devices[0].properties[1].formatted_value, "69 %");
        assert_eq!(devices[0].properties[2].instance, "battery_level");
        assert_eq!(devices[0].properties[2].name, "Заряд батареи");
        assert_eq!(devices[0].properties[2].formatted_value, "90 %");
    }

    #[test]
    fn requires_both_oauth_environment_values() {
        assert!(YandexHomeClient::from_lookup(|_| None).unwrap().is_none());
        assert!(matches!(
            YandexHomeClient::from_lookup(
                |name| (name == super::CLIENT_ID_ENV).then_some("id".to_owned())
            ),
            Err(YandexHomeError::Unavailable)
        ));
    }
}
