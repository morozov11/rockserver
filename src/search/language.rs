//! Explicit broadcast-language markers in user request text.

// English names support "in French" / "French language" requests. Russian forms
// support "на французском" / "французский язык" without using STT locale.
#[cfg(test)]
pub(crate) const LANGUAGES: &[(&str, &str, &str, &str)] = &[
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
    crate::languages::bundled_registry().explicit_language_code(terms)
}

#[cfg(test)]
#[path = "language/tests.rs"]
mod tests;
