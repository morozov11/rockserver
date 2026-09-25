//! Persistent storage for prepared station-icon artifacts.
//!
//! This module intentionally stores only already-validated WebP bytes. Downloading and image
//! decoding are separate boundaries, so a read path can never fetch from an external origin.

use std::{
    error::Error,
    fmt, fs, io,
    io::Cursor,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use image::{DynamicImage, GenericImageView, ImageFormat, RgbaImage, imageops::FilterType};
use reqwest::redirect::Policy;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

/// Maximum accepted encoded source or administrator-upload byte size.
pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum homepage HTML bytes inspected while resolving a favicon candidate.
///
/// One MiB: production sampling showed a third of failing homepages simply
/// exceeded the earlier 256 KiB bound while being otherwise valid.
const MAX_HOMEPAGE_BYTES: usize = 1024 * 1024;
const MAX_SOURCE_PIXELS: u32 = 1_048_576;
const MAX_SOURCE_DIMENSION: u32 = 1_024;
const ICON_DIMENSION: u32 = 256;

/// A validated content-addressed key for one normalized WebP artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IconStorageKey(String);

impl IconStorageKey {
    /// Builds the canonical filename for a SHA-256 content hash.
    pub fn from_hash(hash: &[u8; 32]) -> Self {
        Self(format!("{}.webp", hex(hash)))
    }

    /// Parses a canonical storage key without allowing paths or alternate extensions.
    pub fn parse(value: &str) -> Result<Self, IconStorageError> {
        let valid = value.len() == 69
            && value.ends_with(".webp")
            && value[..64].bytes().all(|byte| byte.is_ascii_hexdigit());
        valid
            .then(|| Self(value.to_ascii_lowercase()))
            .ok_or(IconStorageError::InvalidKey)
    }

    /// Returns the filename that may be used only below the configured storage root.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Safe storage errors that never include a filesystem path or source bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconStorageError {
    /// A caller supplied a non-canonical or potentially unsafe key.
    InvalidKey,
    /// The configured storage cannot complete the requested operation.
    Unavailable,
}

impl fmt::Display for IconStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidKey => "station icon storage key is invalid",
            Self::Unavailable => "station icon storage is unavailable",
        })
    }
}

impl Error for IconStorageError {}

/// A validated and normalized ready-to-store WebP icon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedIcon {
    /// Lossless square WebP bytes safe for storage and HTTP delivery.
    pub bytes: Vec<u8>,
    /// SHA-256 content hash used as the storage key and strong ETag source.
    pub content_hash: [u8; 32],
    /// Final square artifact width.
    pub width: u32,
    /// Final square artifact height.
    pub height: u32,
}

/// Safe rejection class for untrusted icon sources and uploads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconValidationError {
    /// The source body is empty or exceeds the bounded byte limit.
    Size,
    /// The source URL is unusable (invalid, non-public, or unsuccessfully answered)
    /// or its bytes are not one of the accepted raster formats.
    Format,
    /// The source cannot be decoded as a safe raster image.
    Decode,
    /// The decoded source dimensions exceed the fixed processing limit.
    Dimensions,
}

impl fmt::Display for IconValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Size => "station icon size is invalid",
            Self::Format => "station icon format is invalid",
            Self::Decode => "station icon cannot be decoded",
            Self::Dimensions => "station icon dimensions are invalid",
        })
    }
}

impl Error for IconValidationError {}

/// Safe terminal result of a manual administrator icon change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManualIconError {
    /// The supplied untrusted bytes failed the shared raster validation boundary.
    Validation(IconValidationError),
    /// The requested station does not exist, or has no manual override to remove.
    NotFound,
    /// Metadata or persistent artifact storage is temporarily unavailable.
    Unavailable,
}

impl fmt::Display for ManualIconError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::NotFound => formatter.write_str("station icon target was not found"),
            Self::Unavailable => formatter.write_str("station icon storage is unavailable"),
        }
    }
}

impl Error for ManualIconError {}

/// Validates a raster source and converts it to the sole stored v1 format: square WebP.
pub fn prepare_icon(source: &[u8]) -> Result<PreparedIcon, IconValidationError> {
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
        return Err(IconValidationError::Size);
    }
    let format = image::guess_format(source).map_err(|_| IconValidationError::Format)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Ico
    ) {
        return Err(IconValidationError::Format);
    }
    let image = image::load_from_memory_with_format(source, format)
        .map_err(|_| IconValidationError::Decode)?;
    let (width, height) = image.dimensions();
    if width == 0
        || height == 0
        || width > MAX_SOURCE_DIMENSION
        || height > MAX_SOURCE_DIMENSION
        || width.saturating_mul(height) > MAX_SOURCE_PIXELS
    {
        return Err(IconValidationError::Dimensions);
    }
    let resized = image
        .resize(ICON_DIMENSION, ICON_DIMENSION, FilterType::Lanczos3)
        .to_rgba8();
    let mut canvas = RgbaImage::new(ICON_DIMENSION, ICON_DIMENSION);
    let x = (ICON_DIMENSION - resized.width()) / 2;
    let y = (ICON_DIMENSION - resized.height()) / 2;
    image::imageops::overlay(&mut canvas, &resized, i64::from(x), i64::from(y));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(canvas)
        .write_to(&mut bytes, ImageFormat::WebP)
        .map_err(|_| IconValidationError::Decode)?;
    let bytes = bytes.into_inner();
    Ok(PreparedIcon {
        content_hash: Sha256::digest(&bytes).into(),
        bytes,
        width: ICON_DIMENSION,
        height: ICON_DIMENSION,
    })
}

/// Boundary for bounded external icon fetching, faked in tests without network access.
#[async_trait]
pub trait IconSourceFetcher: Send + Sync {
    /// Downloads and normalizes one validated external icon URL.
    async fn fetch_icon(&self, source: &str) -> Result<PreparedIcon, IconValidationError>;

    /// Resolves the ordered favicon candidate URLs for a station homepage, or why none
    /// can be fetched.
    async fn discover_homepage_icons(
        &self,
        homepage: &str,
    ) -> Result<Vec<String>, IconValidationError>;
}

