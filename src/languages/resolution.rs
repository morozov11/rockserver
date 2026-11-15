//! Resolution chain for user communication language.

use std::collections::HashSet;

/// Resolves the effective communication language for a request.
///
/// Priority chain:
/// 1. Account setting (if the user is authenticated and has an explicit language set, and it is published).
/// 2. Request language parameter (if supplied and published).
/// 3. Request locale mapping (e.g. "ru-RU" -> "ru", "en-US" -> "en", if published).
/// 4. Default fallback: "en".
pub fn resolve_communication_language(
    account_language: Option<&str>,
    request_language: Option<&str>,
    request_locale: Option<&str>,
    published_languages: &HashSet<String>,
) -> String {
    // 1. Account setting
    if let Some(lang) = account_language {
        let normalized = lang.trim().to_ascii_lowercase();
        if published_languages.contains(&normalized) {
            return normalized;
        }
    }

    // 2. Request language setting
    if let Some(lang) = request_language {
        let normalized = lang.trim().to_ascii_lowercase();
        if published_languages.contains(&normalized) {
            return normalized;
        }
    }

    // 3. Request locale mapping (backward compatibility with RockCast/RockMobile)
    if let Some(locale) = request_locale {
        let base = extract_base_language(locale);
        if published_languages.contains(&base) {
            return base;
        }
    }

    // 4. Default fallback
    "en".to_owned()
}

/// Extracts the ISO 639-1 base language code from a BCP 47 or POSIX locale.
///
/// `"ru-RU"` becomes `"ru"`, `"en_US"` becomes `"en"`, `"ru"` becomes `"ru"`.
pub fn extract_base_language(locale: &str) -> String {
    let trimmed = locale.trim();
    if trimmed.is_empty() {
        return "en".to_owned();
    }
    trimmed
        .split(['-', '_'])
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("en")
        .to_ascii_lowercase()
}
