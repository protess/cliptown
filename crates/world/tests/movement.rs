//! Unit tests for the movement subsystem (M1.13).
//! Drives `move_sys::start_move` + `step_all` directly, without any WS plumbing.

use cliptown_world::{
    move_sys::{self, MoveEvent, PathStore, StartMoveResult},
    path::RoomGraph,
    seed::{RoomDef, TownLayout},
    state::{AvatarView, WorldView},
};
use std::collections::HashMap;

fn fresh_avatar(agent_id: &str, startup_id: &str, room_id: &str, pos: (i32, i32)) -> AvatarView {
    AvatarView {
        agent_id: agent_id.into(),
        startup_id: startup_id.into(),
        role: "engineer".into(),
        backend: "claude_code".into(),
        current_pos: pos,
        target_pos: None,
        room_id: room_id.into(),
        status: "idle".into(),
        last_seen_at: None,
        health: cliptown_world::health::Health::Offline,
    }
}

/// Build a fixture: default town, with `suite_1` privately owned by startup
/// `"alpha"` and `suite_3` privately owned by `"beta"`. This lets the
/// permission-deny test exercise an agent from `alpha` trying to enter
/// `beta`'s suite.
fn fixture_with_owned_suites() -> (WorldView, PathStore, TownLayout, RoomGraph) {
    let mut layout = TownLayout::default_town();
    for r in layout.rooms.iter_mut() {
        match r.id.as_str() {
            "suite_1" => r.private_to_startup_id = Some("alpha".into()),
            "suite_3" => r.private_to_startup_id = Some("beta".into()),
            _ => {}
        }
    }
    let graph = move_sys::graph_from_layout(&layout);
    (WorldView::default(), HashMap::new(), layout, graph)
}

#[test]
fn straight_line_move_within_one_room() {
    // Agent in lobby (which spans x:7..=32, y:4..=7) moves straight along
    // y from (10, 5) to (15, 5). Manhattan distance 5 → expect 5 ticks
    // until arrival, current_pos updates each tick, target_pos cleared on
    // arrival, MoveEvent::Complete emitted exactly once.
    let (mut w, mut paths, layout, graph) = fixture_with_owned_suites();
    w.avatars.insert(
        "a1".into(),
        fresh_avatar("a1", "alpha", "lobby", (10, 5)),
    );

    let r = move_sys::start_move(&mut w, &mut paths, &layout, &graph, "a1", "lobby", 15, 5);
    assert_eq!(r, StartMoveResult::Ok);
    assert_eq!(w.avatars["a1"].target_pos, Some((15, 5)));
    assert_eq!(paths.get("a1").map(|q| q.len()), Some(5));

    // Tick 5 times; each tick advances one tile.
    let mut all_events: Vec<MoveEvent> = Vec::new();
    for _ in 0..5 {
        let ev = move_sys::step_all(&mut w, &mut paths);
        all_events.extend(ev);
    }
    assert_eq!(w.avatars["a1"].current_pos, (15, 5));
    assert_eq!(w.avatars["a1"].target_pos, None);
    assert_eq!(w.avatars["a1"].room_id, "lobby");
    assert_eq!(all_events.len(), 1);
    assert_eq!(
        all_events[0],
        MoveEvent::Complete {
            agent_id: "a1".into(),
            room_id: "lobby".into()
        }
    );
    // Path entry consumed.
    assert!(paths.get("a1").is_none());
}

#[test]
fn cross_room_move_through_door_updates_room_id() {
    // Agent in suite_1 at (3, 2) moves to library (15, 10). Route crosses
    // suite_1 → lobby → library via two doors. Verify room_id flips when
    // the avatar steps onto each door tile.
    let (mut w, mut paths, layout, graph) = fixture_with_owned_suites();
    w.avatars.insert(
        "a1".into(),
        fresh_avatar("a1", "alpha", "suite_1", (3, 2)),
    );

    let r = move_sys::start_move(&mut w, &mut paths, &layout, &graph, "a1", "library", 15, 10);
    assert_eq!(r, StartMoveResult::Ok);

    // Drive ticks until the path is consumed (bounded loop to avoid infinite).
    let mut events: Vec<MoveEvent> = Vec::new();
    let mut saw_lobby = false;
    let mut saw_library = false;
    for _ in 0..200 {
        let ev = move_sys::step_all(&mut w, &mut paths);
        events.extend(ev);
        match w.avatars["a1"].room_id.as_str() {
            "lobby" => saw_lobby = true,
            "library" => saw_library = true,
            _ => {}
        }
        if paths.get("a1").is_none() {
            break;
        }
    }

    assert!(saw_lobby, "avatar should pass through lobby");
    assert!(saw_library, "avatar should end in library");
    assert_eq!(w.avatars["a1"].current_pos, (15, 10));
    assert_eq!(w.avatars["a1"].target_pos, None);
    assert_eq!(w.avatars["a1"].room_id, "library");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        MoveEvent::Complete {
            agent_id: "a1".into(),
            room_id: "library".into()
        }
    );
}

