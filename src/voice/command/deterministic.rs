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
