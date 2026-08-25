//! Bounded Radio Browser client used only by the explicit catalog importer.

use std::{collections::BTreeSet, env, time::Duration};

use async_trait::async_trait;
use reqwest::header::ACCEPT;
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

use crate::catalog::{
    CatalogImportError, CatalogImportProvider, ImportLimits, ImportPage, ImportedStation,
    ImportedStream,
};

/// Stable catalog ownership name for Radio Browser records.
pub const SOURCE: &str = "radio_browser";
/// Optional Radio Browser root endpoint override.
pub const BASE_URL_ENV: &str = "RADIO_BROWSER_BASE_URL";
/// Optional descriptive HTTP User-Agent override.
pub const USER_AGENT_ENV: &str = "RADIO_BROWSER_USER_AGENT";
/// Optional whole-request timeout in seconds.
pub const TIMEOUT_SECS_ENV: &str = "RADIO_BROWSER_TIMEOUT_SECS";
/// Optional number of upstream DTOs requested per page.
pub const PAGE_SIZE_ENV: &str = "RADIO_BROWSER_PAGE_SIZE";
/// Optional maximum number of pages fetched per run.
pub const MAX_PAGES_ENV: &str = "RADIO_BROWSER_MAX_PAGES";
/// Optional comma-separated tags to query by (fetches one pass per tag).
pub const TAGS_ENV: &str = "RADIO_BROWSER_TAGS";

/// Default DNS-balanced official Radio Browser endpoint.
pub const DEFAULT_BASE_URL: &str = "https://all.api.radio-browser.info";
/// Default request timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 15;
/// Default bounded page size.
pub const DEFAULT_PAGE_SIZE: usize = 100;
/// Default bounded number of pages per run.
pub const DEFAULT_MAX_PAGES: usize = 10;

const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 60;
const MIN_PAGE_SIZE: usize = 1;
const MAX_PAGE_SIZE: usize = 500;
const MIN_MAX_PAGES: usize = 1;
const MAX_MAX_PAGES: usize = 100;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_URL_CHARS: usize = 2_048;
const MAX_NAME_CHARS: usize = 200;
const MAX_TAGS: usize = 32;
const MAX_TAG_CHARS: usize = 64;
const MAX_CODEC_CHARS: usize = 32;
const MAX_BITRATE_KBPS: i64 = 2_000;

/// Validated client and pagination configuration loaded by the importer binary.
#[derive(Clone, Debug)]
pub struct RadioBrowserConfig {
    /// Official or test root endpoint without credentials, query, or fragment.
    pub base_url: Url,
    /// Explicit descriptive request User-Agent.
    pub user_agent: String,
    /// Whole-request timeout.
    pub timeout: Duration,
    /// Bounded pagination controls.
    pub limits: ImportLimits,
}

impl RadioBrowserConfig {
    /// Loads Radio Browser settings from environment variables with safe defaults and bounds.
    pub fn from_env() -> Result<Self, CatalogImportError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    // A lookup boundary keeps configuration tests deterministic and process-global-env free.
    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, CatalogImportError> {
        let base_value = lookup(BASE_URL_ENV).unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let mut base_url = Url::parse(&base_value).map_err(|_| {
            CatalogImportError::safe(format!("{BASE_URL_ENV} must be a valid HTTP(S) URL"))
        })?;
        if !matches!(base_url.scheme(), "http" | "https")
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(CatalogImportError::safe(format!(
                "{BASE_URL_ENV} must be an HTTP(S) root without credentials, query, or fragment"
            )));
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }

        let user_agent = lookup(USER_AGENT_ENV)
            .unwrap_or_else(|| format!("RockServer/{}", env!("CARGO_PKG_VERSION")));
        if user_agent.is_empty()
            || user_agent.len() > 128
            || reqwest::header::HeaderValue::from_str(&user_agent).is_err()
        {
            return Err(CatalogImportError::safe(format!(
                "{USER_AGENT_ENV} must be a non-empty HTTP header value of at most 128 bytes"
            )));
        }

        let timeout_secs = bounded_number(
            lookup(TIMEOUT_SECS_ENV),
            TIMEOUT_SECS_ENV,
            DEFAULT_TIMEOUT_SECS,
            MIN_TIMEOUT_SECS,
            MAX_TIMEOUT_SECS,
        )?;
        let page_size = bounded_number(
            lookup(PAGE_SIZE_ENV),
            PAGE_SIZE_ENV,
            DEFAULT_PAGE_SIZE,
            MIN_PAGE_SIZE,
            MAX_PAGE_SIZE,
        )?;
        let max_pages = bounded_number(
            lookup(MAX_PAGES_ENV),
            MAX_PAGES_ENV,
            DEFAULT_MAX_PAGES,
            MIN_MAX_PAGES,
            MAX_MAX_PAGES,
        )?;