/// Fetches external icons only after validating the URL and every redirect destination.
#[derive(Clone, Debug)]
pub struct SafeIconFetcher {
    client: reqwest::Client,
}

impl SafeIconFetcher {
    /// Creates a fetcher with bounded request time and no implicit redirects.
    ///
    /// A browser-like User-Agent is sent because many station sites block empty
    /// or tool-like agents outright; production sampling showed that a third of
    /// otherwise-failing homepages answer normally with one.
    pub fn new() -> Result<Self, IconStorageError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(Policy::none())
            .user_agent("Mozilla/5.0 (compatible; RockServer station-icon fetcher)")
            .build()
            .map_err(|_| IconStorageError::Unavailable)?;
        Ok(Self { client })
    }

    /// Downloads at most `limit` bytes from a validated publicly routable URL.
    ///
    /// Every redirect destination is re-validated against the same SSRF policy before the
    /// request is sent. An unsuccessful HTTP status classifies as `Format` because that URL
    /// is unusable, while transport failures classify as `Decode` so callers can retry.
    async fn fetch_bounded(
        &self,
        source: &str,
        limit: usize,
    ) -> Result<Vec<u8>, IconValidationError> {
        self.fetch_bounded_opt(source, limit, false).await
    }

    /// Downloads a bounded body; `truncate_oversize` stops reading at the limit instead of
    /// failing, which suits HTML inspection where the declared icon links sit in the document
    /// head and a heavy page must not hide them behind a size rejection.
    async fn fetch_bounded_opt(
        &self,
        source: &str,
        limit: usize,
        truncate_oversize: bool,
    ) -> Result<Vec<u8>, IconValidationError> {
        let mut url = Url::parse(source).map_err(|_| IconValidationError::Format)?;
        for _ in 0..=5 {
            validate_public_url(&url).await?;
            let mut response = self
                .client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| IconValidationError::Decode)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(IconValidationError::Format)?;
                url = url
                    .join(location)
                    .map_err(|_| IconValidationError::Format)?;
                continue;
            }
            if !response.status().is_success() {
                return Err(IconValidationError::Format);
            }
            if !truncate_oversize
                && response
                    .content_length()
                    .is_some_and(|size| size as usize > limit)
            {
                return Err(IconValidationError::Size);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| IconValidationError::Decode)?
            {
                if body.len() + chunk.len() > limit {
                    if truncate_oversize {
                        break;
                    }
                    return Err(IconValidationError::Size);
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(body);
        }
        Err(IconValidationError::Format)
    }
}

#[async_trait]
impl IconSourceFetcher for SafeIconFetcher {
    /// Downloads at most two MiB from a publicly routable HTTP(S) URL and normalizes it.
    async fn fetch_icon(&self, source: &str) -> Result<PreparedIcon, IconValidationError> {
        prepare_icon(&self.fetch_bounded(source, MAX_SOURCE_BYTES).await?)
    }

    /// Resolves the ordered favicon candidates for a validated station homepage.
    ///
    /// Returns every declared HTTP(S) `<link rel="...icon...">` target in document order
    /// followed by the homepage-root `/favicon.ico` fallback, deduplicated: production
    /// sampling showed both a deep-page relative link dying while the root icon lives and
    /// a first declared candidate being unusable, so the worker tries them in order.
    async fn discover_homepage_icons(
        &self,
        homepage: &str,
    ) -> Result<Vec<String>, IconValidationError> {
        // Heavy pages are truncated to the inspected prefix instead of failing: the declared
        // icon links live in the document head, and production sampling showed ~17% of
        // remaining permanent errors were pages heavier than the whole-body bound.
        let body = self
            .fetch_bounded_opt(homepage, MAX_HOMEPAGE_BYTES, true)
            .await?;
        let base = Url::parse(homepage).map_err(|_| IconValidationError::Format)?;
        Ok(homepage_icon_candidates(
            &String::from_utf8_lossy(&body),
            &base,
        ))
    }
}

/// Upper bound on the candidates inspected for one homepage.
const MAX_ICON_CANDIDATES: usize = 4;

/// Builds a homepage's ordered favicon candidates: declared icon links, then `/favicon.ico`.
fn homepage_icon_candidates(html: &str, base: &Url) -> Vec<String> {
    let mut candidates = icon_links(html, base, MAX_ICON_CANDIDATES);
    let mut fallback = base
        .join("/favicon.ico")
        .expect("an absolute HTTP(S) base always joins a root-relative path");
    fallback.set_fragment(None);
    let fallback = fallback.to_string();
    if !candidates.contains(&fallback) {
        candidates.push(fallback);
    }
    candidates
}

/// Extracts up to `max` distinct HTTP(S) icon links declared by bounded HTML, in document
/// order, resolved against the base.
///
/// Accepts `rel` tokens `icon` and `apple-touch-icon` (this naturally excludes the SVG-only
/// `mask-icon` token) and skips `data:` targets, which cannot be fetched like a URL.
fn icon_links(html: &str, base: &Url, max: usize) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    if max == 0 {
        return links;
    }
    let lowered = html.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(found) = lowered[cursor..].find("<link") {
        let tag_start = cursor + found;
        cursor = tag_start + "<link".len();
        // `<link` must end the tag name so prefixes like `<linker` are not matched.
        let after_name = lowered.as_bytes().get(cursor).copied();
        let name_terminated = after_name.is_none()
            || matches!(after_name, Some(b'>') | Some(b'/'))
            || after_name.is_some_and(|byte| byte.is_ascii_whitespace());
        if !name_terminated {
            continue;
        }
        let Some(tag_end) = lowered[cursor..].find('>') else {
            break;
        };
        let tag_end = cursor + tag_end;
        let attributes = parse_link_attributes(&html[tag_start..=tag_end]);
        let Some((_, rel)) = attributes.iter().find(|(name, _)| name == "rel") else {
            cursor = tag_end + 1;
            continue;
        };
        let is_icon_rel = rel
            .to_ascii_lowercase()
            .split_ascii_whitespace()
            .any(|token| matches!(token, "icon" | "apple-touch-icon"));
        if is_icon_rel
            && let Some((_, href)) = attributes.iter().find(|(name, _)| name == "href")
            && let Some(resolved) = resolve_icon_href(&unescape_href(href), base)
            && !links.contains(&resolved)
        {
            links.push(resolved);
            if links.len() == max {
                return links;
            }
        }
        cursor = tag_end + 1;
    }
    links
}

