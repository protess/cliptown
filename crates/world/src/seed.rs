//! Seeds the default town map (1 town, 7 rooms, 6 doors) on first boot.
//! Idempotent: if towns table already has rows, this is a no-op.

use anyhow::Result;
use sqlx::SqlitePool;

const TOWN_ID: &str = "town_default";
const TOWN_MAP_JSON: &str = include_str!("../seed/town_default.json");

pub async fn seed_if_empty(pool: &SqlitePool) -> Result<()> {
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM towns")
        .fetch_one(pool).await?;
    if count.0 > 0 { return Ok(()); }

    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO towns (id, name, map_json) VALUES (?, ?, ?)")
        .bind(TOWN_ID).bind("Default Town").bind(TOWN_MAP_JSON)
        .execute(&mut *tx).await?;

    // 4 suite slots (initially unowned: private_to_startup_id NULL) + 3 common rooms.
    // Layout (40x12 tile grid):
    //   suite_1 (top-left)    | cafe (top-mid)        | suite_3 (top-right)
    //   suite_2 (mid-left)    | lobby (middle spine)  | suite_4 (mid-right)
    //                         | library (bottom-mid)
    let rooms: [(&str, &str, &str, &str); 7] = [
        ("suite_1", "Suite 1", "office",  r#"{"x":0,"y":0,"w":7,"h":6}"#),
        ("suite_2", "Suite 2", "office",  r#"{"x":0,"y":6,"w":7,"h":6}"#),
        ("suite_3", "Suite 3", "office",  r#"{"x":33,"y":0,"w":7,"h":6}"#),
        ("suite_4", "Suite 4", "office",  r#"{"x":33,"y":6,"w":7,"h":6}"#),
        ("lobby",   "Lobby",   "transit", r#"{"x":7,"y":4,"w":26,"h":4}"#),
        ("cafe",    "Cafe",    "social",  r#"{"x":7,"y":0,"w":26,"h":4}"#),
        ("library", "Library", "focus",   r#"{"x":7,"y":8,"w":26,"h":4}"#),
    ];
    for (id, name, kind, bounds) in rooms {
        sqlx::query("INSERT INTO rooms (id, town_id, name, type, bounds, private_to_startup_id) VALUES (?, ?, ?, ?, ?, NULL)")
            .bind(id).bind(TOWN_ID).bind(name).bind(kind).bind(bounds)
            .execute(&mut *tx).await?;
    }
    // 6 doors: each suite <-> lobby, lobby <-> cafe, lobby <-> library.
    let doors: [(&str, &str, &str, i32, i32); 6] = [
        ("door_s1_lobby",      "suite_1", "lobby",   7,  4),
        ("door_s2_lobby",      "suite_2", "lobby",   7,  7),
        ("door_s3_lobby",      "suite_3", "lobby",   33, 4),
        ("door_s4_lobby",      "suite_4", "lobby",   33, 7),
        ("door_lobby_cafe",    "lobby",   "cafe",    20, 4),
        ("door_lobby_library", "lobby",   "library", 20, 8),
    ];
    for (id, a, b, x, y) in doors {
        sqlx::query("INSERT INTO room_doors (id, town_id, room_a, room_b, tile_x, tile_y) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(id).bind(TOWN_ID).bind(a).bind(b).bind(x).bind(y)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
