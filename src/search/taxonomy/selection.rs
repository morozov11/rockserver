//! Deterministic prompt tag selection for LLM structured output.
//!
//! The full taxonomy table (`genre_hierarchy`) contains ~1,680 genres, which exceeds
//! the maximum prompt schema capacity and the contract limit of <= 150 tags (§5.1).
//!
//! This module implements a deterministic rule for selecting a representative subset:
//! 1. Only canonical tags (`is_canonical == true`) are eligible.
//! 2. If the total eligible tags <= 150 (such as in tests or small catalogs), all canonical tags are preserved.
//! 3. If the total eligible tags > 150:
//!    - All root genres (`parent_tag IS NULL`) are included (top-level genres and moods/attributes).
//!    - All branching category genres (genres that act as parents to other subgenres) are included.
//!    - Hyper-specific leaf subgenres are omitted from the LLM prompt enum, while remaining in the database
//!      hierarchy for post-filtering and ancestor broadening.
//! 4. Tags are sorted lexicographically by UTF-8 bytes and capped at 150.
//!
//! On the production catalog (migration 0006, 1,646 tags):
//! - 28 root genres
//! - 43 branching category subgenres
//! - Exactly 71 tags selected (well within the 150-tag limit).

use std::collections::{BTreeSet, HashMap, HashSet};

/// Maximum number of tags allowed in an LLM prompt schema according to contract §5.1.
pub const MAX_PROMPT_TAGS: usize = 150;

/// Selects a bounded, deterministic subset of canonical tags suitable for LLM prompt schemas.
pub fn select_prompt_tags(
    canonical: &BTreeSet<String>,
    parents: &HashMap<String, String>,
) -> Vec<String> {
    if canonical.len() <= MAX_PROMPT_TAGS {
        let mut tags: Vec<String> = canonical.iter().cloned().collect();
        tags.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        return tags;
    }

    // Set of tags that appear as parents to at least one child tag.
    let parent_set: HashSet<&str> = parents.values().map(String::as_str).collect();

    let mut selected = BTreeSet::new();
    for tag in canonical {
        let is_root = !parents.contains_key(tag);
        let is_branching = parent_set.contains(tag.as_str());

        if is_root || is_branching {
            selected.insert(tag.clone());
        }
    }

    let mut tags: Vec<String> = selected.into_iter().collect();
    tags.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    if tags.len() > MAX_PROMPT_TAGS {
        tags.truncate(MAX_PROMPT_TAGS);
    }
    tags
}