/// Resolves one link target to an absolute fragment-free HTTP(S) URL.
fn resolve_icon_href(href: &str, base: &Url) -> Option<String> {
    if href.is_empty() || href.starts_with("data:") {
        return None;
    }
    let mut url = base.join(href).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_fragment(None);
    Some(url.to_string())
}

/// Parses one `<link>` tag's attributes; names are lowercased, values are kept verbatim.
///
/// Handles double-quoted, single-quoted and unquoted values; slice offsets only ever stop on
/// ASCII delimiters, so slicing never splits a multi-byte character.
fn parse_link_attributes(tag: &str) -> Vec<(String, String)> {
    let bytes = tag.as_bytes();
    let mut attributes = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let name_start = index;
        while index < bytes.len() && bytes[index] != b'=' && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let name = tag[name_start..index].to_ascii_lowercase();
        let mut value = String::new();
        if index < bytes.len() && bytes[index] == b'=' {
            index += 1;
            if matches!(bytes.get(index), Some(b'"') | Some(b'\'')) {
                let quote = bytes[index];
                index += 1;
                let value_start = index;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
                value = tag[value_start..index].to_owned();
                if index < bytes.len() {
                    index += 1;
                }
            } else {
                let value_start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                value = tag[value_start..index].to_owned();
            }
        }
        attributes.push((name, value));
    }
    attributes
}

/// Decodes the small set of character entities valid inside an attribute value.
fn unescape_href(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

async fn validate_public_url(url: &Url) -> Result<(), IconValidationError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(IconValidationError::Format);
    }
    let host = url.host_str().expect("host was checked");
    if host.eq_ignore_ascii_case("localhost") {
        return Err(IconValidationError::Format);
    }
    let addresses = tokio::net::lookup_host((host, url.port_or_known_default().unwrap_or(80)))
        .await
        .map_err(|_| IconValidationError::Format)?;
    if addresses
        .into_iter()
        .any(|address| !public_ip(address.ip()))
    {
        return Err(IconValidationError::Format);
    }
    Ok(())
}

fn public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_broadcast()
                && !address.is_unspecified()
                && !address.is_documentation()
        }
        IpAddr::V6(address) => {
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
        }
    }
}

/// Durable, safe counters displayed by the administrator while one import job runs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IconJobProgress {
    /// Stable job identifier returned immediately after the protected start request.
    pub id: Uuid,
    /// Terminal or active lifecycle state.
    pub status: String,
    /// Items selected when the job started.
    pub selected: i32,
    /// Items that reached a terminal per-item result.
    pub processed: i32,
    /// Artifacts successfully normalized and published.
    pub ready: i32,
    /// Items with no usable automatic source.
    pub missing: i32,
    /// Items eligible for a later explicit retry.
    pub retryable_error: i32,
    /// Items rejected permanently by validation.
    pub permanent_error: i32,
    /// Items deliberately left untouched, such as manual overrides.
    pub skipped: i32,
}

/// A ready artifact resolved from metadata and storage without exposing its source URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyIcon {
    /// Prepared WebP bytes.
    pub bytes: Vec<u8>,
    /// SHA-256 content hash used by HTTP validators.
    pub content_hash: [u8; 32],
}

/// PostgreSQL coordinator for the one administrator-started station-icon job.
#[derive(Clone)]
pub struct IconImportCoordinator {
    pool: PgPool,
    storage: Arc<dyn IconStorage>,
    fetcher: Arc<dyn IconSourceFetcher>,
}

impl IconImportCoordinator {
    /// Reuses the production pool and prepared storage; it never performs work until `run` is called.
    pub fn new(pool: PgPool, storage: Arc<dyn IconStorage>) -> Result<Self, IconStorageError> {
        Ok(Self {
            pool,
            storage,
            fetcher: Arc::new(SafeIconFetcher::new()?),
        })
    }

    /// Marks a prior interrupted process-local run terminal without fetching any source.
    pub async fn interrupt_running(&self) -> Result<(), IconStorageError> {
        sqlx::query("UPDATE station_icon_jobs SET status = 'interrupted', finished_at = now() WHERE status = 'running'")
            .execute(&self.pool).await.map_err(|_| IconStorageError::Unavailable)?;
        Ok(())
    }

