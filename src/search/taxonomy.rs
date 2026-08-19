//! Genre taxonomy with parent-child hierarchy for progressive search fallback.
//!
//! The in-memory `GenreTaxonomy` can be loaded from PostgreSQL (`genre_hierarchy`
//! table) or built from the compiled-in static defaults for tests and the
//! in-memory catalog mode.

use std::collections::{BTreeSet, HashMap, HashSet};

const MOOD_TAGS: &[&str] = &["calm", "instrumental", "upbeat", "easy listening"];

/// In-memory genre tree loaded once at startup and shared via `Arc`.
#[derive(Clone, Debug)]
pub struct GenreTaxonomy {
    /// Every canonical tag that an LLM or deterministic parser may return.
    canonical: BTreeSet<String>,
    /// child → parent mapping for the full hierarchy.
    parents: HashMap<String, String>,
}

impl GenreTaxonomy {
    /// Builds the taxonomy from rows retrieved from `genre_hierarchy`.
    pub fn from_rows(rows: impl IntoIterator<Item = GenreRow>) -> Self {
        let mut canonical = BTreeSet::new();
        let mut parents = HashMap::new();
        for row in rows {
            if row.is_canonical {
                canonical.insert(row.tag.clone());
            }
            if let Some(parent) = row.parent_tag {
                parents.insert(row.tag, parent);
            }
        }
        Self { canonical, parents }
    }

    /// Returns the compiled-in default taxonomy used when no database is available.
    pub fn builtin() -> Self {
        Self::from_rows(builtin_rows())
    }

    /// Returns the sorted canonical tag list for LLM structured-output schemas.
    pub fn canonical_tags(&self) -> Vec<&str> {
        self.canonical.iter().map(String::as_str).collect()
    }

