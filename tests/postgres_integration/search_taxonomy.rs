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