    /// Creates the one active job and snapshots its eligible station IDs for stable progress.
    pub async fn start(&self) -> Result<IconJobProgress, IconStorageError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| IconStorageError::Unavailable)?;
        let id = Uuid::new_v4();
        let inserted = sqlx::query("INSERT INTO station_icon_jobs (id, status) VALUES ($1, 'running') ON CONFLICT DO NOTHING")
            .bind(id).execute(&mut *transaction).await.map_err(|_| IconStorageError::Unavailable)?;
        if inserted.rows_affected() != 1 {
            return Err(IconStorageError::Unavailable);
        }
        let selected = sqlx::query("INSERT INTO station_icon_job_items (job_id, station_id) SELECT $1, id FROM stations s LEFT JOIN station_icons i ON i.station_id = s.id WHERE s.retired_at IS NULL AND COALESCE(i.manual_override, false) = false AND (i.status IS NULL OR i.status IN ('pending', 'missing', 'retryable_error') OR i.refresh_needed) ON CONFLICT DO NOTHING")
            .bind(id).execute(&mut *transaction).await.map_err(|_| IconStorageError::Unavailable)?.rows_affected();
        sqlx::query("UPDATE station_icon_jobs SET selected_count = $2 WHERE id = $1")
            .bind(id)
            .bind(i32::try_from(selected).map_err(|_| IconStorageError::Unavailable)?)
            .execute(&mut *transaction)
            .await
            .map_err(|_| IconStorageError::Unavailable)?;
        transaction
            .commit()
            .await
            .map_err(|_| IconStorageError::Unavailable)?;
        self.progress(id)
            .await?
            .ok_or(IconStorageError::Unavailable)
    }

    /// Reads the current persisted job counters without exposing source URLs or bytes.
    pub async fn progress(&self, id: Uuid) -> Result<Option<IconJobProgress>, IconStorageError> {
        sqlx::query_as::<_, IconJobProgressRow>("SELECT id, status, selected_count AS selected, processed_count AS processed, ready_count AS ready, missing_count AS missing, retryable_error_count AS retryable_error, permanent_error_count AS permanent_error, skipped_count AS skipped FROM station_icon_jobs WHERE id = $1")
            .bind(id).fetch_optional(&self.pool).await.map_err(|_| IconStorageError::Unavailable).map(|row| row.map(Into::into))
    }

    /// Returns the latest persisted job so a reloaded administrator console can resume progress.
    pub async fn latest_progress(&self) -> Result<Option<IconJobProgress>, IconStorageError> {
        sqlx::query_as::<_, IconJobProgressRow>("SELECT id, status, selected_count AS selected, processed_count AS processed, ready_count AS ready, missing_count AS missing, retryable_error_count AS retryable_error, permanent_error_count AS permanent_error, skipped_count AS skipped FROM station_icon_jobs ORDER BY started_at DESC LIMIT 1")
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| IconStorageError::Unavailable)
            .map(|row| row.map(Into::into))
    }

    /// Resolves only a ready metadata row whose content-addressed file is still present.
    pub async fn ready_icon(
        &self,
        station_id: &str,
    ) -> Result<Option<ReadyIcon>, IconStorageError> {
        let row = sqlx::query_as::<_, ReadyIconRow>("SELECT storage_key, content_hash FROM station_icons WHERE station_id = $1 AND status = 'ready'")
            .bind(station_id).fetch_optional(&self.pool).await.map_err(|_| IconStorageError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let key = IconStorageKey::parse(&row.storage_key)?;
        let hash: [u8; 32] = row
            .content_hash
            .try_into()
            .map_err(|_| IconStorageError::Unavailable)?;
        self.storage.get(&key).await.map(|bytes| {
            bytes.map(|bytes| ReadyIcon {
                bytes,
                content_hash: hash,
            })
        })
    }

    /// Validates and publishes a manual override without replacing the preserved source metadata.
    ///
    /// The artifact is stored before its ready metadata is committed, so an unavailable database
    /// can at worst leave an unreferenced content-addressed file, never a partial ready row.
    pub async fn replace_manual(
        &self,
        station_id: &str,
        source: &[u8],
    ) -> Result<(), ManualIconError> {
        let icon = prepare_icon(source).map_err(ManualIconError::Validation)?;
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM stations WHERE id = $1 AND retired_at IS NULL)",
        )
        .bind(station_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| ManualIconError::Unavailable)?;
        if !exists {
            return Err(ManualIconError::NotFound);
        }
        let key = IconStorageKey::from_hash(&icon.content_hash);
        self.storage
            .put_atomic(&key, &icon.bytes)
            .await
            .map_err(|_| ManualIconError::Unavailable)?;
        sqlx::query("INSERT INTO station_icons (station_id, storage_key, content_type, byte_size, width, height, content_hash, status, manual_override, refresh_needed) VALUES ($1, $2, 'image/webp', $3, $4, $5, $6, 'ready', true, false) ON CONFLICT (station_id) DO UPDATE SET storage_key = EXCLUDED.storage_key, content_type = EXCLUDED.content_type, byte_size = EXCLUDED.byte_size, width = EXCLUDED.width, height = EXCLUDED.height, content_hash = EXCLUDED.content_hash, status = 'ready', manual_override = true, refresh_needed = false, retry_after = NULL, last_error_code = NULL, updated_at = now()")
            .bind(station_id)
            .bind(key.as_str())
            .bind(i32::try_from(icon.bytes.len()).map_err(|_| ManualIconError::Unavailable)?)
            .bind(i32::try_from(icon.width).map_err(|_| ManualIconError::Unavailable)?)
            .bind(i32::try_from(icon.height).map_err(|_| ManualIconError::Unavailable)?)
            .bind(icon.content_hash.as_slice())
            .execute(&self.pool)
            .await
            .map_err(|_| ManualIconError::Unavailable)?;
        Ok(())
    }

    /// Detaches a manual override without deleting a possibly shared content-addressed artifact.
    ///
    /// A later explicit import may use the preserved automatic source. Orphan cleanup is an
    /// independent operator action so a remove can never break another station sharing the hash.
    pub async fn remove_manual(&self, station_id: &str) -> Result<(), ManualIconError> {
        let changed = sqlx::query("UPDATE station_icons SET storage_key = NULL, content_type = NULL, byte_size = NULL, width = NULL, height = NULL, content_hash = NULL, status = 'missing', manual_override = false, refresh_needed = true, retry_after = NULL, last_error_code = NULL, updated_at = now() WHERE station_id = $1 AND manual_override = true")
            .bind(station_id)
            .execute(&self.pool)
            .await
            .map_err(|_| ManualIconError::Unavailable)?;
        (changed.rows_affected() == 1)
            .then_some(())
            .ok_or(ManualIconError::NotFound)
    }

    /// Processes the snapshot one item at a time; callers run this only in a detached server task.
    pub async fn run(&self, id: Uuid) {
        while let Ok(Some(item)) = self.claim_item(id).await {
            let outcome = match resolve_item_plan(self.fetcher.as_ref(), &item).await {
                ItemPlan::Missing => "missing",
                ItemPlan::Retryable => "retryable_error",
                ItemPlan::Permanent => "permanent_error",
                ItemPlan::Ready {
                    icon,
                    source_url,
                    source_priority,
                } => match self
                    .publish(&item.station_id, &source_url, source_priority, &icon)
                    .await
                {
                    Ok(()) => "ready",
                    Err(_) => "retryable_error",
                },
            };
            let _ = self.finish_item(id, &item.station_id, outcome).await;
        }
        let _ = sqlx::query("UPDATE station_icon_jobs SET status = 'completed', finished_at = now() WHERE id = $1 AND status = 'running'")
            .bind(id).execute(&self.pool).await;
    }

    async fn claim_item(&self, id: Uuid) -> Result<Option<IconJobItem>, IconStorageError> {
        sqlx::query_as::<_, IconJobItem>("WITH next_item AS (SELECT station_id FROM station_icon_job_items WHERE job_id = $1 AND status = 'pending' ORDER BY station_id FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE station_icon_job_items item SET status = 'processing' FROM next_item WHERE item.job_id = $1 AND item.station_id = next_item.station_id RETURNING item.station_id, (SELECT source_url FROM station_icons WHERE station_id = item.station_id) AS source_url, (SELECT homepage_url FROM stations WHERE id = item.station_id) AS homepage_url")
            .bind(id).fetch_optional(&self.pool).await.map_err(|_| IconStorageError::Unavailable)
    }

    /// Upserts ready metadata so a station without a prior metadata row becomes ready atomically.
    ///
    /// A manual override row is never touched: the job cannot have selected it, but a concurrent
    /// administrator upload while the job runs must keep its explicit artifact and source.
    async fn publish(
        &self,
        station_id: &str,
        source_url: &str,
        source_priority: i16,
        icon: &PreparedIcon,
    ) -> Result<(), IconStorageError> {
        let key = IconStorageKey::from_hash(&icon.content_hash);
        self.storage.put_atomic(&key, &icon.bytes).await?;
        sqlx::query("INSERT INTO station_icons (station_id, source_url, source_priority, storage_key, content_type, byte_size, width, height, content_hash, status, refresh_needed) VALUES ($1, $2, $3, $4, 'image/webp', $5, $6, $7, $8, 'ready', false) ON CONFLICT (station_id) DO UPDATE SET source_url = CASE WHEN station_icons.manual_override THEN station_icons.source_url ELSE EXCLUDED.source_url END, source_priority = CASE WHEN station_icons.manual_override THEN station_icons.source_priority ELSE EXCLUDED.source_priority END, storage_key = CASE WHEN station_icons.manual_override THEN station_icons.storage_key ELSE EXCLUDED.storage_key END, content_type = CASE WHEN station_icons.manual_override THEN station_icons.content_type ELSE EXCLUDED.content_type END, byte_size = CASE WHEN station_icons.manual_override THEN station_icons.byte_size ELSE EXCLUDED.byte_size END, width = CASE WHEN station_icons.manual_override THEN station_icons.width ELSE EXCLUDED.width END, height = CASE WHEN station_icons.manual_override THEN station_icons.height ELSE EXCLUDED.height END, content_hash = CASE WHEN station_icons.manual_override THEN station_icons.content_hash ELSE EXCLUDED.content_hash END, status = CASE WHEN station_icons.manual_override THEN station_icons.status ELSE 'ready' END, refresh_needed = CASE WHEN station_icons.manual_override THEN station_icons.refresh_needed ELSE false END, retry_after = CASE WHEN station_icons.manual_override THEN station_icons.retry_after ELSE NULL END, last_error_code = CASE WHEN station_icons.manual_override THEN station_icons.last_error_code ELSE NULL END, updated_at = now()")
            .bind(station_id).bind(source_url).bind(source_priority).bind(key.as_str())
            .bind(i32::try_from(icon.bytes.len()).map_err(|_| IconStorageError::Unavailable)?)
            .bind(i32::try_from(icon.width).map_err(|_| IconStorageError::Unavailable)?)
            .bind(i32::try_from(icon.height).map_err(|_| IconStorageError::Unavailable)?)
            .bind(icon.content_hash.as_slice())
            .execute(&self.pool)
            .await
            .map_err(|_| IconStorageError::Unavailable)?;
        Ok(())
    }

    async fn finish_item(
        &self,
        id: Uuid,
        station_id: &str,
        outcome: &str,
    ) -> Result<(), IconStorageError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| IconStorageError::Unavailable)?;
        sqlx::query("UPDATE station_icon_job_items SET status = $3 WHERE job_id = $1 AND station_id = $2 AND status = 'processing'")
            .bind(id).bind(station_id).bind(outcome).execute(&mut *transaction).await.map_err(|_| IconStorageError::Unavailable)?;
        sqlx::query("UPDATE station_icon_jobs SET processed_count = processed_count + 1, ready_count = ready_count + CASE WHEN $2 = 'ready' THEN 1 ELSE 0 END, missing_count = missing_count + CASE WHEN $2 = 'missing' THEN 1 ELSE 0 END, retryable_error_count = retryable_error_count + CASE WHEN $2 = 'retryable_error' THEN 1 ELSE 0 END, permanent_error_count = permanent_error_count + CASE WHEN $2 = 'permanent_error' THEN 1 ELSE 0 END WHERE id = $1")
            .bind(id).bind(outcome).execute(&mut *transaction).await.map_err(|_| IconStorageError::Unavailable)?;
        transaction
            .commit()
            .await
            .map_err(|_| IconStorageError::Unavailable)
    }
}

