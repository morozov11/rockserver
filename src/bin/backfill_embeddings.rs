//! Manual backfill of one versioned station-embedding table.

use std::{env, error::Error, process::ExitCode};

use rockserver::{
    persistence::{
        DATABASE_URL_ENV, EMBEDDINGS_DIMENSION, EMBEDDINGS_MODEL, EMBEDDINGS_VERSION,
        EmbeddingVersionStore, MAX_BATCH_STATIONS,
    },
    providers::embedder::{
        EMBEDDER_ADDR_ENV, EmbedderClient, EmbedderConfig, EmbedderService, fill_version,
    },
    telemetry,
};

/// Parses `--version N` from the command line.
fn requested_version() -> Result<u32, Box<dyn Error + Send + Sync>> {
    let mut version = None;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--version" {
            let Some(value) = arguments.next() else {
                return Err("--version requires a numeric argument".into());
            };
            version = Some(value.parse::<u32>().map_err(|_| {
                Box::<dyn Error + Send + Sync>::from("--version expects a positive integer")
            })?);
        } else {
            return Err(format!("unknown argument: {argument}").into());
        }
    }
    version.filter(|value| *value >= 1).ok_or_else(|| {
        Box::<dyn Error + Send + Sync>::from("--version N is required (use the EMBEDDINGS_VERSION constant of the release that will read this table)")
    })
}

fn required_env(name: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required for embedding backfill").into())
}

#[tokio::main]
async fn main() -> ExitCode {
    telemetry::init().expect("telemetry must initialize");
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("embedding backfill failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let version = requested_version()?;
    let database_url = required_env(DATABASE_URL_ENV)?;
    let embedder_config = EmbedderConfig::from_env()?
        .ok_or_else(|| format!("{EMBEDDER_ADDR_ENV} is required for embedding backfill"))?;

    let service = EmbedderClient::lazy(embedder_config)?;
    let provenance = service.info().await?;
    if provenance.model != EMBEDDINGS_MODEL {
        return Err(format!(
            "embedding service model {} does not match the expected {EMBEDDINGS_MODEL}",
            provenance.model
        )
        .into());
    }
    // Only the released version has a known dimension in this build; staging a
    // future table requires deploying the release that defines it first.
    let dimension = if version == EMBEDDINGS_VERSION {
        EMBEDDINGS_DIMENSION
    } else {
        provenance.dimension
    };
    tracing::info!(
        version,
        model = %provenance.model,
        vector_version = %provenance.version,
        dimension,
        "starting embedding backfill"
    );

    let mut store = EmbeddingVersionStore::connect(&database_url).await?;
    store.acquire_advisory_lock(version).await?;
    store.ensure_table(version, dimension).await?;
    let started = std::time::Instant::now();
    let stats = fill_version(&service, &mut store, version, MAX_BATCH_STATIONS).await;
    let (active, covered) = store.coverage(version).await.unwrap_or((-1, -1));
    drop(store);
    let stats = stats?;
    tracing::info!(
        version,
        active,
        covered,
        processed = stats.processed,
        written = stats.written,
        elapsed_secs = started.elapsed().as_secs_f64(),
        "embedding backfill completed"
    );
    Ok(())
}
