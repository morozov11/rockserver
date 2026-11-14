//! Compiled-in fallback taxonomy defaults and legacy helper functions.

use std::collections::{BTreeSet, HashSet};

use super::GenreRow;

/// Mood and presentation tags that do not represent musical genres.
pub const MOOD_TAGS: &[&str] = &["calm", "instrumental", "upbeat", "easy listening"];

/// Canonical genre and mood tags compiled into the binary as a fallback.
///
/// At runtime, prefer `GenreTaxonomy::canonical_tags()` loaded from the database.
pub const CANONICAL_TAGS: &[&str] = &[
    "acid jazz",
    "alternative rock",
    "ambient",
    "black metal",
    "blues",
    "calm",
    "classic rock",
    "classical",
    "country",
    "dance",
    "death metal",
    "doom metal",
    "electronic",
    "folk",
    "funk",
    "hard rock",
    "hardcore",
    "heavy metal",
    "hip hop",
    "indie rock",
    "instrumental",
    "jazz",
    "latin",
    "latin jazz",
    "metal",
    "news",
    "pop",
    "pop rock",
    "power metal",
    "progressive rock",
    "psychedelic rock",
    "punk",
    "reggae",
    "rock",
    "russian folk",
    "smooth jazz",
    "soft rock",
    "soul",
    "symphonic metal",
    "talk",
    "thrash metal",
    "upbeat",
    "world music",
];

/// Keeps only normalized values from the controlled catalog vocabulary using fallback defaults.
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

/// Returns the parent genre for a subgenre using the builtin static mapping.
pub fn genre_parent(genre: &str) -> Option<&'static str> {
    BUILTIN_PARENT
        .iter()
        .find(|(g, _)| *g == genre)
        .map(|(_, p)| *p)
}

/// Collects all ancestor genres using the builtin static mapping.
pub fn genre_ancestors(genre: &str) -> Vec<&'static str> {
    let mut ancestors = Vec::new();
    let mut current = genre;
    while let Some(parent) = genre_parent(current) {
        ancestors.push(parent);
        current = parent;
    }
    ancestors
}

/// Station genre match using the builtin static mapping.
pub fn station_matches_requested_genre(query_tags: &[String], station_tags: &[String]) -> bool {
    let genres = requested_genres(query_tags);
    if genres.is_empty() {
        return true;
    }
    genres.iter().any(|genre| {
        station_tags.iter().any(|tag| tag == genre)
            || genre_ancestors(genre)
                .iter()
                .any(|ancestor| station_tags.iter().any(|tag| tag == ancestor))
    })
}

