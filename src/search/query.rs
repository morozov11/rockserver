//! Provider-neutral query interpretation and deterministic metadata fallback.

use std::{collections::BTreeSet, error::Error, fmt};

use async_trait::async_trait;

use super::{
    SearchQuery,
    language::explicit_language_code,
    taxonomy::{GenreTaxonomy, canonical_tags},
};

/// Request-only input supplied to a query parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryParserInput {
    /// Validated natural-language request text.
    pub query: String,
    /// Validated locale used to interpret the request.
    pub locale: String,
}

/// Structured intent returned by a query parser before repository search.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryIntent {
    /// Whether the caller should immediately play or only display matches.
    pub action: SearchAction,
    /// Lowercase terms that participate in metadata matching.
    pub terms: Vec<String>,
    /// Normalized catalog tags inferred from the request.
    pub tags: Vec<String>,
    /// Optional ISO 639 language hard filter.
    pub language: Option<String>,
    /// Optional ISO 3166-1 alpha-2 country hard filter.
    pub country_code: Option<String>,
    /// Number of core terms before transliteration expansion, used as the
    /// score denominator so alias terms don't dilute match quality.
    pub core_term_count: usize,
    /// Cleaned query string (stop-words removed) for full-text search.
    pub raw_query: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchAction {
    #[default]
    Play,
    Show,
}

/// Safe query-parser failure that can fall back to deterministic interpretation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryParserError {
    summary: String,
}

impl QueryParserError {
    /// Creates a provider-safe failure summary for logs.
    pub fn safe(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }
}

impl fmt::Display for QueryParserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.summary)
    }
}

impl Error for QueryParserError {}

/// Boundary for translating request text into structured search intent.
///
/// Implementations receive only request data. They never receive stations or catalog snapshots.
#[async_trait]
pub trait QueryParser: Send + Sync {
    /// Parses one validated request into provider-neutral intent.
    async fn parse(&self, input: &QueryParserInput) -> Result<QueryIntent, QueryParserError>;

    /// Parses against the controlled vocabulary selected for this request.
    async fn parse_with_taxonomy(
        &self,
        input: &QueryParserInput,
        _taxonomy: &GenreTaxonomy,
    ) -> Result<QueryIntent, QueryParserError> {
        self.parse(input).await
    }
}

/// Existing deterministic metadata interpreter used by default and as the failure fallback.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicQueryParser;

#[async_trait]
impl QueryParser for DeterministicQueryParser {
    async fn parse(&self, input: &QueryParserInput) -> Result<QueryIntent, QueryParserError> {
        Ok(deterministic_intent(&input.query, &input.locale))
    }
}

/// Builds a normalized query using the deterministic metadata interpretation.
pub fn normalize_query(original: String, locale: String) -> SearchQuery {
    let intent = deterministic_intent(&original, &locale);
    SearchQuery::from_intent(original, locale, intent)
}

pub(super) fn station_name_hint_queries(original: &str) -> Vec<String> {
    crate::languages::bundled_registry().station_name_hint_queries(original, "ru")
}

#[cfg(test)]
pub(super) fn validate_intent(intent: QueryIntent) -> Result<QueryIntent, QueryParserError> {
    validate_intent_with_taxonomy(intent, &GenreTaxonomy::builtin())
}

pub(super) fn validate_intent_with_taxonomy(
    intent: QueryIntent,
    taxonomy: &GenreTaxonomy,
) -> Result<QueryIntent, QueryParserError> {
    // LLM can return multi-word `terms` like "викер радио" or include symbols.
    // We must normalize them into atomic matchable tokens using `tokenize()`.
    let mut terms = normalize_values(intent.terms);
    terms = terms
        .into_iter()
        .flat_map(|term| tokenize(&term))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    remove_stop_words(&mut terms);

    let raw_query = terms.join(" ");
    let core_term_count = terms.len();

    expand_transliterations(&mut terms);

    let tags = taxonomy.filter_canonical(intent.tags);
    let language = intent
        .language
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    if language.as_deref().is_some_and(|value| {
        !(2..=3).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_lowercase())
    }) {
        return Err(QueryParserError::safe(
            "query parser returned an invalid language filter",
        ));
    }

    let country_code = intent
        .country_code
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| !value.is_empty());
    if country_code.as_deref().is_some_and(|value| {
        value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_uppercase())
    }) {
        return Err(QueryParserError::safe(
            "query parser returned an invalid country filter",
        ));
    }

    Ok(QueryIntent {
        action: intent.action,
        terms,
        tags,
        language,
        country_code,
        core_term_count,
        raw_query,
    })
}

