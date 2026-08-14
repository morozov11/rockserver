//! External catalog providers isolated from HTTP search and persistence.

/// Explicit development-only deterministic embedding provider.
pub mod deterministic_embedding;
/// Radio Browser HTTP client and deterministic DTO normalization.
pub mod radio_browser;
