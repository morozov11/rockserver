//! Deterministic explicit-language request checks.

use super::{LANGUAGES, explicit_language_code};
use crate::search::tokenize;

fn code(text: &str) -> Option<&'static str> {
    explicit_language_code(&tokenize(text))
}

#[test]
fn all_supported_language_names_need_an_explicit_marker() {
    assert_eq!(LANGUAGES.len(), 28);
    for &(expected, english, russian_form, russian_name) in LANGUAGES {
        assert_eq!(
            code(&format!("play jazz in {english} language")),
            Some(expected)
        );
        assert_eq!(code(&format!("джаз на {russian_form}")), Some(expected));
        assert_eq!(code(&format!("{russian_name} язык")), Some(expected));
        assert_eq!(code(&format!("play {english} jazz")), None);
    }
}

#[test]
fn locale_style_and_ambiguous_markers_do_not_create_hard_filters() {
    assert_eq!(code("включи джаз"), None);
    assert_eq!(code("Включи английский рок"), None);
    assert_eq!(code("French jazz from France"), None);
    assert_eq!(code("jazz in French style"), None);
    assert_eq!(code("jazz in French and German"), None);
    assert_eq!(code("джаз на французском и немецком"), None);
    assert_eq!(code("джаз по-русски"), Some("ru"));
    assert_eq!(code("русскоязычный джаз"), Some("ru"));
}
