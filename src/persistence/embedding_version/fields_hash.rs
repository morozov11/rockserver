//! Canonical `stations.fields_hash` computation over the embedded station fields.

use sha2::{Digest, Sha256};

/// Computes the SHA-256 attestation of the fields sent to the embedding service.
///
/// The hashed fields are exactly the `EmbedStations` inputs: `name`, `tags`
/// (stored order), `language`, `country_code` (empty when absent). Canonical
/// form: the parts `name`, the decimal tag count, every tag, `language`,
/// `country_code` are joined with NUL (`\0`) separators, UTF-8 encoded, then
/// hashed; the hash is lowercase hex. PostgreSQL `text` cannot contain NUL, so
/// stored field values never blur part boundaries, and the leading tag count
/// keeps an empty tag list distinct from a list of empty tags. The embedding
/// service builds the station text from these same fields, so a matching hash
/// proves the vector was derived from the current station fields.
pub fn station_fields_hash(
    name: &str,
    tags: &[String],
    language: Option<&str>,
    country_code: Option<&str>,
) -> String {
    let mut canonical = String::with_capacity(
        name.len() + tags.iter().map(String::len).sum::<usize>() + tags.len() + 16,
    );
    canonical.push_str(name);
    canonical.push('\0');
    canonical.push_str(&tags.len().to_string());
    for tag in tags {
        canonical.push('\0');
        canonical.push_str(tag);
    }
    canonical.push('\0');
    canonical.push_str(language.unwrap_or_default());
    canonical.push('\0');
    canonical.push_str(country_code.unwrap_or_default());
    let digest = Sha256::digest(canonical.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
