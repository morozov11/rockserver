//! Genre taxonomy with parent-child hierarchy for progressive search fallback.
//!
//! The in-memory `GenreTaxonomy` can be loaded from PostgreSQL (`genre_hierarchy`
//! table) or built from the compiled-in static defaults for tests and the
//! in-memory catalog mode.

mod builtin;
mod cache;
mod selection;
mod version;

#[cfg(test)]
#[path = "taxonomy/tests.rs"]
mod tests;

pub use builtin::{
    CANONICAL_TAGS, MOOD_TAGS, canonical_tags, genre_ancestors, genre_parent, requested_genres,
    station_matches_requested_genre,
};
pub use cache::{DEFAULT_TAXONOMY_TTL, TAXONOMY_TTL_ENV, TaxonomyCache, TaxonomySourceKind};
pub use selection::{MAX_PROMPT_TAGS, select_prompt_tags};
pub use version::{TAXONOMY_TEST_PAIR_EXPECTED, taxonomy_version};

use async_trait::async_trait;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Loads the bounded controlled genre vocabulary without exposing station catalog data.
#[async_trait]
pub trait GenreTaxonomySource: Send + Sync {
    /// Returns the current taxonomy; callers use builtin defaults if loading fails.
    async fn load(&self) -> Result<GenreTaxonomy, String>;
}

/// One row from `genre_hierarchy`.
#[derive(Clone, Debug)]
pub struct GenreRow {
    pub tag: String,
    pub parent_tag: Option<String>,
    pub is_canonical: bool,
}

/// In-memory genre tree loaded once at startup and shared via `Arc`.
#[derive(Clone, Debug)]
pub struct GenreTaxonomy {
    /// Every canonical tag that an LLM or deterministic parser may return.
    canonical: BTreeSet<String>,
    /// child → parent mapping for the full hierarchy.
    parents: HashMap<String, String>,
    /// Selected subset of tags for LLM prompt schemas (<= 150 items).
    prompt_tags: Vec<String>,
    /// sha256:hex taxonomy version string computed from prompt_tags according to contract §5.1.
    version: String,
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
        let prompt_tags = select_prompt_tags(&canonical, &parents);
        let version = taxonomy_version(&prompt_tags);
        Self {
            canonical,
            parents,
            prompt_tags,
            version,
        }
    }

    /// Returns the compiled-in default taxonomy used when no database is available.
    pub fn builtin() -> Self {
        Self::from_rows(builtin::builtin_rows())
    }

    /// Returns the computed taxonomy version string according to contract §5.1.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the computed taxonomy version string (alias for `version()`).
    pub fn taxonomy_version(&self) -> &str {
        &self.version
    }

    /// Returns the deterministic subset of tags selected for LLM prompt schemas (<= 150 items).
    pub fn prompt_tags(&self) -> &[String] {
        &self.prompt_tags
    }

    /// Returns the sorted canonical tag list.
    pub fn canonical_tags(&self) -> Vec<&str> {
        self.canonical.iter().map(String::as_str).collect()
    }

    /// Checks one exact canonical tag in the current snapshot.
    pub fn contains_tag(&self, tag: &str) -> bool {
        self.canonical.contains(tag)
    }

    /// Whether this snapshot contains a usable controlled vocabulary.
    pub fn is_usable(&self) -> bool {
        !self.canonical.is_empty()
            && self.canonical.len() <= 2048
            && self.canonical.iter().all(|tag| valid_tag(tag))
            && self.parents.len() <= 2048
            && self
                .parents
                .iter()
                .all(|(tag, parent)| valid_tag(tag) && valid_tag(parent) && tag != parent)
            && !self.prompt_tags.is_empty()
            && self.prompt_tags.len() <= MAX_PROMPT_TAGS
            && self.version.starts_with("sha256:")
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

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 64
        && tag.trim() == tag
        && tag == tag.to_lowercase()
        && tag
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, ' ' | '&' | '/' | '+' | '-' | '!'))
}