    /// Keeps only values present in the canonical vocabulary.
    pub fn filter_canonical(&self, values: impl IntoIterator<Item = String>) -> Vec<String> {
        values
            .into_iter()
            .map(|v| v.trim().to_lowercase())
            .filter(|v| self.canonical.contains(v.as_str()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Returns the parent genre for a subgenre.
    pub fn parent(&self, genre: &str) -> Option<&str> {
        self.parents.get(genre).map(String::as_str)
    }

    /// Collects all ancestor genres from nearest parent to root.
    pub fn ancestors(&self, genre: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = genre.to_owned();
        let mut visited = HashSet::new();
        while let Some(parent) = self.parents.get(&current) {
            if !visited.insert(parent.clone()) {
                break; // cycle guard
            }
            result.push(parent.clone());
            current = parent.clone();
        }
        result
    }

    /// Returns genre constraints, excluding moods and presentation attributes.
    pub fn requested_genres<'a>(&self, tags: &'a [String]) -> Vec<&'a str> {
        tags.iter()
            .map(String::as_str)
            .filter(|tag| !MOOD_TAGS.contains(tag))
            .collect()
    }

    /// A station matches when any of its tags equals the requested genre or
    /// any ancestor of the requested genre.
    pub fn station_matches_requested_genre(
        &self,
        query_tags: &[String],
        station_tags: &[String],
    ) -> bool {
        let genres = self.requested_genres(query_tags);
        if genres.is_empty() {
            return true;
        }
        genres.iter().any(|genre| {
            station_tags.iter().any(|tag| tag == genre)
                || self
                    .ancestors(genre)
                    .iter()
                    .any(|ancestor| station_tags.iter().any(|tag| tag == ancestor))
        })
    }

    /// Collects all ancestor tags for all given tags (for fallback broadening).
    pub fn broaden_tags(&self, tags: &[String]) -> Vec<String> {
        let mut broadened = BTreeSet::new();
        for tag in tags {
            for ancestor in self.ancestors(tag) {
                broadened.insert(ancestor);
            }
        }
        broadened.into_iter().collect()
    }
}

/// One row from `genre_hierarchy`.
#[derive(Clone, Debug)]
pub struct GenreRow {
    pub tag: String,
    pub parent_tag: Option<String>,
    pub is_canonical: bool,
}

// ── Legacy public API (delegates to GenreTaxonomy::builtin) ──────────────

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

/// Returns the parent genre for a subgenre using the builtin static mapping.
pub fn genre_parent(genre: &str) -> Option<&'static str> {
    // Delegates to the builtin taxonomy for backward compatibility.
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
const BUILTIN_PARENT: &[(&str, &str)] = &[
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
fn builtin_rows() -> Vec<GenreRow> {
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

#[cfg(test)]
mod tests {
    use super::{
        GenreTaxonomy, canonical_tags, genre_ancestors, genre_parent,
        station_matches_requested_genre,
    };

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
    fn accepts_new_subgenre_tags() {
        assert_eq!(
            canonical_tags(["black metal".to_owned(), "alternative rock".to_owned()]),
            ["alternative rock", "black metal"]
        );
    }

    #[test]
    fn genre_parent_maps_subgenres() {
        assert_eq!(genre_parent("black metal"), Some("metal"));
        assert_eq!(genre_parent("metal"), Some("rock"));
        assert_eq!(genre_parent("heavy metal"), Some("metal"));
        assert_eq!(genre_parent("hard rock"), Some("rock"));
        assert_eq!(genre_parent("smooth jazz"), Some("jazz"));
        assert_eq!(genre_parent("rock"), None);
        assert_eq!(genre_parent("jazz"), None);
    }

    #[test]
    fn genre_ancestors_walks_full_chain() {
        assert_eq!(genre_ancestors("black metal"), ["metal", "rock"]);
        assert_eq!(genre_ancestors("metal"), ["rock"]);
        assert_eq!(genre_ancestors("hard rock"), ["rock"]);
        assert_eq!(genre_ancestors("rock"), Vec::<&str>::new());
        assert_eq!(genre_ancestors("smooth jazz"), ["jazz"]);
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

    #[test]
    fn subgenre_query_matches_parent_station_tags() {
        assert!(station_matches_requested_genre(
            &["heavy metal".to_owned()],
            &["rock".to_owned()]
        ));
        assert!(station_matches_requested_genre(
            &["black metal".to_owned()],
            &["metal".to_owned()]
        ));
        assert!(station_matches_requested_genre(
            &["black metal".to_owned()],
            &["rock".to_owned()]
        ));
        assert!(station_matches_requested_genre(
            &["smooth jazz".to_owned()],
            &["jazz".to_owned()]
        ));
    }

    #[test]
    fn unrelated_genre_still_rejected() {
        assert!(!station_matches_requested_genre(
            &["heavy metal".to_owned()],
            &["jazz".to_owned()]
        ));
        assert!(!station_matches_requested_genre(
            &["black metal".to_owned()],
            &["pop".to_owned()]
        ));
    }

    #[test]
    fn builtin_taxonomy_matches_static_functions() {
        let taxonomy = GenreTaxonomy::builtin();
        assert_eq!(taxonomy.parent("black metal"), genre_parent("black metal"));
        assert_eq!(taxonomy.parent("metal"), genre_parent("metal"));
        assert_eq!(taxonomy.parent("rock"), genre_parent("rock"));

        let binding = taxonomy.ancestors("black metal");
        let ancestors: Vec<&str> = binding.iter().map(String::as_str).collect();
        assert_eq!(ancestors, genre_ancestors("black metal"));
    }

    #[test]
    fn taxonomy_canonical_tags_include_all_static_tags() {
        let taxonomy = GenreTaxonomy::builtin();
        let canonical = taxonomy.canonical_tags();
        for tag in super::CANONICAL_TAGS {
            assert!(canonical.contains(tag), "missing canonical tag: {tag}");
        }
    }

    #[test]
    fn taxonomy_station_match_is_consistent_with_static() {
        let taxonomy = GenreTaxonomy::builtin();
        let query = vec!["heavy metal".to_owned()];
        let station = vec!["rock".to_owned()];
        assert_eq!(
            taxonomy.station_matches_requested_genre(&query, &station),
            station_matches_requested_genre(&query, &station)
        );
    }

    #[test]
    fn taxonomy_broaden_tags_collects_ancestors() {
        let taxonomy = GenreTaxonomy::builtin();
        let tags = vec!["black metal".to_owned()];
        let broadened = taxonomy.broaden_tags(&tags);
        assert!(broadened.contains(&"metal".to_owned()));
        assert!(broadened.contains(&"rock".to_owned()));
    }
}
