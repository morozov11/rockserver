//! Adapters exposing the neural LLM client as `QueryParser` and `CommandInterpreter`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::search::taxonomy::GenreTaxonomy;
use crate::search::{
    DeterministicQueryParser, QueryIntent, QueryParser, QueryParserError, QueryParserInput,
    tokenize,
};
use crate::voice::command::DeterministicCommandInterpreter;
use crate::voice::{CommandInterpretationError, CommandInterpreter, Intent, VoiceCommand};

use super::traits::LlmService;

const MAX_COMMAND_TEXT_CHARS: usize = 2_000;

/// `QueryParser` backed by the neural LLM service with full deterministic fallback.
#[derive(Clone)]
pub struct LlmQueryParser {
    service: Option<Arc<dyn LlmService>>,
}

impl LlmQueryParser {
    /// Creates a query parser backed by the supplied LLM service.
    pub fn new(service: Arc<dyn LlmService>) -> Self {
        Self {
            service: Some(service),
        }
    }

    /// Creates a query parser in fallback-only mode (no LLM service configured).
    pub fn disabled() -> Self {
        Self { service: None }
    }
}

#[async_trait]
impl QueryParser for LlmQueryParser {
    async fn parse(&self, input: &QueryParserInput) -> Result<QueryIntent, QueryParserError> {
        self.parse_with_taxonomy(input, &GenreTaxonomy::builtin())
            .await
    }

    async fn parse_with_taxonomy(
        &self,
        input: &QueryParserInput,
        taxonomy: &GenreTaxonomy,
    ) -> Result<QueryIntent, QueryParserError> {
        let Some(service) = &self.service else {
            return DeterministicQueryParser
                .parse_with_taxonomy(input, taxonomy)
                .await;
        };

        match service
            .parse_intent(&input.query, &input.locale, taxonomy)
            .await
        {
            Ok(intent) => Ok(intent),
            Err(err) => {
                tracing::warn!(
                    %err,
                    "LLM query parser failed; degrading to deterministic metadata fallback"
                );
                DeterministicQueryParser
                    .parse_with_taxonomy(input, taxonomy)
                    .await
            }
        }
    }
}

/// `CommandInterpreter` backed by the neural LLM service with full deterministic fallback.
#[derive(Clone)]
pub struct LlmCommandInterpreter {
    service: Option<Arc<dyn LlmService>>,
}

impl LlmCommandInterpreter {
    /// Creates an interpreter backed by the supplied LLM service.
    pub fn new(service: Arc<dyn LlmService>) -> Self {
        Self {
            service: Some(service),
        }
    }

    /// Creates an interpreter in fallback-only mode (no LLM service configured).
    pub fn disabled() -> Self {
        Self { service: None }
    }
}

#[async_trait]
impl CommandInterpreter for LlmCommandInterpreter {
    async fn interpret(
        &self,
        transcript: &str,
        locale: &str,
    ) -> Result<VoiceCommand, CommandInterpretationError> {
        validate_command_input(transcript, locale)?;

        let Some(service) = &self.service else {
            return DeterministicCommandInterpreter
                .interpret(transcript, locale)
                .await;
        };

        let tokens = tokenize(transcript);
        match service.parse_command(transcript, locale).await {
            Ok(command)
                if command.intent == Intent::Unknown
                    && tokens.iter().any(|t| {
                        crate::languages::bundled_registry().is_basic_command_token(t)
                    }) =>
            {
                // Rescue unknown verdicts for recognized basic commands.
                let rescued = DeterministicCommandInterpreter
                    .interpret(transcript, locale)
                    .await
                    .ok()
                    .filter(|cmd| cmd.intent != Intent::Unknown);
                match rescued {
                    Some(cmd) => {
                        tracing::warn!(
                            "LLM command interpreter returned unknown; rescued local basic command"
                        );
                        Ok(cmd)
                    }
                    None => Ok(command),
                }
            }
            Ok(command) => Ok(command),
            Err(err) => {
                tracing::warn!(
                    %err,
                    "LLM voice command interpreter failed; degrading to deterministic fallback"
                );
                DeterministicCommandInterpreter
                    .interpret(transcript, locale)
                    .await
            }
        }
    }
}

fn validate_command_input(
    transcript: &str,
    locale: &str,
) -> Result<(), CommandInterpretationError> {
    if transcript.trim().is_empty() || transcript.chars().count() > MAX_COMMAND_TEXT_CHARS {
        return Err(CommandInterpretationError::safe(
            "voice transcript must be non-empty and bounded",
        ));
    }
    if locale.trim().is_empty() || locale.len() > 32 {
        return Err(CommandInterpretationError::safe("voice locale is invalid"));
    }
    Ok(())
}