#[derive(sqlx::FromRow)]
struct IconJobProgressRow {
    id: Uuid,
    status: String,
    selected: i32,
    processed: i32,
    ready: i32,
    missing: i32,
    retryable_error: i32,
    permanent_error: i32,
    skipped: i32,
}

impl From<IconJobProgressRow> for IconJobProgress {
    fn from(row: IconJobProgressRow) -> Self {
        Self {
            id: row.id,
            status: row.status,
            selected: row.selected,
            processed: row.processed,
            ready: row.ready,
            missing: row.missing,
            retryable_error: row.retryable_error,
            permanent_error: row.permanent_error,
            skipped: row.skipped,
        }
    }
}

#[derive(sqlx::FromRow)]
struct IconJobItem {
    station_id: String,
    source_url: Option<String>,
    homepage_url: Option<String>,
}

/// One item's fully resolved terminal plan before any persistence side effect.
#[derive(Debug, PartialEq)]
enum ItemPlan {
    /// No automatic source exists for the station.
    Missing,
    /// A bounded source operation failed transiently and may succeed on a later job.
    Retryable,
    /// The source is unusable and will not heal without new metadata.
    Permanent,
    /// A normalized artifact plus the automatic source that produced it.
    Ready {
        icon: PreparedIcon,
        source_url: String,
        source_priority: i16,
    },
}

