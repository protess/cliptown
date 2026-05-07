use cliptown_world::path::{full_route, tile_path, Door, RoomGraph};
use std::collections::HashMap;

fn three_room_graph() -> RoomGraph {
    RoomGraph::from_doors(vec![
        Door {
            a: "suite_1".into(),
            b: "lobby".into(),
            tile: (7, 4),
        },
        Door {
            a: "lobby".into(),
            b: "library".into(),
            tile: (20, 8),
        },
        Door {
            a: "lobby".into(),
            b: "cafe".into(),
            tile: (20, 4),
        },
    ])
}

#[test]
fn graph_same_room_is_empty_path() {
    let g = three_room_graph();
    assert_eq!(g.route("lobby", "lobby"), Some(vec![]));
}

#[test]
fn graph_suite_to_library_passes_through_lobby() {
    let g = three_room_graph();
    let r = g.route("suite_1", "library").unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].0, "lobby");
    assert_eq!(r[1].0, "library");
}

#[test]
fn graph_no_path_returns_none() {
    let g = RoomGraph::from_doors(vec![Door {
        a: "a".into(),
        b: "b".into(),
        tile: (0, 0),
    }]);
    assert_eq!(g.route("a", "z"), None);
}

#[test]
fn tile_path_within_one_room() {
    let p = tile_path((0, 0, 5, 5), (0, 0), (4, 4)).unwrap();
    assert_eq!(p.first(), Some(&(0, 0)));
    assert_eq!(p.last(), Some(&(4, 4)));
    // Manhattan distance 8 → 9 waypoints.
    assert_eq!(p.len(), 9);
}

#[test]
fn tile_path_out_of_bounds_returns_none() {
    assert_eq!(tile_path((0, 0, 5, 5), (-1, 0), (4, 4)), None);
    assert_eq!(tile_path((0, 0, 5, 5), (0, 0), (5, 5)), None);
}

#[test]
fn full_route_same_room() {
    let g = three_room_graph();
    let mut bounds = HashMap::new();
    bounds.insert("lobby".to_string(), (7, 4, 26, 4));
    let r = full_route(&g, &bounds, ("lobby", (8, 5)), ("lobby", (10, 6))).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, "lobby");
}

#[test]
fn full_route_suite_to_library_via_lobby_with_door_tile_outside_suite() {
    // Verifies the M1.5→M1.6 contract: door tiles lie in only one room's bounds.
    // suite_1 bounds (0..=6, 0..=5); door_s1_lobby = (7, 4) is OUTSIDE suite_1 but inside lobby.
    let g = three_room_graph();
    let mut bounds = HashMap::new();
    bounds.insert("suite_1".to_string(), (0, 0, 7, 6)); // x:0..=6, y:0..=5
    bounds.insert("lobby".to_string(), (7, 4, 26, 4)); // x:7..=32, y:4..=7
    bounds.insert("library".to_string(), (7, 8, 26, 4)); // x:7..=32, y:8..=11

    let r = full_route(&g, &bounds, ("suite_1", (3, 2)), ("library", (15, 10))).unwrap();
    // 3 segments: suite_1, lobby, library.
    assert_eq!(r.len(), 3);
    assert_eq!(r[0].0, "suite_1");
    assert_eq!(r[1].0, "lobby");
    assert_eq!(r[2].0, "library");
    // The first segment must include the door tile (7, 4) as its last waypoint
    // (the agent steps onto the door even though it's outside suite_1's bounds).
    assert_eq!(r[0].1.last(), Some(&(7, 4)));
    // The first segment up to the exit tile must stay inside suite_1.
    for (i, &(x, y)) in r[0].1.iter().enumerate().take(r[0].1.len() - 1) {
        assert!(
            x >= 0 && x < 7 && y >= 0 && y < 6,
            "waypoint {i} ({x},{y}) escaped suite_1 bounds"
        );
    }
}
