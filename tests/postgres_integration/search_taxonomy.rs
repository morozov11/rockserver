//! Live PostgreSQL vocabulary refresh on an explicitly disposable database.

use std::env;

use rockserver::{persistence::PostgresGenreTaxonomySource, search::taxonomy::GenreTaxonomySource};
use sqlx::PgPool;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and ROCKSERVER_SEARCH_EVAL_DISPOSABLE=1"]
async fn inserted_genre_is_visible_to_next_taxonomy_read() {
    assert_eq!(
        env::var("ROCKSERVER_SEARCH_EVAL_DISPOSABLE").as_deref(),
        Ok("1")
    );
    let url = env::var("TEST_DATABASE_URL").expect("isolated TEST_DATABASE_URL is required");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let source = PostgresGenreTaxonomySource::from_pool(pool.clone());
    let before = source.load().await.unwrap();
    assert!(before.is_usable());
    assert!(!before.canonical_tags().contains(&"srch007 test genre"));

    sqlx::query(
        "INSERT INTO genre_hierarchy (tag, parent_tag, display_name) VALUES ($1, 'rock', $2)",
    )
    .bind("srch007 test genre")
    .bind("SRCH-007 test genre")
    .execute(&pool)
    .await
    .unwrap();
    let after = source.load().await.unwrap();
    assert!(after.is_usable());
    assert!(after.canonical_tags().contains(&"srch007 test genre"));
    assert_eq!(after.parent("srch007 test genre"), Some("rock"));

    sqlx::query("DELETE FROM genre_hierarchy WHERE tag = $1")
        .bind("srch007 test genre")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and ROCKSERVER_SEARCH_EVAL_DISPOSABLE=1"]
async fn db_taxonomy_divergence_from_fallback_is_expected_and_verified() {
    assert_eq!(
        env::var("ROCKSERVER_SEARCH_EVAL_DISPOSABLE").as_deref(),
        Ok("1")
    );
    let url = env::var("TEST_DATABASE_URL").expect("isolated TEST_DATABASE_URL is required");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();

    let source = PostgresGenreTaxonomySource::from_pool(pool.clone());
    let db_taxonomy = source
        .load()
        .await
        .expect("database taxonomy must load successfully");

    // The database must supply a usable, non-empty controlled vocabulary.
    assert!(db_taxonomy.is_usable(), "database taxonomy must be usable");
    let db_canonical = db_taxonomy.canonical_tags();
    assert!(
        !db_canonical.is_empty(),
        "database canonical tags must not be empty"
    );

    // Contract §5.1: version must be computed as sha256:...
    let version = db_taxonomy.version();
    assert!(
        version.starts_with("sha256:"),
        "database taxonomy version must start with sha256:, got: {version}"
    );

    // Prompt tags subset must be bounded by contract limit <= 150 and not empty.
    let prompt_tags = db_taxonomy.prompt_tags();
    assert!(
        prompt_tags.len() <= 150,
        "prompt tags ({}) must not exceed 150",
        prompt_tags.len()
    );
    assert!(!prompt_tags.is_empty(), "prompt tags must not be empty");

    // Divergence with CANONICAL_TAGS fallback constant is verified.
    // Divergence is NOT an error: the database genre_hierarchy is the single source of truth (~1646 tags),
    // whereas CANONICAL_TAGS is only a 43-tag static fallback for offline mode.
    let fallback = rockserver::search::taxonomy::GenreTaxonomy::builtin();
    let fallback_canonical = fallback.canonical_tags();
    assert_ne!(
        db_canonical.len(),
        fallback_canonical.len(),
        "database catalog count ({}) and fallback count ({}) must diverge",
        db_canonical.len(),
        fallback_canonical.len()
    );

    eprintln!(
        "DB taxonomy verified: version={}, canonical_tags={}, prompt_tags={}",
        version,
        db_canonical.len(),
        prompt_tags.len()
    );

    pool.close().await;
}
