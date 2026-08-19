//! One-shot stream liveness probe that marks unreachable streams as degraded.
//!
//! Connects to each stream URL with a short timeout and updates the `health`
//! column in `station_streams`. Streams that respond with audio data are marked
//! `healthy`; those that time out or return errors are marked `degraded` with the
//! error recorded in `last_probe_error`.

use std::{env, error::Error, time::Duration};

use reqwest::header::ACCEPT;
use sqlx::postgres::PgPoolOptions;

use rockserver::{persistence, telemetry};

/// Maximum time to wait for the first audio bytes from a stream.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// How many streams to probe per batch from the database.
const BATCH_SIZE: i64 = 500;
/// Maximum concurrent probe connections.
const CONCURRENCY: usize = 50;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    telemetry::init()?;

    let database_url = env::var(persistence::DATABASE_URL_ENV)
        .map_err(|_| "DATABASE_URL is required for the stream probe")?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .danger_accept_invalid_certs(false)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;

    let urls = persistence::streams_to_probe(&pool, BATCH_SIZE).await?;
    tracing::info!(count = urls.len(), "streams to probe");

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let mut handles = Vec::with_capacity(urls.len());

    for url in urls {
        let client = client.clone();
        let pool = pool.clone();
        let permit = semaphore.clone().acquire_owned().await?;
        handles.push(tokio::spawn(async move {
            let result = probe_stream(&client, &url).await;
            let (healthy, error_msg) = match &result {
                Ok(()) => (true, None),
                Err(e) => {
                    let msg = e.to_string();
                    let truncated: String = msg.chars().take(500).collect();
                    (false, Some(truncated))
                }
            };
            if let Err(db_err) = persistence::update_stream_health(
                &pool,
                &url,
                healthy,
                error_msg.as_deref(),
            )
            .await
            {
                tracing::error!(stream_url = %url, error = %db_err, "failed to update stream health");
            } else {
                tracing::info!(stream_url = %url, healthy, "stream probed");
            }
            drop(permit);
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    pool.close().await;
    tracing::info!("stream probe completed");
    Ok(())
}

async fn probe_stream(client: &reqwest::Client, url: &str) -> Result<(), ProbeError> {
    let response = client
        .get(url)
        .header(ACCEPT, "*/*")
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                ProbeError::Timeout
            } else if e.is_connect() {
                ProbeError::ConnectionFailed(e.to_string())
            } else {
                ProbeError::RequestFailed(e.to_string())
            }
        })?;

    let status = response.status();
    if !status.is_success() && !status.is_redirection() {
        return Err(ProbeError::HttpError(status.as_u16()));
    }

    // Read a small chunk to confirm data is flowing.
    let mut body_bytes = 0usize;
    let mut stream = response;
    while let Some(chunk) = stream.chunk().await.map_err(|e| {
        if e.is_timeout() {
            ProbeError::Timeout
        } else {
            ProbeError::ReadFailed(e.to_string())
        }
    })? {
        body_bytes += chunk.len();
        if body_bytes > 0 {
            break; // Got audio data, stream is alive.
        }
    }

    if body_bytes == 0 {
        return Err(ProbeError::EmptyResponse);
    }

    Ok(())
}

#[derive(Debug)]
enum ProbeError {
    Timeout,
    ConnectionFailed(String),
    RequestFailed(String),
    HttpError(u16),
    ReadFailed(String),
    EmptyResponse,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "stream timed out"),
            Self::ConnectionFailed(e) => write!(f, "connection failed: {e}"),
            Self::RequestFailed(e) => write!(f, "request failed: {e}"),
            Self::HttpError(code) => write!(f, "HTTP {code}"),
            Self::ReadFailed(e) => write!(f, "read failed: {e}"),
            Self::EmptyResponse => write!(f, "empty response"),
        }
    }
}

impl std::error::Error for ProbeError {}