#[test]
fn permission_deny_when_entering_foreign_suite() {
    // alpha-owned agent tries to enter beta's suite_3 → PermissionDenied.
    // No path queued, target_pos untouched.
    let (mut w, mut paths, layout, graph) = fixture_with_owned_suites();
    w.avatars.insert(
        "a1".into(),
        fresh_avatar("a1", "alpha", "lobby", (10, 5)),
    );

    let r = move_sys::start_move(&mut w, &mut paths, &layout, &graph, "a1", "suite_3", 35, 2);
    assert_eq!(r, StartMoveResult::PermissionDenied);
    assert!(paths.get("a1").is_none());
    assert_eq!(w.avatars["a1"].target_pos, None);
    assert_eq!(w.avatars["a1"].current_pos, (10, 5));
}

#[test]
fn no_path_when_target_room_unreachable() {
    // Construct a layout with an isolated room (no doors connecting to it),
    // place the agent in a connected room, and try to move to the isolated
    // one. full_route should return None → StartMoveResult::NoPath.
    let mut layout = TownLayout::default_town();
    // Add an unreachable "vault" room with no doors.
    layout.rooms.push(RoomDef {
        id: "vault".into(),
        bounds: (50, 50, 4, 4),
        private_to_startup_id: None,
    });
    let graph = move_sys::graph_from_layout(&layout);

    let mut w = WorldView::default();
    let mut paths: PathStore = HashMap::new();
    w.avatars.insert(
        "a1".into(),
        fresh_avatar("a1", "alpha", "lobby", (10, 5)),
    );

    let r = move_sys::start_move(&mut w, &mut paths, &layout, &graph, "a1", "vault", 51, 51);
    assert_eq!(r, StartMoveResult::NoPath);
    assert!(paths.get("a1").is_none());
    assert_eq!(w.avatars["a1"].target_pos, None);
}

#[test]
fn no_such_agent_returns_no_such_agent() {
    // Sanity: missing agent → NoSuchAgent (no panic).
    let (mut w, mut paths, layout, graph) = fixture_with_owned_suites();
    let r = move_sys::start_move(
        &mut w, &mut paths, &layout, &graph, "ghost", "lobby", 10, 5,
    );
    assert_eq!(r, StartMoveResult::NoSuchAgent);
}

/// Helper: build a default-town fixture (no suites privately owned), with
/// a single avatar inserted in the given room/position. Drives the four
/// regression tests below that exercise the door-tile dedupe bug.
fn default_town_fixture(
    agent_id: &str,
    room: &str,
    pos: (i32, i32),
) -> (WorldView, PathStore, TownLayout, RoomGraph) {
    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    let mut w = WorldView::default();
    w.avatars
        .insert(agent_id.into(), fresh_avatar(agent_id, "alpha", room, pos));
    (w, HashMap::new(), layout, graph)
}

/// Run ticks until the path queue is empty. Returns all events collected.
fn drain_path(
    w: &mut WorldView,
    paths: &mut PathStore,
    agent_id: &str,
) -> Vec<MoveEvent> {
    let mut events: Vec<MoveEvent> = Vec::new();
    for _ in 0..200 {
        let ev = move_sys::step_all(w, paths);
        events.extend(ev);
        if paths.get(agent_id).is_none() {
            break;
        }
    }
    events
}

#[test]
fn cross_room_lobby_to_cafe_updates_room_id() {
    // Door (20, 4) is in-bounds of lobby (7..=32, 4..=7) — this is the bug
    // case where the door-tile dedupe used to drop enter_room=Some("cafe").
    let (mut w, mut paths, layout, graph) =
        default_town_fixture("a1", "lobby", (10, 5));

    let r = move_sys::start_move(
        &mut w, &mut paths, &layout, &graph, "a1", "cafe", 15, 2,
    );
    assert_eq!(r, StartMoveResult::Ok);

    let events = drain_path(&mut w, &mut paths, "a1");

    assert_eq!(w.avatars["a1"].current_pos, (15, 2));
    assert_eq!(w.avatars["a1"].target_pos, None);
    assert_eq!(w.avatars["a1"].room_id, "cafe");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        MoveEvent::Complete {
            agent_id: "a1".into(),
            room_id: "cafe".into()
        }
    );
}

#[test]
fn cross_room_library_to_lobby_updates_room_id() {
    // Door (20, 8) is in-bounds of library (7..=32, 8..=11).
    let (mut w, mut paths, layout, graph) =
        default_town_fixture("a1", "library", (15, 10));

    let r = move_sys::start_move(
        &mut w, &mut paths, &layout, &graph, "a1", "lobby", 18, 6,
    );
    assert_eq!(r, StartMoveResult::Ok);

    let events = drain_path(&mut w, &mut paths, "a1");

    assert_eq!(w.avatars["a1"].current_pos, (18, 6));
    assert_eq!(w.avatars["a1"].target_pos, None);
    assert_eq!(w.avatars["a1"].room_id, "lobby");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        MoveEvent::Complete {
            agent_id: "a1".into(),
            room_id: "lobby".into()
        }
    );
}

