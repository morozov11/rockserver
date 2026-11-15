//! Deterministic rule-based voice command interpreter backed by language modules.

use async_trait::async_trait;

use super::{
    CommandInterpretationError, CommandInterpreter, Intent, RadioQuery, VoiceCommand,
    control_command, validate_input,
};

/// Conservative local classifier used when no structured-output interpreter is configured.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicCommandInterpreter;

#[async_trait]
impl CommandInterpreter for DeterministicCommandInterpreter {
    async fn interpret(
        &self,
        transcript: &str,
        locale: &str,
    ) -> Result<VoiceCommand, CommandInterpretationError> {
        validate_input(transcript, locale)?;
        let tokens = crate::search::tokenize(transcript);
        let registry = crate::languages::bundled_registry();
        let lang = crate::languages::extract_base_language(locale);
        let module = registry.get(&lang).unwrap_or_else(|| registry.en());
        let en_module = registry.en();

        if tokens.iter().any(|t| {
            module.commands.is_unsupported(t)
                || (lang != "en" && en_module.commands.is_unsupported(t))
        }) {
            return Ok(control_command(Intent::Unknown));
        }

        if tokens.iter().any(|t| {
            module.commands.is_set_volume(t)
                || (lang != "en" && en_module.commands.is_set_volume(t))
        }) {
            return Ok(tokens
                .iter()
                .find_map(|token| token.parse::<u8>().ok().filter(|level| *level <= 100))
                .map(|level| VoiceCommand {
                    intent: Intent::SetVolume,
                    query: None,
                    volume_delta: None,
                    volume_level: Some(level),
                })
                .unwrap_or_else(|| control_command(Intent::Unknown)));
        }

        if tokens
            .iter()
            .any(|t| module.commands.is_stop(t) || (lang != "en" && en_module.commands.is_stop(t)))
        {
            return Ok(control_command(Intent::Stop));
        }

        if tokens.iter().any(|t| {
            module.commands.is_resume(t) || (lang != "en" && en_module.commands.is_resume(t))
        }) {
            return Ok(control_command(Intent::Resume));
        }

        if tokens.iter().any(|t| {
            module.commands.is_add_favorite(t)
                || (lang != "en" && en_module.commands.is_add_favorite(t))
        }) {
            return Ok(control_command(Intent::AddFavorite));
        }

        if tokens.iter().any(|t| {
            module.commands.is_volume_up(t) || (lang != "en" && en_module.commands.is_volume_up(t))
        }) {
            return Ok(VoiceCommand {
                intent: Intent::VolumeChange,
                query: None,
                volume_delta: Some(5),
                volume_level: None,
            });
        }

        if tokens.iter().any(|t| {
            module.commands.is_volume_down(t)
                || (lang != "en" && en_module.commands.is_volume_down(t))
        }) {
            return Ok(VoiceCommand {
                intent: Intent::VolumeChange,
                query: None,
                volume_delta: Some(-5),
                volume_level: None,
            });
        }

        let genre = if tokens.iter().any(|t| {
            module.commands.matches_genre(t, "rock")
                || (lang != "en" && en_module.commands.matches_genre(t, "rock"))
        }) {
            "rock"
        } else if tokens.iter().any(|t| {
            module.commands.matches_genre(t, "jazz")
                || (lang != "en" && en_module.commands.matches_genre(t, "jazz"))
        }) {
            "jazz"
        } else if tokens.iter().any(|t| {
            module.commands.matches_genre(t, "metal")
                || (lang != "en" && en_module.commands.matches_genre(t, "metal"))
        }) {
            "metal"
        } else {
            "radio"
        };

        VoiceCommand {
            intent: Intent::PlayRadio,
            query: Some(RadioQuery {
                genres: vec![genre.to_owned()],
                ..RadioQuery::default()
            }),
            volume_delta: None,
            volume_level: None,
        }
        .validate()
    }
}

/// Tokens that signal one of the locally verified basic commands and allow the
/// deterministic classifier to rescue an `unknown` verdict or a provider
/// failure.
pub(super) fn basic_command_candidate(tokens: &[String]) -> bool {
    let registry = crate::languages::bundled_registry();
    tokens
        .iter()
        .any(|token| registry.is_basic_command_token(token))
}

/// Checks whether tokens represent a play metal command across stop words and genre vocabulary.
pub(super) fn is_play_metal_command(tokens: &[String]) -> bool {
    let registry = crate::languages::bundled_registry();
    let has_play_verb = tokens.iter().any(|token| {
        registry
            .valid_codes()
            .iter()
            .filter_map(|code| registry.get(code))
            .any(|module| module.lexical.stop_words.iter().any(|sw| sw == token))
    });
    let has_metal = tokens.iter().any(|token| {
        registry
            .valid_codes()
            .iter()
            .filter_map(|code| registry.get(code))
            .any(|module| module.commands.matches_genre(token, "metal"))
    });
    has_play_verb && has_metal
}