        Ok(Self {
            base_url,
            user_agent,
            timeout: Duration::from_secs(timeout_secs),
            limits: ImportLimits {
                page_size,
                max_pages,
            },
        })
    }
}

fn bounded_number<T>(
    value: Option<String>,
    name: &str,
    default: T,
    minimum: T,
    maximum: T,
) -> Result<T, CatalogImportError>
where
    T: Copy + Ord + std::str::FromStr + fmt::Display,
{
    let parsed = match value {
        Some(value) => value.parse::<T>().map_err(|_| {
            CatalogImportError::safe(format!(
                "{name} must be an integer from {minimum} to {maximum}"
            ))
        })?,
        None => default,
    };
    if !(minimum..=maximum).contains(&parsed) {
        return Err(CatalogImportError::safe(format!(
            "{name} must be an integer from {minimum} to {maximum}"
        )));
    }
    Ok(parsed)
}

use std::fmt;

/// HTTP implementation of the Radio Browser catalog source boundary.
#[derive(Clone, Debug)]
pub struct RadioBrowserClient {
    client: reqwest::Client,
    endpoint: Url,
    /// Optional tag filter applied to each request.
    tag_filter: Option<String>,
}

impl RadioBrowserClient {
    /// Creates a client with an explicit User-Agent, timeout, and response-size cap.
    pub fn new(config: &RadioBrowserConfig) -> Result<Self, CatalogImportError> {
        Self::with_tag(config, None)
    }

    /// Creates a client that filters by a specific tag.
    pub fn with_tag(
        config: &RadioBrowserConfig,
        tag: Option<String>,
    ) -> Result<Self, CatalogImportError> {
        let client = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .timeout(config.timeout)
            .build()
            .map_err(|_| {
                CatalogImportError::safe("Radio Browser HTTP client configuration failed")
            })?;
        let endpoint = config
            .base_url
            .join("json/stations/search")
            .map_err(|_| CatalogImportError::safe("Radio Browser endpoint construction failed"))?;
        Ok(Self {
            client,
            endpoint,
            tag_filter: tag,
        })
    }
}

