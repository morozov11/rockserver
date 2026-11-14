//! Unit tests for the versioned embedding storage helpers.

use super::{
    EMBEDDINGS_DIMENSION, EMBEDDINGS_MODEL, EMBEDDINGS_VERSION, MAX_BATCH_STATIONS,
    embeddings_table_name, station_fields_hash,
};

#[test]
fn table_and_constant_values_pin_the_v1_contract() {
    assert_eq!(EMBEDDINGS_VERSION, 1);
    assert_eq!(EMBEDDINGS_DIMENSION, 384);
    assert_eq!(EMBEDDINGS_MODEL, "intfloat/multilingual-e5-small");
    assert_eq!(MAX_BATCH_STATIONS, 64);
    assert_eq!(
        embeddings_table_name(EMBEDDINGS_VERSION),
        "station_embeddings_v1"
    );
    assert_eq!(embeddings_table_name(7), "station_embeddings_v7");
}

#[test]
fn fields_hash_is_stable_and_field_sensitive() {
    let base = station_fields_hash(
        "Rock Antenne",
        &["rock".into(), "classic rock".into()],
        Some("de"),
        Some("DE"),
    );
    // Same inputs produce the same hash.
    assert_eq!(
        base,
        station_fields_hash(
            "Rock Antenne",
            &["rock".to_owned(), "classic rock".to_owned()],
            Some("de"),
            Some("DE")
        )
    );
    // Every field change must change the hash.
    assert_ne!(
        base,
        station_fields_hash(
            "Rock Antenne Live",
            &["rock".to_owned(), "classic rock".to_owned()],
            Some("de"),
            Some("DE")
        )
    );
    assert_ne!(
        base,
        station_fields_hash(
            "Rock Antenne",
            &["classic rock".to_owned(), "rock".to_owned()],
            Some("de"),
            Some("DE")
        )
    );
    assert_ne!(
        base,
        station_fields_hash("Rock Antenne", &["rock".to_owned()], Some("de"), Some("DE"))
    );
    assert_ne!(
        base,
        station_fields_hash(
            "Rock Antenne",
            &["rock".to_owned(), "classic rock".to_owned()],
            None,
            Some("DE")
        )
    );
    assert_ne!(
        base,
        station_fields_hash(
            "Rock Antenne",
            &["rock".to_owned(), "classic rock".to_owned()],
            Some("de"),
            None
        )
    );
    // Absent and empty-string optional fields are the same canonical part.
    assert_eq!(
        station_fields_hash("A", &[], None, None),
        station_fields_hash("A", &[], Some(""), Some(""))
    );
    // An empty tag list is distinct from one empty tag.
    assert_ne!(
        station_fields_hash("A", &[], None, None),
        station_fields_hash("A", &["".to_owned()], None, None)
    );
    // Hash format is 64 lowercase hex characters.
    assert_eq!(base.len(), 64);
    assert!(
        base.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
}

#[test]
fn fields_hash_matches_independently_computed_canonical_form() {
    // Golden construction of the documented canonical form, computed without
    // the production helper to pin the format:
    // name \0 tag_count \0 tag... \0 language \0 country_code, SHA-256 hex.
    let tags = vec!["jazz".to_owned(), "blues".to_owned()];
    let canonical = format!("Radio X\0{}\0jazz\0blues\0en\0US", tags.len());
    let digest = {
        use sha2::{Digest, Sha256};
        let bytes = Sha256::digest(canonical.as_bytes());
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        station_fields_hash("Radio X", &tags, Some("en"), Some("US")),
        digest
    );
}