pub(super) fn deterministic_intent(original: &str, _locale: &str) -> QueryIntent {
    let mut terms = tokenize(original);
    let country_code = infer_country_code(&terms);
    let language = explicit_language_code(&terms).map(str::to_owned);
    remove_stop_words(&mut terms);
    let raw_query = terms.join(" ");
    let core_term_count = terms.len();
    expand_transliterations(&mut terms);
    let tags = canonical_tags(terms.clone());

    QueryIntent {
        action: SearchAction::Play,
        terms,
        tags,
        language,
        country_code,
        core_term_count,
        raw_query,
    }
}

fn normalize_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .flat_map(split_camel_case)
        .map(|t| t.to_lowercase())
        .collect()
}

/// Splits a token on camelCase/PascalCase boundaries.
///
/// `"radioDJ"` becomes `["radio", "DJ"]`, `"HelloWorld"` becomes `["Hello", "World"]`.
/// Runs of uppercase followed by a lowercase letter split before the last uppercase
/// so `"XMLParser"` becomes `["XML", "Parser"]`.
fn split_camel_case(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 1 {
        return vec![token.to_owned()];
    }
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let split = (chars[i - 1].is_lowercase() && chars[i].is_uppercase())
            || (i + 1 < chars.len()
                && chars[i - 1].is_uppercase()
                && chars[i].is_uppercase()
                && chars[i + 1].is_lowercase());
        if split {
            let part: String = chars[start..i].iter().collect();
            if !part.is_empty() {
                parts.push(part);
            }
            start = i;
        }
    }
    let tail: String = chars[start..].iter().collect();
    if !tail.is_empty() {
        parts.push(tail);
    }
    if parts.is_empty() {
        vec![token.to_owned()]
    } else {
        parts
    }
}

/// Removes command verbs from query terms using active language modules.
pub(super) fn remove_stop_words(terms: &mut Vec<String>) {
    crate::languages::bundled_registry().remove_stop_words(terms, "ru");
}

/// Expands query terms with transliterated equivalents.
pub(super) fn expand_transliterations(terms: &mut Vec<String>) {
    crate::languages::bundled_registry().expand_transliterations(terms, "ru");
}

pub(super) fn infer_country_code(terms: &[String]) -> Option<String> {
    crate::languages::bundled_registry().infer_country_code(terms, "ru")
}

#[cfg(test)]
mod tests {
    use super::{
        QueryIntent, SearchAction, deterministic_intent, station_name_hint_queries, validate_intent,
    };

    #[test]
    fn provider_intent_is_normalized_and_deduplicated() {
        let intent = validate_intent(QueryIntent {
            action: SearchAction::Play,
            terms: vec![" Jazz ".to_owned(), "jazz".to_owned()],
            tags: vec![" Calm ".to_owned()],
            language: Some("EN".to_owned()),
            country_code: Some("us".to_owned()),
            core_term_count: 0,
            raw_query: String::new(),
        })
        .unwrap();

        assert!(intent.terms.contains(&"jazz".to_owned()));
        assert!(intent.terms.contains(&"джаз".to_owned()));
        assert_eq!(intent.tags, ["calm"]);
        assert_eq!(intent.language.as_deref(), Some("en"));
        assert_eq!(intent.country_code.as_deref(), Some("US"));
    }

