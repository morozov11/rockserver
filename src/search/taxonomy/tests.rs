//! Tests for controlled genre taxonomy behavior, contract versioning, selection, and caching.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use super::{
    CANONICAL_TAGS, GenreTaxonomy, GenreTaxonomySource, MAX_PROMPT_TAGS,
    TAXONOMY_TEST_PAIR_EXPECTED, TaxonomyCache, TaxonomySourceKind, canonical_tags,
    genre_ancestors, genre_parent, select_prompt_tags, station_matches_requested_genre,
    taxonomy_version,
};

// ── Contract §5.1 Hash & Version Tests ───────────────────────────────────

#[test]
fn contract_test_pair_hash_matches_rockmodules() {
    let tags = ["rock", "jazz", "blues"].map(str::to_owned);
    assert_eq!(taxonomy_version(&tags), TAXONOMY_TEST_PAIR_EXPECTED);
}

#[test]
fn order_of_input_tags_does_not_matter_for_version() {
    let a = ["blues", "jazz", "rock"].map(str::to_owned);
    let b = ["jazz", "rock", "blues"].map(str::to_owned);
    let c = ["rock", "blues", "jazz"].map(str::to_owned);
    assert_eq!(taxonomy_version(&a), taxonomy_version(&b));
    assert_eq!(taxonomy_version(&b), taxonomy_version(&c));
}

#[test]
fn sorting_is_by_utf8_bytes_not_code_points() {
    let tags = ["\u{ff}", "a"].map(str::to_owned);
    assert_eq!(
        taxonomy_version(&tags),
        taxonomy_version(&["a", "\u{ff}"].map(str::to_owned))
    );
}

#[test]
fn empty_taxonomy_has_defined_version() {
    assert_eq!(
        taxonomy_version::<&str>(&[]),
        "sha256:4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    );
}

// ── GenreTaxonomy Struct & Builtin Version Tests ─────────────────────────

#[test]
fn builtin_taxonomy_has_valid_version_and_prompt_tags() {
    let taxonomy = GenreTaxonomy::builtin();
    assert!(taxonomy.is_usable());
    assert!(taxonomy.version().starts_with("sha256:"));
    assert_eq!(taxonomy.version(), taxonomy.taxonomy_version());
    assert!(!taxonomy.prompt_tags().is_empty());
    assert!(taxonomy.prompt_tags().len() <= MAX_PROMPT_TAGS);
    // Version must match the hash of prompt_tags
    assert_eq!(taxonomy.version(), taxonomy_version(taxonomy.prompt_tags()));
}

// ── Deterministic Prompt Tag Selection Tests ─────────────────────────────

#[test]
fn select_prompt_tags_preserves_all_tags_when_within_limit() {
    let tags = ["rock", "jazz", "blues"]
        .into_iter()
        .map(String::from)
        .collect::<BTreeSet<_>>();
    let parents = HashMap::new();
    let selected = select_prompt_tags(&tags, &parents);
    assert_eq!(selected, vec!["blues", "jazz", "rock"]);
}

#[test]
fn select_prompt_tags_filters_leaves_on_large_catalogs() {
    let mut canonical = BTreeSet::new();
    let mut parents = HashMap::new();

    // 10 root genres
    for i in 0..10 {
        let root = format!("root-{i:02}");
        canonical.insert(root);
    }

    // 10 branching subgenres (children of root-00, and parents of leaves)
    for i in 0..10 {
        let branch = format!("branch-{i:02}");
        canonical.insert(branch.clone());
        parents.insert(branch, "root-00".to_string());
    }

    // 200 leaf subgenres (children of branches)
    for i in 0..200 {
        let leaf = format!("leaf-{i:03}");
        canonical.insert(leaf.clone());
        parents.insert(leaf, format!("branch-{:02}", i % 10));
    }

    assert_eq!(canonical.len(), 220);
    let selected = select_prompt_tags(&canonical, &parents);

    // Selected must contain all 10 roots + 10 branching nodes = 20 nodes, no leaves.
    assert_eq!(selected.len(), 20);
    for i in 0..10 {
        assert!(selected.contains(&format!("root-{i:02}")));
        assert!(selected.contains(&format!("branch-{i:02}")));
    }
    for i in 0..200 {
        assert!(!selected.contains(&format!("leaf-{i:03}")));
    }
}

#[test]
fn select_prompt_tags_is_strictly_deterministic() {
    let mut canonical = BTreeSet::new();
    let mut parents = HashMap::new();

    for i in 0..160 {
        canonical.insert(format!("genre-{i}"));
        if i > 20 {
            parents.insert(format!("genre-{i}"), format!("genre-{}", i % 20));
        }
    }

    let first = select_prompt_tags(&canonical, &parents);
    let second = select_prompt_tags(&canonical, &parents);
    assert_eq!(first, second);
}

// ── Taxonomy Cache Tests ─────────────────────────────────────────────────

struct CountingTaxonomySource {
    call_count: AtomicUsize,
    result: std::sync::Mutex<Result<GenreTaxonomy, String>>,
}

impl CountingTaxonomySource {
    fn new(result: Result<GenreTaxonomy, String>) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            result: std::sync::Mutex::new(result),
        }
    }

    fn count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    fn set_result(&self, result: Result<GenreTaxonomy, String>) {
        *self.result.lock().unwrap() = result;
    }
}

