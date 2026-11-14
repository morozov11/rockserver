//! External catalog providers isolated from HTTP search and persistence.

/// gRPC client of the embedding service and its table lifecycle (`embed.v1`).
pub mod embedder;
/// Radio Browser HTTP client and deterministic DTO normalization.
pub mod radio_browser;
/// Read-only Yandex Smart Home OAuth and sensor adapter.
pub mod yandex_home;
/// Yandex AI Studio adapter for structured radio-intent generation.
pub mod yandex_llm;
/// Local-configuration Yandex SpeechKit adapter for voice streaming.
pub mod yandex_speechkit;
pub mod yandex_speechkit_streaming;
mod yandex_speechkit_v3;