    #[test]
    fn invalid_provider_hard_filter_is_rejected() {
        let error = validate_intent(QueryIntent {
            action: SearchAction::Play,
            terms: vec!["jazz".to_owned()],
            tags: Vec::new(),
            language: Some("english".to_owned()),
            country_code: None,
            core_term_count: 0,
            raw_query: String::new(),
        })
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "query parser returned an invalid language filter"
        );
    }

    #[test]
    fn locale_does_not_create_language_or_country_filters() {
        let intent = deterministic_intent("включи медленный джаз", "ru-RU");
        assert_eq!(intent.language, None);
        assert_eq!(intent.country_code, None);
    }

    #[test]
    fn natural_language_fallback_does_not_guess_catalog_tags() {
        let intent = deterministic_intent("русская народная музыка", "ru-RU");
        assert_eq!(intent.language, None);
        assert_eq!(intent.country_code, None);
    }

    #[test]
    fn country_filter_recognizes_world_country_names_and_demonyms() {
        for (request, expected_country_code) in [
            ("Включи немецкий рок", "DE"),
            ("включи рок из Германии", "DE"),
            ("Japanese jazz", "JP"),
            ("Бразилия радио", "BR"),
            ("south african metal", "ZA"),
            ("Эмираты поп", "AE"),
        ] {
            assert_eq!(
                deterministic_intent(request, "ru-RU")
                    .country_code
                    .as_deref(),
                Some(expected_country_code),
                "{request}"
            );
        }
    }

    #[test]
    fn country_filter_recognizes_russian_genitive_for_england() {
        let intent = deterministic_intent("Включи рок из Англии", "ru-RU");

        assert_eq!(intent.country_code.as_deref(), Some("GB"));
    }

    #[test]
    fn ambiguous_country_terms_do_not_create_a_hard_filter() {
        assert_eq!(
            deterministic_intent("рок из Конго", "ru-RU").country_code,
            None
        );
    }

    #[test]
    fn stop_words_are_removed() {
        let intent = deterministic_intent("включи радио диджей", "ru-RU");
        assert!(!intent.terms.contains(&"включи".to_owned()));
        assert!(intent.terms.contains(&"радио".to_owned()));
        assert!(intent.terms.contains(&"диджей".to_owned()));
    }

    #[test]
    fn transliteration_expands_terms() {
        let intent = deterministic_intent("включи радио диджей", "ru-RU");
        assert!(intent.terms.contains(&"radio".to_owned()));
        assert!(intent.terms.contains(&"dj".to_owned()));
    }

    #[test]
    fn transliteration_expands_common_station_tokens() {
        let intent = deterministic_intent("включи радио ультра рокс викер боб год", "ru-RU");
        assert!(intent.terms.contains(&"ультра".to_owned()));
        assert!(intent.terms.contains(&"ultra".to_owned()));
        assert!(intent.terms.contains(&"рокс".to_owned()));
        assert!(intent.terms.contains(&"roks".to_owned()));
        assert!(intent.terms.contains(&"викер".to_owned()));
        assert!(intent.terms.contains(&"viker".to_owned()));
        assert!(intent.terms.contains(&"боб".to_owned()));
        assert!(intent.terms.contains(&"bob".to_owned()));
        assert!(intent.terms.contains(&"год".to_owned()));
        assert!(intent.terms.contains(&"god".to_owned()));
    }

    #[test]
    fn station_name_mode_keeps_ordered_phrase_after_vklyuchi_radio() {
        let hints = station_name_hint_queries("Включи радио рок фм");
        assert!(hints.contains(&"рок фм".to_owned()));
        assert!(
            hints
                .iter()
                .any(|hint| hint.contains("rok") || hint.contains("rock"))
        );
    }

    #[test]
    fn camel_case_split_works() {
        use super::split_camel_case;
        assert_eq!(split_camel_case("radioDJ"), vec!["radio", "DJ"]);
        assert_eq!(split_camel_case("HelloWorld"), vec!["Hello", "World"]);
        assert_eq!(split_camel_case("XMLParser"), vec!["XML", "Parser"]);
        assert_eq!(split_camel_case("simple"), vec!["simple"]);
    }

    #[test]
    fn tokenize_splits_camel_case_names() {
        use super::tokenize;
        let tokens = tokenize("radioDJ");
        assert!(tokens.contains(&"radio".to_owned()));
        assert!(tokens.contains(&"dj".to_owned()));
    }
}
