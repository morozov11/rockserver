//! Provider-neutral structured voice-command interpretation.

use std::{error::Error, fmt};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use crate::providers::llm::LlmCommandInterpreter;

const MAX_COMMAND_TEXT_CHARS: usize = 2_000;
const MAX_QUERY_VALUES: usize = 32;
const MAX_QUERY_VALUE_CHARS: usize = 64;

/// A typed command produced from recognized speech.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceCommand {
    /// The action requested by the speaker.
    pub intent: Intent,
    /// Radio-search constraints, present only for `play_radio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<RadioQuery>,
    /// Signed percentage-point volume change, present only for `volume_change`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_delta: Option<i8>,
    /// Absolute output level, present only for `set_volume`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_level: Option<u8>,
}

/// Supported voice-control actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// Find and start a radio station.
    PlayRadio,
    /// Stop playback.
    Stop,
    /// Resume paused playback or restart the last played station.
    Resume,
    /// Select the next station.
    NextStation,
    /// Select the previous station.
    PreviousStation,
    /// Change playback volume by `volume_delta` percentage points.
    VolumeChange,
    /// Set playback volume to `volume_level` percent.
    SetVolume,
    /// Add the currently playing station to the account favourites.
    AddFavorite,
    /// The utterance cannot be mapped safely to a supported action.
    Unknown,
}

/// Provider-neutral radio-search filters extracted from a voice command.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioQuery {
    /// Normalized genres requested by the speaker.
    #[serde(default)]
    pub genres: Vec<String>,
    /// Normalized moods requested by the speaker.
    #[serde(default)]
    pub moods: Vec<String>,
    /// Artist used as a similarity hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similar_to: Option<String>,
    /// Optional ISO 639 language code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Optional ISO 3166-1 alpha-2 country code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
}

/// Safe command-interpretation failure suitable for logs and API mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandInterpretationError {
    message: String,
}

impl CommandInterpretationError {
    /// Creates a safe interpretation error with a public message.
    pub fn safe(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CommandInterpretationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CommandInterpretationError {}

/// Replaceable boundary for translating recognized text into a typed command.
#[async_trait]
pub trait CommandInterpreter: Send + Sync {
    /// Interprets one non-empty STT transcript using the supplied locale.
    async fn interpret(
        &self,
        transcript: &str,
        locale: &str,
    ) -> Result<VoiceCommand, CommandInterpretationError>;
}

#[path = "command/deterministic.rs"]
mod deterministic;

pub use deterministic::DeterministicCommandInterpreter;

fn control_command(intent: Intent) -> VoiceCommand {
    VoiceCommand {
        intent,
        query: None,
        volume_delta: None,
        volume_level: None,
    }
}

fn validate_input(transcript: &str, locale: &str) -> Result<(), CommandInterpretationError> {
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

impl VoiceCommand {
    /// Validates field constraints across intent variants.
    pub fn validate(mut self) -> Result<Self, CommandInterpretationError> {
        match self.intent {
            Intent::PlayRadio => {
                let query = self
                    .query
                    .as_mut()
                    .ok_or_else(|| CommandInterpretationError::safe("play_radio requires query"))?;
                normalize_query(query)?;
                if query.genres.is_empty()
                    && query.moods.is_empty()
                    && query.similar_to.is_none()
                    && query.language.is_none()
                    && query.country_code.is_none()
                {
                    return Err(CommandInterpretationError::safe(
                        "play_radio requires at least one search criterion",
                    ));
                }
                if self.volume_delta.is_some() || self.volume_level.is_some() {
                    return Err(CommandInterpretationError::safe(
                        "play_radio must not include volume_delta",
                    ));
                }
            }
            Intent::VolumeChange
                if self.query.is_some()
                    || self.volume_delta.is_none_or(|delta| delta == 0)
                    || self.volume_level.is_some() =>
            {
                return Err(CommandInterpretationError::safe(
                    "volume_change requires a non-zero volume_delta and no query",
                ));
            }
            Intent::SetVolume
                if self.query.is_some()
                    || self.volume_delta.is_some()
                    || self.volume_level.is_none_or(|level| level > 100) =>
            {
                return Err(CommandInterpretationError::safe(
                    "set_volume requires an absolute volume_level and no query",
                ));
            }
            _ if self.query.is_some()
                || self.volume_delta.is_some()
                || self.volume_level.is_some() =>
            {
                return Err(CommandInterpretationError::safe(
                    "control command must not include query or volume_delta",
                ));
            }
            _ => {}
        }
        Ok(self)
    }
}

fn normalize_query(query: &mut RadioQuery) -> Result<(), CommandInterpretationError> {
    for values in [&mut query.genres, &mut query.moods] {
        if values.len() > MAX_QUERY_VALUES
            || values
                .iter()
                .any(|value| value.chars().count() > MAX_QUERY_VALUE_CHARS)
        {
            return Err(CommandInterpretationError::safe(
                "voice command contains an invalid search value",
            ));
        }
        *values = std::mem::take(values)
            .into_iter()
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        values.sort();
        values.dedup();
    }
    query.similar_to = normalize_optional(query.similar_to.take());
    query.language =
        normalize_optional(query.language.take()).map(|value| value.to_ascii_lowercase());
    if query.language.as_deref().is_some_and(|value| {
        !(2..=3).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_lowercase())
    }) {
        return Err(CommandInterpretationError::safe(
            "voice command contains an invalid language",
        ));
    }
    query.country_code =
        normalize_optional(query.country_code.take()).map(|value| value.to_ascii_uppercase());
    if query.country_code.as_deref().is_some_and(|value| {
        value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_uppercase())
    }) {
        return Err(CommandInterpretationError::safe(
            "voice command contains an invalid country code",
        ));
    }
    Ok(())
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod fallback_tests;

#[cfg(test)]
#[path = "command/tests.rs"]
mod tests;
