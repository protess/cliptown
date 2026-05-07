//! Seeds the default town map (1 town, 7 rooms, 6 doors) on first boot.
//! Idempotent: if towns table already has rows, this is a no-op.

use anyhow::Result;
use sqlx::SqlitePool;
use std::collections::HashMap;

const TOWN_ID: &str = "town_default";
const TOWN_MAP_JSON: &str = include_str!("../seed/town_default.json");

/// In-memory typed representation of the seeded town used by movement,
/// permissions, and pathfinding. Mirrors what `seed_if_empty` writes to SQLite,
/// kept in sync as the single source of truth for layout constants.
#[derive(Debug, Clone)]
pub struct TownLayout {
    pub town_id: String,
    pub rooms: Vec<RoomDef>,
    pub doors: Vec<DoorDef>,
}

#[derive(Debug, Clone)]
pub struct RoomDef {
    pub id: String,
    /// (x, y, w, h) — inclusive of `(x..x+w, y..y+h)`.
    pub bounds: (i32, i32, i32, i32),
    /// `None` => common (anyone can enter); `Some(s)` => private to startup `s`.
    pub private_to_startup_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DoorDef {
    pub id: String,
    pub a_room: String,
    pub b_room: String,
    pub tile: (i32, i32),
}

impl TownLayout {
    /// Build the default town layout (matches `seed_if_empty`).
    /// Suite ownership defaults to `None` (matching the seeded NULL); ownership
    /// is later set when a startup is provisioned.
    pub fn default_town() -> Self {
        let rooms = vec![
            RoomDef { id: "suite_1".into(), bounds: (0, 0, 7, 6),  private_to_startup_id: None },
            RoomDef { id: "suite_2".into(), bounds: (0, 6, 7, 6),  private_to_startup_id: None },
            RoomDef { id: "suite_3".into(), bounds: (33, 0, 7, 6), private_to_startup_id: None },
            RoomDef { id: "suite_4".into(), bounds: (33, 6, 7, 6), private_to_startup_id: None },
            RoomDef { id: "lobby".into(),   bounds: (7, 4, 26, 4), private_to_startup_id: None },
            RoomDef { id: "cafe".into(),    bounds: (7, 0, 26, 4), private_to_startup_id: None },
            RoomDef { id: "library".into(), bounds: (7, 8, 26, 4), private_to_startup_id: None },
        ];
        let doors = vec![
            DoorDef { id: "door_s1_lobby".into(),      a_room: "suite_1".into(), b_room: "lobby".into(),   tile: (7, 4) },
            DoorDef { id: "door_s2_lobby".into(),      a_room: "suite_2".into(), b_room: "lobby".into(),   tile: (7, 7) },
            DoorDef { id: "door_s3_lobby".into(),      a_room: "suite_3".into(), b_room: "lobby".into(),   tile: (33, 4) },
            DoorDef { id: "door_s4_lobby".into(),      a_room: "suite_4".into(), b_room: "lobby".into(),   tile: (33, 7) },
            DoorDef { id: "door_lobby_cafe".into(),    a_room: "lobby".into(),   b_room: "cafe".into(),    tile: (20, 4) },
            DoorDef { id: "door_lobby_library".into(), a_room: "lobby".into(),   b_room: "library".into(), tile: (20, 8) },
        ];
        Self { town_id: TOWN_ID.into(), rooms, doors }
    }

    /// Returns a map of room_id -> bounds, ready to pass to `path::full_route`.
    pub fn room_bounds_map(&self) -> HashMap<String, (i32, i32, i32, i32)> {
        self.rooms.iter().map(|r| (r.id.clone(), r.bounds)).collect()
    }

    pub fn room(&self, id: &str) -> Option<&RoomDef> {
        self.rooms.iter().find(|r| r.id == id)
    }

    pub fn door_at(&self, tile: (i32, i32)) -> Option<&DoorDef> {
        self.doors.iter().find(|d| d.tile == tile)
    }
}

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
