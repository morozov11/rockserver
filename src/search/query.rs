//! Provider-neutral query interpretation and deterministic metadata fallback.

use std::{collections::BTreeSet, error::Error, fmt};

use async_trait::async_trait;

use super::{SearchQuery, taxonomy::canonical_tags};

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

pub(super) fn validate_intent(intent: QueryIntent) -> Result<QueryIntent, QueryParserError> {
    let terms = normalize_values(intent.terms);
    let tags = canonical_tags(intent.tags);
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
    })
}

pub(super) fn deterministic_intent(original: &str, _locale: &str) -> QueryIntent {
    let terms = tokenize(original);
    let tags = canonical_tags(terms.clone());
    let country_code = infer_country_code(&terms);
    let language = infer_language(&terms);

    QueryIntent {
        action: SearchAction::Play,
        terms,
        tags,
        language,
        country_code,
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

pub(super) fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn infer_language(terms: &[String]) -> Option<String> {
    let term_set = terms.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if term_set.contains("русскоязычный") || term_set.contains("русскоязычная")
    {
        Some("ru".to_owned())
    } else if term_set.contains("англоязычный") || term_set.contains("англоязычная")
    {
        Some("en".to_owned())
    } else {
        None
    }
}

pub(super) fn infer_country_code(terms: &[String]) -> Option<String> {
    let term_set = terms.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if [
        "russia",
        "russian",
        "россия",
        "россии",
        "российский",
        "российская",
    ]
    .iter()
    .any(|term| term_set.contains(term))
    {
        Some("RU".to_owned())
    } else if term_set.contains("british") || term_set.contains("uk") {
        Some("GB".to_owned())
    } else if term_set.contains("american") || term_set.contains("usa") {
        Some("US".to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{QueryIntent, SearchAction, deterministic_intent, validate_intent};

    #[test]
    fn provider_intent_is_normalized_and_deduplicated() {
        let intent = validate_intent(QueryIntent {
            action: SearchAction::Play,
            terms: vec![" Jazz ".to_owned(), "jazz".to_owned()],
            tags: vec![" Calm ".to_owned()],
            language: Some("EN".to_owned()),
            country_code: Some("us".to_owned()),
        })
        .unwrap();

        assert_eq!(intent.terms, ["jazz"]);
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
        assert!(intent.tags.is_empty());
    }

    #[test]
    fn natural_language_fallback_does_not_guess_catalog_tags() {
        let intent = deterministic_intent("русская народная музыка", "ru-RU");
        assert_eq!(intent.language, None);
        assert_eq!(intent.country_code, None);
        assert!(intent.tags.is_empty());
    }
}
