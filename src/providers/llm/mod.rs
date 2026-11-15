//! gRPC client, adapters, and domain mappings for the neural LLM service (`llm.v1`).

pub mod client;
pub mod config;
pub mod error;
pub mod language_source;
pub mod mapping;
pub mod provider;
pub mod traits;

#[cfg(test)]
mod tests;

pub use client::LlmClient;
pub use config::{
    LLM_ADDR_ENV, LLM_COMMAND_TIMEOUT_MS_ENV, LLM_INFO_TIMEOUT_MS_ENV, LLM_INTENT_TIMEOUT_MS_ENV,
    LLM_TOKEN_ENV, LlmConfig,
};
pub use error::{LlmClientError, LlmFailure};
pub use language_source::LlmLanguageSource;
pub use provider::{LlmCommandInterpreter, LlmQueryParser};
pub use traits::LlmService;
