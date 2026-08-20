//! Confidence-gated semantic language filters built on the query embedding provider.

use std::{env, sync::Arc};

use super::{Embedding, EmbeddingProvider, EmbeddingProviderError};

/// Environment variable that disables semantic language filters when set to `off`.
pub const SEMANTIC_LANGUAGE_FILTERS_ENV: &str = "ROCKSERVER_SEMANTIC_LANGUAGE_FILTERS";

const MIN_LANGUAGE_SCORE: f32 = 0.72;
const MIN_LANGUAGE_MARGIN: f32 = 0.04;

/// Loads the semantic-language-filter switch without reading model paths or secrets.
pub fn semantic_language_filters_enabled() -> Result<bool, EmbeddingProviderError> {
    match env::var(SEMANTIC_LANGUAGE_FILTERS_ENV) {
        Err(env::VarError::NotPresent) => parse_enabled_value(None),
        Err(env::VarError::NotUnicode(_)) => Err(EmbeddingProviderError::safe(
            "semantic language filter setting must be valid Unicode",
        )),
        Ok(value) => parse_enabled_value(Some(&value)),
    }
}

/// Parses the optional non-secret switch independently from process environment access.
fn parse_enabled_value(value: Option<&str>) -> Result<bool, EmbeddingProviderError> {
    match value {
        None => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("on") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("off") => Ok(false),
        Some(_) => Err(EmbeddingProviderError::safe(
            "semantic language filter setting must be `on` or `off`",
        )),
    }
}

/// A language inferred from a semantic label with its confidence diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticLanguageMatch {
    /// ISO 639 language code used as the station hard filter.
    pub code: String,
    /// Cosine similarity with the best language label.
    pub score: f32,
    /// Difference between the best and second-best label similarity.
    pub margin: f32,
}

#[derive(Clone, Debug)]
struct LanguagePrototype {
    code: &'static str,
    embedding: Embedding,
}

/// In-memory semantic classifier for short requests that mention a broadcast language.
///
/// Prototypes are embedded once during service startup with the same local model that
/// embeds user queries. A result is returned only when both its score and its lead
/// over the next candidate are high enough to be safe as a hard search filter.
#[derive(Clone, Debug)]
pub struct SemanticLanguageClassifier {
    prototypes: Vec<LanguagePrototype>,
}

