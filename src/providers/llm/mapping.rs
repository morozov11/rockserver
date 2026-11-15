//! Mapping of LLM service responses to domain queries, intents, and voice commands.

use std::collections::BTreeSet;

use crate::grpc::llm::v1::{
    Action, CountryHint, CountryHintKind, LanguageHint, LanguageHintKind, ParseCommandResponse,
    ParseIntentResponse, VoiceIntent,
};
use crate::languages::LanguageRegistry;
use crate::search::taxonomy::GenreTaxonomy;
use crate::search::{QueryIntent, SearchAction, tokenize};
use crate::voice::{Intent, RadioQuery, VoiceCommand};

use super::error::LlmClientError;

const MAX_INTENT_VALUES: usize = 32;
const MAX_INTENT_VALUE_CHARS: usize = 64;

/// Maps a raw `ParseIntentResponse` into a domain `QueryIntent`, validating tags and hints.
pub fn map_intent_response(
    response: ParseIntentResponse,
    query: &str,
    taxonomy: &GenreTaxonomy,
) -> Result<QueryIntent, LlmClientError> {
    let action = match Action::try_from(response.action).unwrap_or(Action::Unspecified) {
        Action::Play | Action::Unspecified => SearchAction::Play,
        Action::Show => SearchAction::Show,
    };

    let mut terms = Vec::new();
    for term in response.terms.into_iter().take(MAX_INTENT_VALUES) {
        let trimmed = term.trim();
        if !trimmed.is_empty() && trimmed.chars().count() <= MAX_INTENT_VALUE_CHARS {
            terms.push(trimmed.to_owned());
        }
    }

    let mut tags = Vec::new();
    for tag in response.tags.into_iter().take(MAX_INTENT_VALUES) {
        let trimmed = tag.trim().to_lowercase();
        if !trimmed.is_empty()
            && trimmed.chars().count() <= MAX_INTENT_VALUE_CHARS
            && taxonomy.contains_tag(&trimmed)
        {
            tags.push(trimmed);
        }
    }
    tags.sort();
    tags.dedup();

    let registry = crate::languages::bundled_registry();
    let language = validate_station_language(&response.station_language, query, registry);
    let country_code = validate_station_country(&response.station_country, query, registry);

    let raw_query = terms.join(" ");
    let core_term_count = terms.len();

    Ok(QueryIntent {
        action,
        terms,
        tags,
        language,
        country_code,
        core_term_count,
        raw_query,
    })
}

/// Validates the model's `station_language` hint against strict client rules.
///
/// Rules:
/// - Kind must be `EXPLICIT_LANGUAGE` (nationality adjectives are vetoed).
/// - Evidence must be a non-empty, non-whole-command quote appearing in the query.
/// - The quoted tokens must map to the declared `code` in the language registry.
/// - The request must not contain negation near the language marker.
/// - The request must not mention multiple conflicting languages.
pub fn validate_station_language(
    hint: &Option<LanguageHint>,
    query: &str,
    registry: &LanguageRegistry,
) -> Option<String> {
    let hint = hint.as_ref()?;
    if hint.kind != LanguageHintKind::LanguageExplicitLanguage as i32 {
        return None;
    }
    let code = hint.code.as_deref()?.trim().to_ascii_lowercase();
    if !(2..=3).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let evidence = hint.evidence.as_deref()?.trim();
    if evidence.is_empty() || evidence.eq_ignore_ascii_case(query.trim()) {
        return None;
    }

    let query_tokens = tokenize(query);
    let evidence_tokens = tokenize(evidence);
    if evidence_tokens.is_empty() {
        return None;
    }

    // Evidence must appear in the query tokens.
    let ev_start = find_subsequence(&query_tokens, &evidence_tokens)?;

    // Check for negations within 2 tokens before the evidence.
    if has_preceding_negation(&query_tokens, ev_start, registry) {
        return None;
    }

    // Check that evidence actually identifies this language code in our registry.
    if !evidence_matches_language(&evidence_tokens, &code, registry) {
        return None;
    }

    // Check that query does not mention multiple distinct languages.
    if count_distinct_languages(&query_tokens, registry) > 1 {
        return None;
    }

    Some(code)
}

