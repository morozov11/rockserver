//! Taxonomy version hashing according to contract §5.1.

use sha2::{Digest, Sha256};

/// Fixed contract test vector constant (§5.1 of `contract.md`).
///
/// Computed from tags `["rock", "jazz", "blues"]` sorted to `["blues", "jazz", "rock"]`.
pub const TAXONOMY_TEST_PAIR_EXPECTED: &str =
    "sha256:9ce0dd764d87a64be0c744d49c8c7fb39c059603649e1b0b42fc4f184d90a436";

/// Computes the canonical taxonomy version string from a slice of tags.
///
/// In accordance with §5.1 of `contract.md`:
/// 1. Tags are sorted lexicographically by their UTF-8 bytes.
/// 2. Serialized to a compact JSON array without whitespace (`["blues","jazz","rock"]`).
/// 3. Hashed using SHA-256.
/// 4. Formatted as `"sha256:<hex_digest>"`.
pub fn taxonomy_version<T: AsRef<str>>(tags: &[T]) -> String {
    let mut sorted: Vec<&str> = tags.iter().map(|tag| tag.as_ref()).collect();
    sorted.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    let canonical = serde_json::to_string(&sorted).unwrap_or_default();
    let digest = Sha256::digest(canonical.as_bytes());
    format!("sha256:{digest:x}")
}
