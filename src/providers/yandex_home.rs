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

    /// Decrypts a token and returns every readable float-temperature property in the user's home.
    pub async fn temperature_sensors(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
    ) -> Result<Vec<YandexTemperatureSensor>, YandexHomeError> {
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
        Ok(temperature_sensors(&value))
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

fn temperature_sensors(value: &Value) -> Vec<YandexTemperatureSensor> {
    let rooms: std::collections::HashMap<&str, &str> = value
        .get("rooms")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|room| Some((room.get("id")?.as_str()?, room.get("name")?.as_str()?)))
        .collect();
    let mut sensors = Vec::new();
    for device in value
        .get("devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(device_name) = device.get("name").and_then(Value::as_str) else {
            continue;
        };
        let room_name = device
            .get("room")
            .and_then(Value::as_str)
            .and_then(|room| rooms.get(room).copied())
            .map(str::to_owned);
        for property in device
            .get("properties")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let valid_temperature = property.get("type").and_then(Value::as_str)
                == Some("devices.properties.float")
                && property
                    .pointer("/parameters/instance")
                    .and_then(Value::as_str)
                    == Some("temperature")
                && property.pointer("/state/instance").and_then(Value::as_str)
                    == Some("temperature")
                && property
                    .get("retrievable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            let Some(temperature) = valid_temperature
                .then(|| property.pointer("/state/value").and_then(Value::as_f64))
                .flatten()
            else {
                continue;
            };
            let unit = match property.pointer("/parameters/unit").and_then(Value::as_str) {
                Some("unit.temperature.kelvin") => "K",
                _ => "°C",
            };
            let updated_at = property
                .get("last_updated")
                .and_then(Value::as_f64)
                .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds as i64).ok())
                .and_then(|time| time.format(&Rfc3339).ok());
            sensors.push(YandexTemperatureSensor {
                device_name: device_name.to_owned(),
                room_name: room_name.clone(),
                temperature,
                unit,
                updated_at,
            });
        }
    }
    sensors.sort_by(|left, right| {
        left.room_name
            .cmp(&right.room_name)
            .then_with(|| left.device_name.cmp(&right.device_name))
    });
    sensors
}

#[cfg(test)]
mod tests {
    use super::{YandexHomeClient, YandexHomeError, temperature_sensors};
    use serde_json::json;

    #[test]
    fn parses_only_retrievable_temperature_properties() {
        let sensors = temperature_sensors(&json!({
            "rooms": [{"id": "room-1", "name": "Кухня"}],
            "devices": [{
                "name": "Датчик",
                "room": "room-1",
                "properties": [
                    {"type":"devices.properties.float","retrievable":true,"parameters":{"instance":"temperature","unit":"unit.temperature.celsius"},"state":{"instance":"temperature","value":22.5},"last_updated":1700000000},
                    {"type":"devices.properties.float","retrievable":false,"parameters":{"instance":"temperature"},"state":{"instance":"temperature","value":99}}
                ]
            }]
        }));
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].device_name, "Датчик");
        assert_eq!(sensors[0].room_name.as_deref(), Some("Кухня"));
        assert_eq!(sensors[0].temperature, 22.5);
        assert_eq!(sensors[0].unit, "°C");
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