/// Validates the model's `station_country` hint against strict client rules.
///
/// Rules:
/// - Kind must be `COUNTRY_NAME` (demonyms/nationality adjectives are vetoed).
/// - Evidence must be a non-empty, non-whole-command quote appearing in the query.
/// - Must have an explicit country request preposition ("из", "from").
/// - Quoted tokens must map to the declared country `code` in the registry (CLDR).
/// - The request must not contain negation near the country preposition/name.
/// - The request must not mention multiple distinct countries.
pub fn validate_station_country(
    hint: &Option<CountryHint>,
    query: &str,
    registry: &LanguageRegistry,
) -> Option<String> {
    let hint = hint.as_ref()?;
    if hint.kind != CountryHintKind::CountryCountryName as i32 {
        return None;
    }
    let code = hint.code.as_deref()?.trim().to_ascii_uppercase();
    if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let evidence = hint.evidence.as_deref()?.trim();
    if evidence.is_empty() || evidence.eq_ignore_ascii_case(query.trim()) {
        return None;
    }

    let query_tokens = tokenize(query);
    let evidence_tokens = tokenize(evidence);
    if evidence_tokens.is_empty() {
        return None;
    }

    let ev_start = find_subsequence(&query_tokens, &evidence_tokens)?;

    // Must have an explicit country preposition preceding the evidence.
    let prep_idx = find_preceding_country_preposition(&query_tokens, ev_start, registry)?;

    // Check for negations within 2 tokens before the preposition.
    if has_preceding_negation(&query_tokens, prep_idx, registry) {
        return None;
    }

    // Check that evidence actually identifies this country code in our registry.
    if !evidence_matches_country(&evidence_tokens, &code, registry) {
        return None;
    }

    // Check that query does not mention multiple distinct countries.
    if count_distinct_countries(&query_tokens, registry) > 1 {
        return None;
    }

    Some(code)
}

/// Maps a raw `ParseCommandResponse` into a domain `VoiceCommand`, validating invariants.
pub fn map_command_response(
    response: ParseCommandResponse,
) -> Result<VoiceCommand, LlmClientError> {
    let intent = match VoiceIntent::try_from(response.intent).unwrap_or(VoiceIntent::Unspecified) {
        VoiceIntent::PlayRadio => Intent::PlayRadio,
        VoiceIntent::Stop => Intent::Stop,
        VoiceIntent::Resume => Intent::Resume,
        VoiceIntent::NextStation => Intent::NextStation,
        VoiceIntent::PreviousStation => Intent::PreviousStation,
        VoiceIntent::VolumeChange => Intent::VolumeChange,
        VoiceIntent::SetVolume => Intent::SetVolume,
        VoiceIntent::AddFavorite => Intent::AddFavorite,
        VoiceIntent::Unknown | VoiceIntent::Unspecified => Intent::Unknown,
    };

    let query = response.query.map(|q| RadioQuery {
        genres: q.genres,
        moods: q.moods,
        similar_to: q.similar_to,
        language: q.language,
        country_code: q.country_code,
    });

    let volume_delta = response.volume_delta.map(|d| d as i8);
    let volume_level = response.volume_level.map(|l| l as u8);

    let command = VoiceCommand {
        intent,
        query,
        volume_delta,
        volume_level,
    };

    command
        .validate()
        .map_err(|err| LlmClientError::safe(format!("invalid model output: {err}")))
}