impl SemanticLanguageClassifier {
    /// Prepares language-label embeddings using the configured local embedding provider.
    pub async fn load(
        provider: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self, EmbeddingProviderError> {
        let mut prototypes = Vec::with_capacity(LANGUAGE_LABELS.len());
        for &(code, label) in LANGUAGE_LABELS {
            let embedding = provider.embed_document(label).await?;
            prototypes.push(LanguagePrototype { code, embedding });
        }
        Ok(Self { prototypes })
    }

    /// Returns a language only when the query clearly matches one prepared label.
    pub fn classify(&self, query_embedding: &Embedding) -> Option<SemanticLanguageMatch> {
        let mut scores = self
            .prototypes
            .iter()
            .filter_map(|prototype| {
                cosine_similarity(query_embedding, &prototype.embedding)
                    .map(|score| (prototype.code, score))
            })
            .collect::<Vec<_>>();
        scores.sort_by(|left, right| right.1.total_cmp(&left.1));

        let (code, score) = *scores.first()?;
        let second_score = scores.get(1).map(|(_, score)| *score).unwrap_or(-1.0);
        let margin = score - second_score;
        (score >= MIN_LANGUAGE_SCORE && margin >= MIN_LANGUAGE_MARGIN).then(|| {
            SemanticLanguageMatch {
                code: code.to_owned(),
                score,
                margin,
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn from_embeddings(prototypes: Vec<(&'static str, Embedding)>) -> Self {
        Self {
            prototypes: prototypes
                .into_iter()
                .map(|(code, embedding)| LanguagePrototype { code, embedding })
                .collect(),
        }
    }
}

/// Computes cosine similarity only for embeddings produced by the same model contract.
fn cosine_similarity(left: &Embedding, right: &Embedding) -> Option<f32> {
    (left.provenance() == right.provenance()).then(|| {
        let dot_product = left
            .values()
            .iter()
            .zip(right.values())
            .map(|(left, right)| left * right)
            .sum::<f32>();
        let left_norm = left
            .values()
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        let right_norm = right
            .values()
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        dot_product / (left_norm * right_norm)
    })
}

// Each label includes the English name plus a native form so the multilingual
// encoder can compare short Russian, Latin-script, and native-language requests.
const LANGUAGE_LABELS: &[(&str, &str)] = &[
    ("ar", "Broadcast language: Arabic. العربية. арабский язык."),
    ("cs", "Broadcast language: Czech. čeština. чешский язык."),
    ("da", "Broadcast language: Danish. dansk. датский язык."),
    ("de", "Broadcast language: German. Deutsch. немецкий язык."),
    ("el", "Broadcast language: Greek. ελληνικά. греческий язык."),
    ("en", "Broadcast language: English. английский язык."),
    (
        "es",
        "Broadcast language: Spanish. español. испанский язык.",
    ),
    ("fi", "Broadcast language: Finnish. suomi. финский язык."),
    (
        "fr",
        "Broadcast language: French. français. французский язык.",
    ),
    ("he", "Broadcast language: Hebrew. עברית. иврит."),
    ("hi", "Broadcast language: Hindi. हिन्दी. хинди."),
    (
        "hu",
        "Broadcast language: Hungarian. magyar. венгерский язык.",
    ),
    (
        "id",
        "Broadcast language: Indonesian. bahasa Indonesia. индонезийский язык.",
    ),
    (
        "it",
        "Broadcast language: Italian. italiano. итальянский язык.",
    ),
    ("ja", "Broadcast language: Japanese. 日本語. японский язык."),
    ("ko", "Broadcast language: Korean. 한국어. корейский язык."),
    (
        "nl",
        "Broadcast language: Dutch. Nederlands. нидерландский язык.",
    ),
    (
        "no",
        "Broadcast language: Norwegian. norsk. норвежский язык.",
    ),
    ("pl", "Broadcast language: Polish. polski. польский язык."),
    (
        "pt",
        "Broadcast language: Portuguese. português. португальский язык.",
    ),
    (
        "ro",
        "Broadcast language: Romanian. română. румынский язык.",
    ),
    ("ru", "Broadcast language: Russian. русский язык."),
    ("sv", "Broadcast language: Swedish. svenska. шведский язык."),
    ("th", "Broadcast language: Thai. ไทย. тайский язык."),
    ("tr", "Broadcast language: Turkish. Türkçe. турецкий язык."),
    (
        "uk",
        "Broadcast language: Ukrainian. українська. украинский язык.",
    ),
    (
        "vi",
        "Broadcast language: Vietnamese. tiếng Việt. вьетнамский язык.",
    ),
    ("zh", "Broadcast language: Chinese. 中文. китайский язык."),
];

#[cfg(test)]
mod tests {
    use super::{SemanticLanguageClassifier, parse_enabled_value};
    use crate::search::Embedding;

    fn embedding(values: Vec<f32>) -> Embedding {
        Embedding::new("test", "1", values.len(), values).unwrap()
    }

    #[test]
    fn confident_leader_becomes_a_language_filter() {
        let classifier = SemanticLanguageClassifier::from_embeddings(vec![
            ("en", embedding(vec![1.0, 0.0])),
            ("es", embedding(vec![0.0, 1.0])),
        ]);

        let result = classifier.classify(&embedding(vec![1.0, 0.0])).unwrap();

        assert_eq!(result.code, "en");
        assert!(result.score >= 0.99);
        assert_eq!(
            classifier
                .classify(&embedding(vec![0.0, 1.0]))
                .unwrap()
                .code,
            "es"
        );
    }

    #[test]
    fn close_candidates_do_not_create_a_hard_filter() {
        let classifier = SemanticLanguageClassifier::from_embeddings(vec![
            ("en", embedding(vec![1.0, 0.0])),
            ("es", embedding(vec![0.99, 0.1])),
        ]);

        assert!(classifier.classify(&embedding(vec![1.0, 0.0])).is_none());
    }

    #[test]
    fn switch_defaults_to_on_and_accepts_explicit_off() {
        assert!(parse_enabled_value(None).unwrap());
        assert!(!parse_enabled_value(Some("off")).unwrap());
        assert!(parse_enabled_value(Some("on")).unwrap());
        assert!(parse_enabled_value(Some("unexpected")).is_err());
    }
}