// Static parent mapping for legacy functions and in-memory fallback.
pub(crate) const BUILTIN_PARENT: &[(&str, &str)] = &[
    // Metal subgenres → metal
    ("black metal", "metal"),
    ("death metal", "metal"),
    ("doom metal", "metal"),
    ("power metal", "metal"),
    ("symphonic metal", "metal"),
    ("thrash metal", "metal"),
    ("heavy metal", "metal"),
    ("progressive metal", "metal"),
    ("nu metal", "metal"),
    ("folk metal", "metal"),
    ("gothic metal", "metal"),
    ("industrial metal", "metal"),
    ("speed metal", "metal"),
    ("sludge metal", "metal"),
    ("groove metal", "metal"),
    ("metalcore", "metal"),
    ("deathcore", "metal"),
    ("alternative metal", "metal"),
    // Metal → rock
    ("metal", "rock"),
    // Rock subgenres → rock
    ("alternative rock", "rock"),
    ("classic rock", "rock"),
    ("hard rock", "rock"),
    ("indie rock", "rock"),
    ("pop rock", "rock"),
    ("progressive rock", "rock"),
    ("psychedelic rock", "rock"),
    ("soft rock", "rock"),
    ("punk rock", "rock"),
    ("grunge", "rock"),
    ("garage rock", "rock"),
    ("post-rock", "rock"),
    ("post-punk", "rock"),
    ("shoegaze", "rock"),
    ("stoner rock", "rock"),
    ("blues rock", "rock"),
    ("southern rock", "rock"),
    ("folk rock", "rock"),
    ("country rock", "rock"),
    ("new wave", "rock"),
    ("britpop", "rock"),
    ("emo", "rock"),
    // Jazz subgenres → jazz
    ("acid jazz", "jazz"),
    ("latin jazz", "jazz"),
    ("smooth jazz", "jazz"),
    ("bebop", "jazz"),
    ("cool jazz", "jazz"),
    ("free jazz", "jazz"),
    ("fusion", "jazz"),
    ("swing", "jazz"),
    ("big band", "jazz"),
    ("gypsy jazz", "jazz"),
    ("nu jazz", "jazz"),
    ("vocal jazz", "jazz"),
    // Blues subgenres → blues
    ("chicago blues", "blues"),
    ("delta blues", "blues"),
    ("electric blues", "blues"),
    // Punk subgenres
    ("hardcore", "punk"),
    ("pop punk", "punk"),
    ("post-hardcore", "punk"),
    ("screamo", "punk"),
    // Electronic subgenres → electronic
    ("house", "electronic"),
    ("techno", "electronic"),
    ("trance", "electronic"),
    ("drum and bass", "electronic"),
    ("dubstep", "electronic"),
    ("downtempo", "electronic"),
    ("synthwave", "electronic"),
    ("chillout", "electronic"),
    ("trip hop", "electronic"),
    ("edm", "electronic"),
    // Electronic sub-subgenres
    ("deep house", "house"),
    ("tech house", "house"),
    ("progressive house", "house"),
    ("progressive trance", "trance"),
    ("psytrance", "trance"),
    ("liquid dnb", "drum and bass"),
    // Country subgenres → country
    ("bluegrass", "country"),
    ("americana", "country"),
    ("country pop", "country"),
    // Folk subgenres → folk
    ("russian folk", "folk"),
    ("celtic", "folk"),
    ("indie folk", "folk"),
    // Reggae subgenres → reggae
    ("dub", "reggae"),
    ("dancehall", "reggae"),
    ("reggaeton", "reggae"),
    // Latin subgenres → latin
    ("salsa", "latin"),
    ("bossa nova", "latin"),
    ("cumbia", "latin"),
    ("bachata", "latin"),
    ("regional mexicana", "latin"),
    // Soul subgenres → soul
    ("neo-soul", "soul"),
    ("motown", "soul"),
    // Funk subgenres → funk
    ("funk rock", "funk"),
    // Dance subgenres → dance
    ("disco", "dance"),
    ("nu-disco", "dance"),
    // Ambient subgenres → ambient
    ("dark ambient", "ambient"),
    ("drone", "ambient"),
    // R&B subgenres
    ("contemporary r&b", "r&b"),
];

/// Constructs `GenreRow` entries from the compiled-in parent mapping.
pub(crate) fn builtin_rows() -> Vec<GenreRow> {
    let parent_tags: HashSet<&str> = BUILTIN_PARENT.iter().map(|(_, p)| *p).collect();

    let mut rows = Vec::new();

    // All tags from CANONICAL_TAGS as canonical entries.
    for tag in CANONICAL_TAGS {
        let parent = BUILTIN_PARENT
            .iter()
            .find(|(c, _)| c == tag)
            .map(|(_, p)| p.to_string());
        rows.push(GenreRow {
            tag: tag.to_string(),
            parent_tag: parent,
            is_canonical: true,
        });
    }

    // Root genres referenced as parents but not yet in CANONICAL_TAGS.
    for parent in &parent_tags {
        if !CANONICAL_TAGS.contains(parent) && !rows.iter().any(|r| r.tag == *parent) {
            rows.push(GenreRow {
                tag: parent.to_string(),
                parent_tag: BUILTIN_PARENT
                    .iter()
                    .find(|(c, _)| c == parent)
                    .map(|(_, p)| p.to_string()),
                is_canonical: true,
            });
        }
    }

    // Child tags from BUILTIN_PARENT not yet added.
    for (child, parent) in BUILTIN_PARENT {
        if !rows.iter().any(|r| r.tag == *child) {
            rows.push(GenreRow {
                tag: child.to_string(),
                parent_tag: Some(parent.to_string()),
                is_canonical: true,
            });
        }
    }

    rows
}