/// Resolves one item's outcome without touching persistence so the decision stays testable.
///
/// Source priority follows the roadmap order: an explicit catalog icon URL wins (2), favicon
/// candidates discovered on the station homepage are the fallback (1), and neither leaves the
/// item missing. Homepage candidates are tried in order, so a dead declared link cannot hide
/// a living root icon. Only transport-level failures are retryable; unusable URLs (invalid,
/// non-public, unsuccessfully answered) and sources exceeding a fixed byte limit are
/// permanent so a dead address or oversized payload cannot loop forever.
async fn resolve_item_plan(fetcher: &dyn IconSourceFetcher, item: &IconJobItem) -> ItemPlan {
    let (sources, source_priority) = match item.source_url.as_deref() {
        Some(source) => (vec![source.to_owned()], 2),
        None => match item.homepage_url.as_deref() {
            None => return ItemPlan::Missing,
            Some(homepage) => match fetcher.discover_homepage_icons(homepage).await {
                Ok(candidates) if candidates.is_empty() => return ItemPlan::Permanent,
                Ok(candidates) => (candidates, 1),
                Err(IconValidationError::Decode) => {
                    return ItemPlan::Retryable;
                }
                Err(_) => return ItemPlan::Permanent,
            },
        },
    };
    let mut saw_retryable = false;
    for source in sources {
        match fetcher.fetch_icon(&source).await {
            Ok(icon) => {
                return ItemPlan::Ready {
                    icon,
                    source_url: source,
                    source_priority,
                };
            }
            // A candidate that merely timed out or overflowed does not poison the rest:
            // the next declared link may still work, and any retryable result keeps the
            // whole item eligible for a later job.
            Err(IconValidationError::Decode) => saw_retryable = true,
            Err(_) => {}
        }
    }
    if saw_retryable {
        ItemPlan::Retryable
    } else {
        ItemPlan::Permanent
    }
}

#[derive(sqlx::FromRow)]
struct ReadyIconRow {
    storage_key: String,
    content_hash: Vec<u8>,
}

/// Minimal storage boundary shared by HTTP delivery, the importer, and manual upload.
#[async_trait]
pub trait IconStorage: Send + Sync {
    /// Reads a complete previously published artifact, or `None` when it is absent.
    async fn get(&self, key: &IconStorageKey) -> Result<Option<Vec<u8>>, IconStorageError>;

    /// Atomically publishes bytes for a canonical key without replacing a completed artifact.
    async fn put_atomic(&self, key: &IconStorageKey, bytes: &[u8]) -> Result<(), IconStorageError>;

    /// Checks whether a prepared artifact is present.
    async fn exists(&self, key: &IconStorageKey) -> Result<bool, IconStorageError>;

    /// Removes one prepared artifact after its metadata has been safely detached.
    async fn delete(&self, key: &IconStorageKey) -> Result<(), IconStorageError>;
}

/// Filesystem storage rooted at one canonical, application-owned directory.
#[derive(Clone, Debug)]
pub struct FilesystemIconStorage {
    root: PathBuf,
}

impl FilesystemIconStorage {
    /// Creates the storage root when necessary and resolves it before accepting any key.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, IconStorageError> {
        fs::create_dir_all(root.as_ref()).map_err(|_| IconStorageError::Unavailable)?;
        let root = fs::canonicalize(root.as_ref()).map_err(|_| IconStorageError::Unavailable)?;
        Ok(Self { root })
    }

    fn path(&self, key: &IconStorageKey) -> PathBuf {
        self.root.join(key.as_str())
    }
}

#[async_trait]
impl IconStorage for FilesystemIconStorage {
    async fn get(&self, key: &IconStorageKey) -> Result<Option<Vec<u8>>, IconStorageError> {
        match fs::read(self.path(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(IconStorageError::Unavailable),
        }
    }

    async fn put_atomic(&self, key: &IconStorageKey, bytes: &[u8]) -> Result<(), IconStorageError> {
        let target = self.path(key);
        if target.exists() {
            return Ok(());
        }
        let temporary = self.root.join(format!(".{}.tmp", Uuid::new_v4()));
        fs::write(&temporary, bytes).map_err(|_| IconStorageError::Unavailable)?;
        match fs::rename(&temporary, &target) {
            Ok(()) => Ok(()),
            Err(_) if target.exists() => {
                let _ = fs::remove_file(temporary);
                Ok(())
            }
            Err(_) => {
                let _ = fs::remove_file(temporary);
                Err(IconStorageError::Unavailable)
            }
        }
    }

    async fn exists(&self, key: &IconStorageKey) -> Result<bool, IconStorageError> {
        match fs::metadata(self.path(key)) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(IconStorageError::Unavailable),
        }
    }

    async fn delete(&self, key: &IconStorageKey) -> Result<(), IconStorageError> {
        match fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(IconStorageError::Unavailable),
        }
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use async_trait::async_trait;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use url::Url;

    use std::sync::Mutex;

    use super::{
        FilesystemIconStorage, IconJobItem, IconSourceFetcher, IconStorage, IconStorageKey,
        IconValidationError, ItemPlan, PreparedIcon, homepage_icon_candidates, icon_links,
        prepare_icon, resolve_item_plan,
    };

