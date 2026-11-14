//! Versioned station-embedding storage (`station_embeddings_v{N}`).
//!
//! One table per embedding contract version holds vectors for exactly the
//! station fields they were computed from, keyed by `stations.fields_hash`.
//! The table is created and verified by [`EmbeddingVersionStore::ensure_table`]
//! (also from migration `0030`); migrations never insert rows.

mod fields_hash;
mod store;

#[cfg(test)]
mod tests;

pub use fields_hash::station_fields_hash;
pub use store::{EmbeddingVersionStore, PendingStationEmbedding, WrittenStationEmbedding};

/// Embedding contract version served by the `embed.v1` gRPC client (E2).
pub const EMBEDDINGS_VERSION: u32 = 1;

/// Model identity expected from the embedding service for table version 1.
pub const EMBEDDINGS_MODEL: &str = "intfloat/multilingual-e5-small";

/// Vector dimension of table version 1 (`vector(384)` column, E5-small).
pub const EMBEDDINGS_DIMENSION: usize = 384;

/// Maximum stations per `EmbedStations` batch (service contract, `embed.v1`).
pub const MAX_BATCH_STATIONS: usize = 64;

/// Physical table name for one embedding contract version.
///
/// The search SQL pins this name for [`EMBEDDINGS_VERSION`]; filling a future
/// version before a release uses the same helper with the new number.
pub fn embeddings_table_name(version: u32) -> String {
    format!("station_embeddings_v{version}")
}
