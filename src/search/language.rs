//! Explicit broadcast-language markers in user request text.

use std::collections::BTreeSet;

// English names support "in French" / "French language" requests. Russian forms
// support "на французском" / "французский язык" without using STT locale.
const LANGUAGES: &[(&str, &str, &str, &str)] = &[
    ("ar", "arabic", "арабском", "арабский"),
    ("cs", "czech", "чешском", "чешский"),
    ("da", "danish", "датском", "датский"),
    ("de", "german", "немецком", "немецкий"),
    ("el", "greek", "греческом", "греческий"),
    ("en", "english", "английском", "английский"),
    ("es", "spanish", "испанском", "испанский"),
    ("fi", "finnish", "финском", "финский"),
    ("fr", "french", "французском", "французский"),
    ("he", "hebrew", "иврите", "иврит"),
    ("hi", "hindi", "хинди", "хинди"),
    ("hu", "hungarian", "венгерском", "венгерский"),
    ("id", "indonesian", "индонезийском", "индонезийский"),
    ("it", "italian", "итальянском", "итальянский"),
    ("ja", "japanese", "японском", "японский"),
    ("ko", "korean", "корейском", "корейский"),
    ("nl", "dutch", "нидерландском", "нидерландский"),
    ("no", "norwegian", "норвежском", "норвежский"),
    ("pl", "polish", "польском", "польский"),
    ("pt", "portuguese", "португальском", "португальский"),
    ("ro", "romanian", "румынском", "румынский"),
    ("ru", "russian", "русском", "русский"),
    ("sv", "swedish", "шведском", "шведский"),
    ("th", "thai", "тайском", "тайский"),
    ("tr", "turkish", "турецком", "турецкий"),
    ("uk", "ukrainian", "украинском", "украинский"),
    ("vi", "vietnamese", "вьетнамском", "вьетнамский"),
    ("zh", "chinese", "китайском", "китайский"),
];

/// Returns a hard-filter code only for one explicit, unambiguous language request.
pub(super) fn explicit_language_code(terms: &[String]) -> Option<&'static str> {
    let mut requested = BTreeSet::new();
    for (index, pair) in terms.windows(2).enumerate() {
        let code = match pair[0].as_str() {
            "in" if terms.get(index + 2).is_some_and(|next| next == "style") => None,
            "in" => language_by_english(&pair[1]),
            "на" => language_by_russian(&pair[1]),
            "по" if pair[1] == "русски" => Some("ru"),
            _ if pair[1] == "language" => language_by_english(&pair[0]),
            _ if matches!(pair[1].as_str(), "язык" | "языке") => {
                language_by_russian(&pair[0])
            }
            _ => None,
        };
        requested.extend(code);
    }
    for term in terms {
        match term.as_str() {
            "русскоязычный" | "русскоязычная" => {
                requested.insert("ru");
            }
            "англоязычный" | "англоязычная" => {
                requested.insert("en");
            }
            _ => {}
        }
    }
    let code = (requested.len() == 1).then(|| *requested.first().unwrap())?;
    let mentions_other_language = terms.iter().any(|term| {
        language_by_english(term)
            .or_else(|| language_by_russian(term))
            .is_some_and(|mentioned| mentioned != code)
    });
    (!mentions_other_language).then_some(code)
}

fn language_by_english(term: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(_, english, _, _)| *english == term)
        .map(|(code, _, _, _)| *code)
}

fn language_by_russian(term: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(_, _, form, noun)| *form == term || *noun == term)
        .map(|(code, _, _, _)| *code)
}

#[cfg(test)]
#[path = "language/tests.rs"]
mod tests;