    /// Deterministic offline fetcher used to verify job outcome classification.
    ///
    /// `fetched` records the candidate URLs the plan actually tried, in order.
    struct FakeFetcher {
        discovered: Result<Vec<String>, IconValidationError>,
        icon: Mutex<Vec<Result<PreparedIcon, IconValidationError>>>,
        fetched: Mutex<Vec<String>>,
    }

    impl FakeFetcher {
        /// Builds a fetcher that answers every candidate fetch with the same result.
        fn uniform(
            discovered: Result<Vec<String>, IconValidationError>,
            icon: Result<PreparedIcon, IconValidationError>,
        ) -> Self {
            Self {
                discovered,
                icon: Mutex::new(Vec::new()),
                fetched: Mutex::new(Vec::new()),
            }
            .with_icon_results(icon)
        }

        /// Queues per-candidate fetch results; the last one repeats.
        fn with_icon_results(self, first: Result<PreparedIcon, IconValidationError>) -> Self {
            *self.icon.lock().unwrap() = vec![first];
            self
        }

        /// Queues per-candidate fetch results; the last one repeats.
        fn with_results(self, results: Vec<Result<PreparedIcon, IconValidationError>>) -> Self {
            assert!(!results.is_empty());
            *self.icon.lock().unwrap() = results;
            self
        }

        /// URLs this fetcher was asked to fetch, in order.
        fn fetched(&self) -> Vec<String> {
            self.fetched.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl IconSourceFetcher for FakeFetcher {
        async fn fetch_icon(&self, source: &str) -> Result<PreparedIcon, IconValidationError> {
            self.fetched.lock().unwrap().push(source.to_owned());
            let mut results = self.icon.lock().unwrap();
            if results.len() > 1 {
                results.remove(0)
            } else {
                results[0].clone()
            }
        }

        async fn discover_homepage_icons(
            &self,
            _homepage: &str,
        ) -> Result<Vec<String>, IconValidationError> {
            self.discovered.clone()
        }
    }

    /// Builds one claimable item without touching persistence.
    fn item(source_url: Option<&str>, homepage_url: Option<&str>) -> IconJobItem {
        IconJobItem {
            station_id: "station-1".to_owned(),
            source_url: source_url.map(str::to_owned),
            homepage_url: homepage_url.map(str::to_owned),
        }
    }

    /// Builds a small valid prepared icon for ready-path assertions.
    fn sample_prepared() -> PreparedIcon {
        let mut source = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(32, 16, Rgba([9, 8, 7, 255])))
            .write_to(&mut source, ImageFormat::Png)
            .unwrap();
        prepare_icon(source.get_ref()).unwrap()
    }

