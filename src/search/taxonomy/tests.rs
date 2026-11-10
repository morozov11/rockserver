//! Tests for controlled genre taxonomy behavior.

use super::{
    GenreTaxonomy, canonical_tags, genre_ancestors, genre_parent, station_matches_requested_genre,
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
