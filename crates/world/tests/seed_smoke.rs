#[tokio::test]
async fn seed_creates_one_town_seven_rooms_six_doors() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.db");
    let pool = cliptown_world::storage::open(p.to_str().unwrap()).await.unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();

    let towns: (i64,) = sqlx::query_as("SELECT count(*) FROM towns").fetch_one(&pool).await.unwrap();
    assert_eq!(towns.0, 1, "expected 1 town");

    let rooms: (i64,) = sqlx::query_as("SELECT count(*) FROM rooms").fetch_one(&pool).await.unwrap();
    assert_eq!(rooms.0, 7, "expected 7 rooms");

    let doors: (i64,) = sqlx::query_as("SELECT count(*) FROM room_doors").fetch_one(&pool).await.unwrap();
    assert_eq!(doors.0, 6, "expected 6 doors");

    // Verify suite slots are unowned
    let unowned_suites: (i64,) = sqlx::query_as("SELECT count(*) FROM rooms WHERE type='office' AND private_to_startup_id IS NULL")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(unowned_suites.0, 4, "expected 4 unowned suite slots");

    // Idempotent: calling again is a no-op
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    let towns2: (i64,) = sqlx::query_as("SELECT count(*) FROM towns").fetch_one(&pool).await.unwrap();
    assert_eq!(towns2.0, 1, "second seed call must not duplicate");
}
