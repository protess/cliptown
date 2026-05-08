#[tokio::test]
async fn storage_opens_and_runs_migrations_in_tempdir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let pool = cliptown_world::storage::open(path.to_str().unwrap()).await.unwrap();
    let row: (String,) = sqlx::query_as("PRAGMA journal_mode").fetch_one(&pool).await.unwrap();
    assert_eq!(row.0.to_lowercase(), "wal");
    let row: (i64,) = sqlx::query_as("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='startups'").fetch_one(&pool).await.unwrap();
    assert_eq!(row.0, 1);
}