#[test]
fn cross_room_suite_3_to_lobby_updates_room_id() {
    // Door (33, 4) — for suite_3 (33..=39, 0..=5) the door IS in-bounds.
    // This is the second bug-case shape (in-bounds of the leaving room).
    let (mut w, mut paths, layout, graph) =
        default_town_fixture("a1", "suite_3", (35, 2));

    let r = move_sys::start_move(
        &mut w, &mut paths, &layout, &graph, "a1", "lobby", 30, 6,
    );
    assert_eq!(r, StartMoveResult::Ok);

    let events = drain_path(&mut w, &mut paths, "a1");

    assert_eq!(w.avatars["a1"].current_pos, (30, 6));
    assert_eq!(w.avatars["a1"].target_pos, None);
    assert_eq!(w.avatars["a1"].room_id, "lobby");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        MoveEvent::Complete {
            agent_id: "a1".into(),
            room_id: "lobby".into()
        }
    );
}

#[test]
fn cross_room_suite_4_to_lobby_updates_room_id() {
    // Door (33, 7) — for suite_4 (33..=39, 6..=11) the door IS in-bounds.
    let (mut w, mut paths, layout, graph) =
        default_town_fixture("a1", "suite_4", (36, 9));

    let r = move_sys::start_move(
        &mut w, &mut paths, &layout, &graph, "a1", "lobby", 30, 6,
    );
    assert_eq!(r, StartMoveResult::Ok);

    let events = drain_path(&mut w, &mut paths, "a1");

    assert_eq!(w.avatars["a1"].current_pos, (30, 6));
    assert_eq!(w.avatars["a1"].target_pos, None);
    assert_eq!(w.avatars["a1"].room_id, "lobby");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        MoveEvent::Complete {
            agent_id: "a1".into(),
            room_id: "lobby".into()
        }
    );
}

/// Drift guard: assert TownLayout::default_town() matches what seed_if_empty()
/// writes to SQL. Both are the source of truth for room geometry; if they
/// drift, movement / pathfinding / permissions desync from the persisted
/// world.
#[tokio::test]
async fn seed_layout_matches_in_memory_default_town() {
    use cliptown_world::seed::seed_if_empty;
    use std::collections::HashMap as StdHashMap;

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("drift.db");
    let pool = cliptown_world::storage::open(p.to_str().unwrap())
        .await
        .unwrap();
    seed_if_empty(&pool).await.unwrap();

    let layout = TownLayout::default_town();

    // Rooms: id, bounds (parsed from JSON), private_to_startup_id.
    #[derive(sqlx::FromRow)]
    struct RoomRow {
        id: String,
        bounds: String,
        private_to_startup_id: Option<String>,
    }
    let rows: Vec<RoomRow> = sqlx::query_as("SELECT id, bounds, private_to_startup_id FROM rooms")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        layout.rooms.len(),
        "room count mismatch between SQL seed and TownLayout::default_town"
    );

    let by_id: StdHashMap<String, RoomRow> =
        rows.into_iter().map(|r| (r.id.clone(), r)).collect();
    for r in &layout.rooms {
        let row = by_id
            .get(&r.id)
            .unwrap_or_else(|| panic!("room {} missing from SQL seed", r.id));
        let v: serde_json::Value = serde_json::from_str(&row.bounds).unwrap();
        let bounds = (
            v["x"].as_i64().unwrap() as i32,
            v["y"].as_i64().unwrap() as i32,
            v["w"].as_i64().unwrap() as i32,
            v["h"].as_i64().unwrap() as i32,
        );
        assert_eq!(
            bounds, r.bounds,
            "bounds mismatch for room {} (SQL vs in-memory)",
            r.id
        );
        assert_eq!(
            row.private_to_startup_id, r.private_to_startup_id,
            "private_to_startup_id mismatch for room {}",
            r.id
        );
    }

    // Doors: id, room_a, room_b, tile_x, tile_y.
    #[derive(sqlx::FromRow)]
    struct DoorRow {
        id: String,
        room_a: String,
        room_b: String,
        tile_x: i32,
        tile_y: i32,
    }
    let drows: Vec<DoorRow> =
        sqlx::query_as("SELECT id, room_a, room_b, tile_x, tile_y FROM room_doors")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        drows.len(),
        layout.doors.len(),
        "door count mismatch between SQL seed and TownLayout::default_town"
    );
    let by_id: StdHashMap<String, DoorRow> =
        drows.into_iter().map(|d| (d.id.clone(), d)).collect();
    for d in &layout.doors {
        let row = by_id
            .get(&d.id)
            .unwrap_or_else(|| panic!("door {} missing from SQL seed", d.id));
        assert_eq!(row.room_a, d.a_room, "a_room mismatch for door {}", d.id);
        assert_eq!(row.room_b, d.b_room, "b_room mismatch for door {}", d.id);
        assert_eq!(
            (row.tile_x, row.tile_y),
            d.tile,
            "tile mismatch for door {}",
            d.id
        );
    }
}
