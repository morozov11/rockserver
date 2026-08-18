//! Controlled vocabulary shared by query providers and catalog matching.

use std::collections::BTreeSet;

/// Canonical genre and mood tags that an intent provider may return.
///
/// These are catalog values, not user-language phrases. Natural-language
/// translation belongs to the intent provider; repository matching only sees
/// this bounded vocabulary.
pub const CANONICAL_TAGS: &[&str] = &[
    "ambient",
    "blues",
    "calm",
    "classical",
    "country",
    "dance",
    "electronic",
    "folk",
    "funk",
    "hard rock",
    "hardcore",
    "heavy metal",
    "hip hop",
    "instrumental",
    "jazz",
    "latin",
    "metal",
    "news",
    "pop",
    "punk",
    "reggae",
    "rock",
    "russian folk",
    "soul",
    "talk",
    "upbeat",
    "world music",
];

const MOOD_TAGS: &[&str] = &["calm", "instrumental", "upbeat"];

/// Keeps only normalized values from the controlled catalog vocabulary.
pub fn canonical_tags(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let allowed = CANONICAL_TAGS.iter().copied().collect::<BTreeSet<_>>();
    values
        .into_iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| allowed.contains(value.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Returns genre constraints, excluding moods and presentation attributes.
pub fn requested_genres(tags: &[String]) -> Vec<&str> {
    tags.iter()
        .map(String::as_str)
        .filter(|tag| !MOOD_TAGS.contains(tag))
        .collect()
}

/// A query containing a genre must match that genre in station metadata.
/// Mood-only queries remain valid and are ranked normally.
pub fn station_matches_requested_genre(query_tags: &[String], station_tags: &[String]) -> bool {
    let genres = requested_genres(query_tags);
    genres.is_empty()
        || genres
            .iter()
            .any(|genre| station_tags.iter().any(|tag| tag == genre))
}

#[cfg(test)]
mod tests {
    use super::{canonical_tags, station_matches_requested_genre};

    #[test]
    fn accepts_only_canonical_catalog_values() {
        assert_eq!(
            canonical_tags([
                " Folk ".to_owned(),
                "русская народная музыка".to_owned(),
                "folk".to_owned(),
            ]),
            ["folk"]
        );
    }

    #[test]
    fn genre_is_required_while_mood_is_optional() {
        let query = vec!["reggae".to_owned(), "upbeat".to_owned()];
        assert!(!station_matches_requested_genre(
            &query,
            &["rock".to_owned(), "upbeat".to_owned()]
        ));
        assert!(station_matches_requested_genre(
            &query,
            &["reggae".to_owned()]
        ));
        assert!(station_matches_requested_genre(
            &["calm".to_owned()],
            &["ambient".to_owned(), "calm".to_owned()]
        ));
    }
}
