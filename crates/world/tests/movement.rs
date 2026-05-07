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
        let ev = move_sys::step_all(&mut w, &mut paths, &layout);
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
        let ev = move_sys::step_all(&mut w, &mut paths, &layout);
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
