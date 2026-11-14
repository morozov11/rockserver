//! gRPC client of the embedding service and its table lifecycle.
//!
//! Layering: `config` reads environment settings, `client` speaks `embed.v1`
//! (deadlines, bearer auth, one shared breaker), `error` maps statuses per the
//! service contract, `provider` exposes the switchable search adapter,
//! `backfill` fills one versioned table, and `lifecycle` runs the startup
//! sequence (`Info` → advisory lock → `ensure_table` → enable → fill).

mod backfill;
mod client;
mod config;
mod error;
mod lifecycle;
mod provider;
mod traits;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub use backfill::{VersionBackfillError, VersionBackfillStats, fill_version};
pub use client::{EMBED_API_VERSION, EmbedderClient};
pub use config::{
    EMBEDDER_ADDR_ENV, EMBEDDER_BATCH_TIMEOUT_MS_ENV, EMBEDDER_INFO_TIMEOUT_MS_ENV,
    EMBEDDER_QUERY_TIMEOUT_MS_ENV, EMBEDDER_TOKEN_ENV, EmbedderConfig,
};
pub use lifecycle::EmbedderLifecycle;
pub use provider::{EmbedderGateway, ServiceEmbeddingProvider};
pub use traits::EmbedderService;