    #[tokio::test]
    async fn filesystem_storage_publishes_and_removes_one_safe_key() {
        let root = std::env::temp_dir().join(format!("rockserver-icons-{}", uuid::Uuid::new_v4()));
        let storage = FilesystemIconStorage::open(&root).unwrap();
        let key = IconStorageKey::from_hash(&[7; 32]);
        storage.put_atomic(&key, b"webp").await.unwrap();
        assert!(storage.exists(&key).await.unwrap());
        assert_eq!(storage.get(&key).await.unwrap(), Some(b"webp".to_vec()));
        storage.delete(&key).await.unwrap();
        assert_eq!(storage.get(&key).await.unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_keys_are_content_addressed_and_never_paths() {
        assert!(IconStorageKey::parse("a".repeat(64).as_str()).is_err());
        assert!(IconStorageKey::parse("../icon.webp").is_err());
        assert_eq!(IconStorageKey::from_hash(&[0; 32]).as_str().len(), 69);
    }

    #[test]
    fn raster_input_becomes_a_square_webp() {
        let mut source = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(32, 16, Rgba([1, 2, 3, 255])))
            .write_to(&mut source, ImageFormat::Png)
            .unwrap();
        let prepared = prepare_icon(source.get_ref()).unwrap();
        assert_eq!((prepared.width, prepared.height), (256, 256));
        assert_eq!(
            image::guess_format(&prepared.bytes).unwrap(),
            ImageFormat::WebP
        );
    }

    #[test]
    fn invalid_or_oversized_input_is_never_prepared() {
        assert_eq!(prepare_icon(b"<svg />"), Err(IconValidationError::Format));
        assert_eq!(
            prepare_icon(&vec![0; 2 * 1024 * 1024 + 1]),
            Err(IconValidationError::Size)
        );
    }

    #[test]
    fn homepage_icon_link_is_extracted_and_resolved() {
        let base = Url::parse("https://radio.example/en/index.html").unwrap();
        let html = r#"<html><head><LINK REL="shortcut icon" HREF='/static/img/favicon.png'><link rel="stylesheet" href="styles.css"></head><body/></html>"#;
        assert_eq!(
            icon_links(html, &base, 4),
            vec!["https://radio.example/static/img/favicon.png"]
        );
    }

    #[test]
    fn apple_touch_icon_and_absolute_hrefs_are_accepted() {
        let base = Url::parse("https://radio.example/deep/page").unwrap();
        let html =
            r#"<head><link rel="apple-touch-icon" href="https://cdn.example/touch.png"></head>"#;
        assert_eq!(
            icon_links(html, &base, 4),
            vec!["https://cdn.example/touch.png"]
        );
    }

    #[test]
    fn mask_icon_data_and_entity_hrefs_are_handled() {
        let base = Url::parse("https://radio.example/deep/page").unwrap();
        let html = concat!(
            r#"<head><link rel="mask-icon" href="/icon.svg">"#,
            r#"<link rel=icon href="data:image/png;base64,AAA">"#,
            r#"<link rel="icon" type="image/png" href="favicon.png?v=2&amp;size=64"></head>"#,
        );
        assert_eq!(
            icon_links(html, &base, 4),
            vec!["https://radio.example/deep/favicon.png?v=2&size=64"]
        );
    }

    #[test]
    fn homepage_candidates_end_with_the_root_favicon_and_dedupe() {
        let base = Url::parse("https://radio.example/deep/page?from=nav#top").unwrap();
        assert_eq!(
            icon_links("<html><head><title>x</title></head></html>", &base, 4),
            Vec::<String>::new()
        );
        assert_eq!(
            homepage_icon_candidates("<html><head><title>x</title></head></html>", &base),
            vec!["https://radio.example/favicon.ico"]
        );
        let declared = r#"<head><link rel="icon" href="/a.png"><link rel="apple-touch-icon" href="/b.png"><link rel="icon" href="/a.png"></head>"#;
        assert_eq!(
            homepage_icon_candidates(declared, &base),
            vec![
                "https://radio.example/a.png",
                "https://radio.example/b.png",
                "https://radio.example/favicon.ico",
            ]
        );
    }

    #[tokio::test]
    async fn explicit_catalog_source_wins_with_top_priority() {
        // A wrongly consulted homepage discovery with this result would end the item
        // as Permanent instead of Ready, so this assertion also proves it is skipped.
        let fetcher = FakeFetcher::uniform(Err(IconValidationError::Format), Ok(sample_prepared()));
        let plan = resolve_item_plan(
            &fetcher,
            &item(
                Some("https://icons.example/explicit.png"),
                Some("https://radio.example"),
            ),
        )
        .await;
        assert!(matches!(
            plan,
            ItemPlan::Ready {
                source_priority: 2,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn homepage_discovery_is_the_fallback_source() {
        let fetcher = FakeFetcher::uniform(
            Ok(vec!["https://radio.example/favicon.ico".to_owned()]),
            Ok(sample_prepared()),
        );
        let plan = resolve_item_plan(&fetcher, &item(None, Some("https://radio.example"))).await;
        assert!(matches!(
            plan,
            ItemPlan::Ready {
                source_priority: 1,
                source_url: ref source,
                ..
            } if source == "https://radio.example/favicon.ico"
        ));
    }

    #[tokio::test]
    async fn candidates_are_tried_in_order_until_one_is_ready() {
        // First declared link is unusable, second is transport-flaky, root fallback works:
        // production showed both patterns, and the tried URLs must be recorded in order.
        let fetcher = FakeFetcher::uniform(
            Ok(vec![
                "https://radio.example/broken.png".to_owned(),
                "https://cdn.example/touch.png".to_owned(),
                "https://radio.example/favicon.ico".to_owned(),
            ]),
            Ok(sample_prepared()),
        )
        .with_results(vec![
            Err(IconValidationError::Format),
            Err(IconValidationError::Decode),
            Ok(sample_prepared()),
        ]);
        let plan = resolve_item_plan(&fetcher, &item(None, Some("https://radio.example"))).await;
        assert!(matches!(
            plan,
            ItemPlan::Ready {
                source_priority: 1,
                ref source_url,
                ..
            } if source_url == "https://radio.example/favicon.ico"
        ));
        assert_eq!(
            fetcher.fetched(),
            vec![
                "https://radio.example/broken.png".to_owned(),
                "https://cdn.example/touch.png".to_owned(),
                "https://radio.example/favicon.ico".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn all_dead_candidates_stay_permanent_but_flaky_ones_retry() {
        let all_dead = FakeFetcher::uniform(
            Ok(vec!["https://radio.example/a.png".to_owned()]),
            Err(IconValidationError::Format),
        );
        assert_eq!(
            resolve_item_plan(&all_dead, &item(None, Some("https://radio.example"))).await,
            ItemPlan::Permanent
        );
        let flaky = FakeFetcher::uniform(
            Ok(vec![
                "https://radio.example/a.png".to_owned(),
                "https://radio.example/favicon.ico".to_owned(),
            ]),
            Err(IconValidationError::Decode),
        );
        assert_eq!(
            resolve_item_plan(&flaky, &item(None, Some("https://radio.example"))).await,
            ItemPlan::Retryable
        );
    }

    #[tokio::test]
    async fn station_without_any_source_is_missing() {
        let fetcher = FakeFetcher::uniform(
            Ok(vec!["https://radio.example/favicon.ico".to_owned()]),
            Ok(sample_prepared()),
        );
        assert_eq!(
            resolve_item_plan(&fetcher, &item(None, None)).await,
            ItemPlan::Missing
        );
    }

    #[tokio::test]
    async fn transient_homepage_failures_are_retryable_and_dead_ones_permanent() {
        let transient =
            FakeFetcher::uniform(Err(IconValidationError::Decode), Ok(sample_prepared()));
        assert_eq!(
            resolve_item_plan(&transient, &item(None, Some("https://radio.example"))).await,
            ItemPlan::Retryable
        );
        let dead = FakeFetcher::uniform(Err(IconValidationError::Format), Ok(sample_prepared()));
        assert_eq!(
            resolve_item_plan(&dead, &item(None, Some("https://radio.example"))).await,
            ItemPlan::Permanent
        );
        let oversized = FakeFetcher::uniform(Err(IconValidationError::Size), Ok(sample_prepared()));
        assert_eq!(
            resolve_item_plan(&oversized, &item(None, Some("https://radio.example"))).await,
            ItemPlan::Permanent
        );
    }

    #[tokio::test]
    async fn icon_fetch_failures_keep_the_boundary_classification() {
        let retryable = FakeFetcher::uniform(
            Ok(vec!["https://radio.example/favicon.ico".to_owned()]),
            Err(IconValidationError::Decode),
        );
        assert_eq!(
            resolve_item_plan(&retryable, &item(None, Some("https://radio.example"))).await,
            ItemPlan::Retryable
        );
        let permanent = FakeFetcher::uniform(
            Ok(vec!["https://radio.example/favicon.ico".to_owned()]),
            Err(IconValidationError::Format),
        );
        assert_eq!(
            resolve_item_plan(&permanent, &item(None, Some("https://radio.example"))).await,
            ItemPlan::Permanent
        );
        let oversized = FakeFetcher::uniform(
            Ok(vec!["https://radio.example/favicon.ico".to_owned()]),
            Err(IconValidationError::Size),
        );
        assert_eq!(
            resolve_item_plan(&oversized, &item(None, Some("https://radio.example"))).await,
            ItemPlan::Permanent
        );
    }
}