fn find_subsequence(haystack: &[String], needle: &[String]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_preceding_country_preposition(
    tokens: &[String],
    before_index: usize,
    registry: &LanguageRegistry,
) -> Option<usize> {
    let mut prepositions = BTreeSet::new();
    for code in registry.valid_codes() {
        if let Some(module) = registry.get(&code) {
            prepositions.extend(module.lexical.countries.prepositions.iter().cloned());
        }
    }
    // Preposition can be directly before or with at most 1 filler/modifier in between.
    let search_start = before_index.saturating_sub(2);
    (search_start..before_index)
        .rev()
        .find(|&idx| prepositions.contains(tokens[idx].as_str()))
}

fn has_preceding_negation(
    tokens: &[String],
    before_index: usize,
    registry: &LanguageRegistry,
) -> bool {
    let mut negations = BTreeSet::new();
    for code in registry.valid_codes() {
        if let Some(module) = registry.get(&code) {
            negations.extend(module.lexical.negations.iter().cloned());
        }
    }
    let search_start = before_index.saturating_sub(2);
    (search_start..before_index).any(|idx| negations.contains(tokens[idx].as_str()))
}

fn evidence_matches_language(
    evidence_tokens: &[String],
    expected_code: &str,
    registry: &LanguageRegistry,
) -> bool {
    for code in registry.valid_codes() {
        if let Some(module) = registry.get(&code) {
            let el = &module.lexical.explicit_languages;
            for lang in &el.languages {
                if lang.code == expected_code
                    && lang
                        .forms
                        .iter()
                        .any(|f| evidence_tokens.iter().any(|t| t == f))
                {
                    return true;
                }
            }
            for phrase in &el.phrases {
                if phrase.code == expected_code && phrase.phrase == evidence_tokens {
                    return true;
                }
            }
            for standalone in &el.standalone {
                if standalone.code == expected_code
                    && standalone
                        .terms
                        .iter()
                        .any(|term| evidence_tokens.iter().any(|t| t == term))
                {
                    return true;
                }
            }
        }
    }
    false
}

fn count_distinct_languages(tokens: &[String], registry: &LanguageRegistry) -> usize {
    let mut matched = BTreeSet::new();
    for code in registry.valid_codes() {
        if let Some(module) = registry.get(&code) {
            let el = &module.lexical.explicit_languages;
            for token in tokens {
                for lang in &el.languages {
                    if lang.forms.iter().any(|f| f == token) {
                        matched.insert(lang.code.clone());
                    }
                }
            }
        }
    }
    matched.len()
}

fn country_token_matches(name_token: &str, candidate_token: &str) -> bool {
    if name_token == candidate_token {
        return true;
    }
    // Russian regular genitive for country names ending in -ия -> -ии (e.g. франция -> франции).
    if name_token.ends_with("ия")
        && candidate_token.ends_with("ии")
        && name_token[..name_token.len() - 4] == candidate_token[..candidate_token.len() - 4]
    {
        return true;
    }
    false
}

fn country_name_matches(name_tokens: &[String], candidate_tokens: &[String]) -> bool {
    if name_tokens.len() != candidate_tokens.len() {
        return false;
    }
    name_tokens
        .iter()
        .zip(candidate_tokens.iter())
        .all(|(n, c)| country_token_matches(n, c))
}

fn find_country_subsequence(haystack: &[String], needle: &[String]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(needle.iter())
            .all(|(h, n)| country_token_matches(n, h))
    })
}

fn evidence_matches_country(
    evidence_tokens: &[String],
    expected_code: &str,
    registry: &LanguageRegistry,
) -> bool {
    for code in registry.valid_codes() {
        if let Some(module) = registry.get(&code) {
            for country in &module.lexical.countries.country_names {
                if country.code == expected_code {
                    for name in &country.names {
                        let name_tokens = tokenize(name);
                        if country_name_matches(&name_tokens, evidence_tokens) {
                            return true;
                        }
                    }
                }
            }
            for inf in &module.lexical.countries.inflections {
                if inf.code == expected_code
                    && inf
                        .forms
                        .iter()
                        .any(|f| evidence_tokens.iter().any(|t| t == f))
                {
                    return true;
                }
            }
        }
    }
    false
}

fn count_distinct_countries(tokens: &[String], registry: &LanguageRegistry) -> usize {
    let mut matched = BTreeSet::new();
    for code in registry.valid_codes() {
        if let Some(module) = registry.get(&code) {
            for country in &module.lexical.countries.country_names {
                for name in &country.names {
                    let name_tokens = tokenize(name);
                    if find_country_subsequence(tokens, &name_tokens).is_some() {
                        matched.insert(country.code.clone());
                    }
                }
            }
            for inf in &module.lexical.countries.inflections {
                for form in &inf.forms {
                    if tokens.iter().any(|t| t == form) {
                        matched.insert(inf.code.clone());
                    }
                }
            }
        }
    }
    matched.len()
}
