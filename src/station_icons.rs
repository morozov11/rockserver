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
    /// The source format is not one of the accepted raster formats.
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

/// Fetches one external icon only after validating the URL and every redirect destination.
#[derive(Clone, Debug)]
pub struct SafeIconFetcher {
    client: reqwest::Client,
}

impl SafeIconFetcher {
    /// Creates a fetcher with bounded request time and no implicit redirects.
    pub fn new() -> Result<Self, IconStorageError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(Policy::none())
            .build()
            .map_err(|_| IconStorageError::Unavailable)?;
        Ok(Self { client })
    }

    /// Downloads at most two MiB from a publicly routable HTTP(S) URL and normalizes it.
    pub async fn fetch(&self, source: &str) -> Result<PreparedIcon, IconValidationError> {
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
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|size| size as usize > MAX_SOURCE_BYTES)
            {
                return Err(IconValidationError::Size);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| IconValidationError::Decode)?
            {
                if body.len() + chunk.len() > MAX_SOURCE_BYTES {
                    return Err(IconValidationError::Size);
                }
                body.extend_from_slice(&chunk);
            }
            return prepare_icon(&body);
        }
        Err(IconValidationError::Format)
    }
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
    fetcher: SafeIconFetcher,
}

impl IconImportCoordinator {
    /// Reuses the production pool and prepared storage; it never performs work until `run` is called.
    pub fn new(pool: PgPool, storage: Arc<dyn IconStorage>) -> Result<Self, IconStorageError> {
        Ok(Self {
            pool,
            storage,
            fetcher: SafeIconFetcher::new()?,
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
            let outcome = match item.source_url {
                None => "missing",
                Some(source_url) => match self.fetcher.fetch(&source_url).await {
                    Ok(icon) => match self.publish(&item.station_id, icon).await {
                        Ok(()) => "ready",
                        Err(_) => "retryable_error",
                    },
                    Err(IconValidationError::Decode | IconValidationError::Size) => {
                        "retryable_error"
                    }
                    Err(_) => "permanent_error",
                },
            };
            let _ = self.finish_item(id, &item.station_id, outcome).await;
        }
        let _ = sqlx::query("UPDATE station_icon_jobs SET status = 'completed', finished_at = now() WHERE id = $1 AND status = 'running'")
            .bind(id).execute(&self.pool).await;
    }

    async fn claim_item(&self, id: Uuid) -> Result<Option<IconJobItem>, IconStorageError> {
        sqlx::query_as::<_, IconJobItem>("WITH next_item AS (SELECT station_id FROM station_icon_job_items WHERE job_id = $1 AND status = 'pending' ORDER BY station_id FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE station_icon_job_items item SET status = 'processing' FROM next_item WHERE item.job_id = $1 AND item.station_id = next_item.station_id RETURNING item.station_id, (SELECT source_url FROM station_icons WHERE station_id = item.station_id) AS source_url")
            .bind(id).fetch_optional(&self.pool).await.map_err(|_| IconStorageError::Unavailable)
    }

    async fn publish(&self, station_id: &str, icon: PreparedIcon) -> Result<(), IconStorageError> {
        let key = IconStorageKey::from_hash(&icon.content_hash);
        self.storage.put_atomic(&key, &icon.bytes).await?;
        sqlx::query("UPDATE station_icons SET storage_key = $2, content_type = 'image/webp', byte_size = $3, width = $4, height = $5, content_hash = $6, status = 'ready', refresh_needed = false, retry_after = NULL, last_error_code = NULL, updated_at = now() WHERE station_id = $1")
            .bind(station_id).bind(key.as_str()).bind(i32::try_from(icon.bytes.len()).map_err(|_| IconStorageError::Unavailable)?).bind(i32::try_from(icon.width).map_err(|_| IconStorageError::Unavailable)?).bind(i32::try_from(icon.height).map_err(|_| IconStorageError::Unavailable)?).bind(icon.content_hash.as_slice())
            .execute(&self.pool).await.map_err(|_| IconStorageError::Unavailable)?;
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

    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

    use super::{
        FilesystemIconStorage, IconStorage, IconStorageKey, IconValidationError, prepare_icon,
    };

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
}
