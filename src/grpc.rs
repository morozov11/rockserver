//! Generated tonic/prost types for the neural-service gRPC contract.
//!
//! The schemas are vendored in `proto/` (source commit and hashes in
//! `proto/README.md`) and compiled by `build.rs`. This module is a facade that
//! only exposes the generated packages; contract behavior lives in the service
//! repository, clients live in `providers`.

/// Embedding service contract (`embed.v1`): `Info`, `EmbedQuery`, `EmbedStations`.
pub mod embed {
    /// Version 1 of the embedding service contract.
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/embed.v1.rs"));
    }
}

/// Reduced `google.rpc` rich-error schema used to decode `ErrorInfo` reasons.
pub mod rpc {
    /// `google.rpc` messages (`Status`, `ErrorInfo`).
    pub mod error_details {
        include!(concat!(env!("OUT_DIR"), "/google.rpc.rs"));
    }
}