#[async_trait]
impl GenreTaxonomySource for CountingTaxonomySource {
    async fn load(&self) -> Result<GenreTaxonomy, String> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.result.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn cache_avoids_per_query_database_hits() {
    let source = Arc::new(CountingTaxonomySource::new(Ok(GenreTaxonomy::builtin())));
    let cache = TaxonomyCache::new(Some(source.clone()), Duration::from_secs(300));

    // First call must load from source
    let tax1 = cache.get_or_load().await;
    assert_eq!(source.count(), 1);
    assert_eq!(cache.source_kind().await, Some(TaxonomySourceKind::Db));

    // Subsequent 10 calls must use the cached snapshot without hitting source
    for _ in 0..10 {
        let tax = cache.get_or_load().await;
        assert_eq!(tax.version(), tax1.version());
    }
    assert_eq!(source.count(), 1);
}

#[tokio::test]
async fn cache_reloads_after_ttl_expires() {
    let source = Arc::new(CountingTaxonomySource::new(Ok(GenreTaxonomy::builtin())));
    // Set 0 TTL so every read expires
    let cache = TaxonomyCache::new(Some(source.clone()), Duration::ZERO);

    let _ = cache.get_or_load().await;
    assert_eq!(source.count(), 1);

    let _ = cache.get_or_load().await;
    assert_eq!(source.count(), 2);
}

#[tokio::test]
async fn cache_refresh_forces_reload() {
    let source = Arc::new(CountingTaxonomySource::new(Ok(GenreTaxonomy::builtin())));
    let cache = TaxonomyCache::new(Some(source.clone()), Duration::from_secs(300));

    let _ = cache.get_or_load().await;
    assert_eq!(source.count(), 1);

    let refreshed = cache.refresh().await.unwrap();
    assert_eq!(source.count(), 2);
    assert!(refreshed.is_usable());
}

#[tokio::test]
async fn cache_degradation_falls_back_to_builtin_on_error() {
    let source = Arc::new(CountingTaxonomySource::new(Ok(GenreTaxonomy::builtin())));
    let cache = TaxonomyCache::new(Some(source.clone()), Duration::ZERO);

    // Initial load succeeds from db
    let initial = cache.get_or_load().await;
    assert_eq!(source.count(), 1);
    assert_eq!(cache.source_kind().await, Some(TaxonomySourceKind::Db));

    // Source now fails
    source.set_result(Err("database down".to_string()));

    // Next load re-attempts, fails, and falls back to builtin
    let degraded = cache.get_or_load().await;
    assert_eq!(source.count(), 2);
    assert_eq!(
        cache.source_kind().await,
        Some(TaxonomySourceKind::Fallback)
    );
    assert_eq!(degraded.canonical_tags(), initial.canonical_tags());
}

#[tokio::test]
async fn cache_startup_failure_falls_back_to_builtin() {
    let source = Arc::new(CountingTaxonomySource::new(Err("db offline".to_string())));
    let cache = TaxonomyCache::new(Some(source.clone()), Duration::from_secs(300));

    let tax = cache.get_or_load().await;
    assert_eq!(source.count(), 1);
    assert_eq!(
        cache.source_kind().await,
        Some(TaxonomySourceKind::Fallback)
    );
    assert!(tax.is_usable());
    assert_eq!(
        tax.canonical_tags(),
        GenreTaxonomy::builtin().canonical_tags()
    );
}

// ── Legacy Taxonomy Unit Tests ───────────────────────────────────────────

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
    for tag in CANONICAL_TAGS {
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