#[async_trait]
impl CatalogImportProvider for RadioBrowserClient {
    fn source(&self) -> &'static str {
        SOURCE
    }

    async fn fetch_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<ImportPage, CatalogImportError> {
        let mut params = vec![
            ("hidebroken", "true".to_owned()),
            ("order", "name".to_owned()),
            ("reverse", "false".to_owned()),
            ("offset", offset.to_string()),
            ("limit", limit.to_string()),
        ];
        if let Some(tag) = &self.tag_filter {
            params.push(("tag", tag.clone()));
            params.push(("tagExact", "true".to_owned()));
        }
        let mut response = self
            .client
            .get(self.endpoint.clone())
            .header(ACCEPT, "application/json")
            .query(&params)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    CatalogImportError::safe("Radio Browser request timed out")
                } else {
                    CatalogImportError::safe("Radio Browser request failed")
                }
            })?;

        if !response.status().is_success() {
            return Err(CatalogImportError::safe(format!(
                "Radio Browser returned HTTP {}",
                response.status().as_u16()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(CatalogImportError::safe(
                "Radio Browser response exceeded the 8 MiB safety limit",
            ));
        }

        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| CatalogImportError::safe("Radio Browser response body failed"))?
        {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(CatalogImportError::safe(
                    "Radio Browser response exceeded the 8 MiB safety limit",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let dtos = serde_json::from_slice::<Vec<RadioBrowserStationDto>>(&body)
            .map_err(|_| CatalogImportError::safe("Radio Browser returned invalid station JSON"))?;
        let fetched = dtos.len();
        let mut stations = Vec::with_capacity(fetched);
        let mut skipped = 0;
        for dto in dtos {
            match ImportedStation::try_from(dto) {
                Ok(station) => stations.push(station),
                Err(_) => skipped += 1,
            }
        }
        stations.sort_by(|left, right| left.source_station_id.cmp(&right.source_station_id));

        Ok(ImportPage {
            fetched,
            stations,
            skipped,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RadioBrowserStationDto {
    #[serde(default)]
    stationuuid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url_resolved: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    countrycode: String,
    #[serde(default)]
    languagecodes: String,
    #[serde(default)]
    codec: String,
    #[serde(default)]
    bitrate: i64,
    #[serde(default)]
    lastcheckok: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SkipReason {
    Broken,
    InvalidIdentity,
    EmptyName,
    InvalidStreamUrl,
}

impl TryFrom<RadioBrowserStationDto> for ImportedStation {
    type Error = SkipReason;

    fn try_from(dto: RadioBrowserStationDto) -> Result<Self, Self::Error> {
        if dto.lastcheckok != 1 {
            return Err(SkipReason::Broken);
        }
        let source_station_id = Uuid::parse_str(dto.stationuuid.trim())
            .map_err(|_| SkipReason::InvalidIdentity)?
            .hyphenated()
            .to_string();
        let name = normalized_text(&dto.name, MAX_NAME_CHARS);
        if name.is_empty() {
            return Err(SkipReason::EmptyName);
        }
        let stream_url =
            normalized_url(&dto.url_resolved, true).ok_or(SkipReason::InvalidStreamUrl)?;

        Ok(Self {
            source: SOURCE,
            id: format!("rb-{source_station_id}"),
            source_station_id: source_station_id.clone(),
            name,
            homepage_url: normalized_url(&dto.homepage, false),
            tags: normalized_tags(&dto.tags),
            language: normalized_language(&dto.languagecodes),
            country_code: normalized_country_code(&dto.countrycode),
            streams: vec![ImportedStream {
                source_stream_id: source_station_id.clone(),
                stream_url,
                codec: normalized_codec(&dto.codec),
                bitrate_kbps: (1..=MAX_BITRATE_KBPS)
                    .contains(&dto.bitrate)
                    .then_some(dto.bitrate as u32),
                is_primary: true,
            }],
        })
    }
}

fn normalized_text(value: &str, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

fn normalized_url(value: &str, required: bool) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut parsed = Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    parsed.set_fragment(None);
    let normalized = parsed.to_string();
    if normalized.chars().count() > MAX_URL_CHARS || (required && normalized.is_empty()) {
        None
    } else {
        Some(normalized)
    }
}

fn normalized_tags(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|tag| normalized_text(tag, MAX_TAG_CHARS).to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_TAGS)
        .collect()
}

fn normalized_language(value: &str) -> Option<String> {
    // Search locales produce two-letter constraints, so prefer any ISO 639-1-shaped candidate.
    let codes = value
        .split(',')
        .map(|code| code.trim().to_ascii_lowercase())
        .filter(|code| {
            (2..=3).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    codes
        .iter()
        .find(|code| code.len() == 2)
        .cloned()
        .or_else(|| codes.into_iter().find(|code| code.len() == 3))
}

fn normalized_country_code(value: &str) -> Option<String> {
    let code = value.trim().to_ascii_uppercase();
    (code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_uppercase())).then_some(code)
}

fn normalized_codec(value: &str) -> Option<String> {
    let codec = normalized_text(value, MAX_CODEC_CHARS).to_ascii_uppercase();
    (!codec.is_empty()).then_some(codec)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        Router,
        extract::State,
        http::{HeaderMap, StatusCode, Uri},
        response::IntoResponse,
        routing::get,
    };
    use serde_json::json;

    use super::{
        BASE_URL_ENV, MAX_PAGES_ENV, PAGE_SIZE_ENV, RadioBrowserClient, RadioBrowserConfig,
        RadioBrowserStationDto, USER_AGENT_ENV,
    };
    use crate::catalog::CatalogImportProvider;

    #[test]
    fn dto_mapping_is_normalized_bounded_and_deterministic() {
        let payload = json!({
            "stationuuid": "01234567-89AB-CDEF-0123-456789ABCDEF",
            "name": "  Test   Radio  ",
            "url_resolved": "https://stream.example.com/live#fragment",
            "homepage": "ftp://invalid.example.com",
            "tags": " Rock, jazz,rock,  Classic   Rock ",
            "countrycode": "us",
            "languagecodes": "EN,eng",
            "codec": " mp3 ",
            "bitrate": 128,
            "lastcheckok": 1
        });
        let first = super::ImportedStation::try_from(
            serde_json::from_value::<RadioBrowserStationDto>(payload.clone()).unwrap(),
        )
        .unwrap();
        let second = super::ImportedStation::try_from(
            serde_json::from_value::<RadioBrowserStationDto>(payload).unwrap(),
        )
        .unwrap();

        assert_eq!(first, second);
        assert_eq!(first.id, "rb-01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(first.name, "Test Radio");
        assert_eq!(
            first.streams[0].stream_url,
            "https://stream.example.com/live"
        );
        assert_eq!(first.homepage_url, None);
        assert_eq!(first.tags, ["classic rock", "jazz", "rock"]);
        assert_eq!(first.language.as_deref(), Some("en"));
        assert_eq!(first.country_code.as_deref(), Some("US"));
        assert_eq!(first.streams[0].codec.as_deref(), Some("MP3"));
        assert_eq!(first.streams[0].bitrate_kbps, Some(128));
    }

    #[test]
    fn language_normalization_prefers_iso_639_1_then_falls_back_to_first_three_letter_code() {
        assert_eq!(super::normalized_language("yue,zh").as_deref(), Some("zh"));
        assert_eq!(super::normalized_language("EN,eng").as_deref(), Some("en"));
        assert_eq!(
            super::normalized_language("yue,zho").as_deref(),
            Some("yue")
        );
    }

    #[test]
    fn invalid_or_unplayable_records_are_skipped() {
        let base = json!({
            "stationuuid": "01234567-89ab-cdef-0123-456789abcdef",
            "name": "Station",
            "url_resolved": "https://stream.example.com/live",
            "lastcheckok": 1
        });
        let mutations = [
            ("lastcheckok", json!(0)),
            ("stationuuid", json!("not-a-uuid")),
            ("name", json!("  ")),
            ("url_resolved", json!("file:///tmp/not-a-stream")),
        ];

        for (field, value) in mutations {
            let mut payload = base.clone();
            payload[field] = value;
            let dto = serde_json::from_value::<RadioBrowserStationDto>(payload).unwrap();
            assert!(super::ImportedStation::try_from(dto).is_err(), "{field}");
        }
    }

    #[test]
    fn configuration_defaults_and_bounds_are_enforced() {
        let defaults = RadioBrowserConfig::from_lookup(|_| None).unwrap();
        assert_eq!(defaults.limits.page_size, 100);
        assert_eq!(defaults.limits.max_pages, 10);
        assert_eq!(defaults.user_agent, "RockServer/0.1.0");

        let error = RadioBrowserConfig::from_lookup(|name| {
            (name == PAGE_SIZE_ENV).then(|| "501".to_owned())
        })
        .unwrap_err();
        assert!(error.safe_summary().contains(PAGE_SIZE_ENV));

        let error = RadioBrowserConfig::from_lookup(|name| match name {
            BASE_URL_ENV => Some("https://user:secret@example.com".to_owned()),
            MAX_PAGES_ENV => Some("2".to_owned()),
            _ => None,
        })
        .unwrap_err();
        assert!(error.safe_summary().contains(BASE_URL_ENV));
    }

    #[tokio::test]
    async fn client_sends_user_agent_and_bounded_pagination_parameters() {
        let response = json!([{
            "stationuuid": "01234567-89ab-cdef-0123-456789abcdef",
            "name": "Mock Radio",
            "url_resolved": "https://stream.example.com/live",
            "tags": "rock",
            "countrycode": "US",
            "languagecodes": "en",
            "codec": "MP3",
            "bitrate": 128,
            "lastcheckok": 1
        }])
        .to_string();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (base_url, server) = start_mock(StatusCode::OK, response, requests.clone()).await;
        let config = RadioBrowserConfig::from_lookup(|name| match name {
            BASE_URL_ENV => Some(base_url.clone()),
            USER_AGENT_ENV => Some("RockServer-Test/1.0".to_owned()),
            _ => None,
        })
        .unwrap();
        let client = RadioBrowserClient::new(&config).unwrap();

        let page = client.fetch_page(20, 10).await.unwrap();
        server.abort();

        assert_eq!(page.fetched, 1);
        assert_eq!(page.stations.len(), 1);
        let request = &requests.lock().unwrap()[0];
        assert_eq!(request.0, "RockServer-Test/1.0");
        assert!(request.1.starts_with("/json/stations/search?"));
        for parameter in [
            "hidebroken=true",
            "order=name",
            "reverse=false",
            "offset=20",
            "limit=10",
        ] {
            assert!(request.1.contains(parameter), "missing {parameter}");
        }
    }

    #[tokio::test]
    async fn client_returns_a_sanitized_error_for_http_failure() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (base_url, server) = start_mock(
            StatusCode::SERVICE_UNAVAILABLE,
            "secret body".to_owned(),
            requests,
        )
        .await;
        let config = RadioBrowserConfig::from_lookup(|name| {
            (name == BASE_URL_ENV).then(|| base_url.clone())
        })
        .unwrap();
        let client = RadioBrowserClient::new(&config).unwrap();

        let error = client.fetch_page(0, 1).await.unwrap_err();
        server.abort();

        assert_eq!(error.safe_summary(), "Radio Browser returned HTTP 503");
        assert!(!error.safe_summary().contains("secret"));
    }

    #[derive(Clone)]
    struct MockState {
        status: StatusCode,
        body: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
    }

    async fn start_mock(
        status: StatusCode,
        body: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/json/stations/search", get(mock_handler))
            .with_state(MockState {
                status,
                body,
                requests,
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), server)
    }

    async fn mock_handler(
        State(state): State<MockState>,
        headers: HeaderMap,
        uri: Uri,
    ) -> impl IntoResponse {
        let user_agent = headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        state
            .requests
            .lock()
            .unwrap()
            .push((user_agent, uri.to_string()));
        (state.status, state.body)
    }
}
