//! Pathfinding for the world: a coarse room-graph A* (waypoints between
//! rooms) composed with a tile-grid A* (steps inside one room), bridged by
//! a door-edge helper that handles door tiles whose seeded coordinates lie
//! inside only one of the two rooms they connect.
//!
//! See M1.5 → M1.6 contract: door tiles in the seed (e.g. `door_s1_lobby
//! = (7, 4)`) are inside the lobby but outside `suite_1`'s bounds. The
//! `adjacent_inside` helper resolves this by picking the in-bounds tile
//! that is Manhattan-adjacent to the door, so the agent can step *onto*
//! the door tile as a transition step without requiring the door tile to
//! be in both rooms' bounds.
use pathfinding::prelude::astar;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Door {
    pub a: String,
    pub b: String,
    pub tile: (i32, i32),
}

#[derive(Debug, Clone)]
pub struct RoomGraph {
    pub doors: Vec<Door>,
    pub neighbors: HashMap<String, Vec<(String, (i32, i32))>>,
}

impl RoomGraph {
    pub fn from_doors(doors: Vec<Door>) -> Self {
        let mut neighbors: HashMap<String, Vec<(String, (i32, i32))>> = HashMap::new();
        for d in &doors {
            neighbors
                .entry(d.a.clone())
                .or_default()
                .push((d.b.clone(), d.tile));
            neighbors
                .entry(d.b.clone())
                .or_default()
                .push((d.a.clone(), d.tile));
        }
        Self { doors, neighbors }
    }

    /// Returns `Vec<(room_id, door_tile_to_enter_room)>` from `from` to `to`.
    /// Empty `Vec` if same room. `None` if no path.
    pub fn route(&self, from: &str, to: &str) -> Option<Vec<(String, (i32, i32))>> {
        if from == to {
            return Some(vec![]);
        }
        let result = astar(
            &from.to_string(),
            |r| {
                self.neighbors
                    .get(r)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(nb, _tile)| (nb, 1u32))
                    .collect::<Vec<_>>()
            },
            |_| 0u32,
            |r| r == to,
        );
        result.map(|(path, _)| {
            path.windows(2)
                .map(|w| {
                    let next = &w[1];
                    let tile = self
                        .neighbors
                        .get(&w[0])
                        .unwrap()
                        .iter()
                        .find(|(n, _)| n == next)
                        .unwrap()
                        .1;
                    (next.clone(), tile)
                })
                .collect()
        })
    }
}

/// Given a room's bounds `(x, y, w, h)` and from/to tiles, return tile
/// waypoints inside the room (4-connected, uniform cost). Returns `None`
/// if `from` or `to` lies outside the bounds, or if no path exists.
pub fn tile_path(
    bounds: (i32, i32, i32, i32),
    from: (i32, i32),
    to: (i32, i32),
) -> Option<Vec<(i32, i32)>> {
    let in_bounds = |x: i32, y: i32| {
        x >= bounds.0 && x < bounds.0 + bounds.2 && y >= bounds.1 && y < bounds.1 + bounds.3
    };
    if !in_bounds(from.0, from.1) || !in_bounds(to.0, to.1) {
        return None;
    }
    let result = astar(
        &from,
        |&(x, y)| {
            let mut nbs = vec![];
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let nx = x + dx;
                let ny = y + dy;
                if in_bounds(nx, ny) {
                    nbs.push(((nx, ny), 1u32));
                }
            }
            nbs
        },
        |&(x, y)| ((x - to.0).abs() + (y - to.1).abs()) as u32,
        |&p| p == to,
    );
    result.map(|(path, _)| path)
}

/// Returns the tile inside `bounds` that is Manhattan-distance 1 from
/// `door_tile` (or the door tile itself, if it happens to be in-bounds).
/// Used by `full_route` to bridge across rooms whose bounds don't include
/// the door tile.
fn adjacent_inside(bounds: (i32, i32, i32, i32), door_tile: (i32, i32)) -> Option<(i32, i32)> {
    let in_bounds = |x: i32, y: i32| {
        x >= bounds.0 && x < bounds.0 + bounds.2 && y >= bounds.1 && y < bounds.1 + bounds.3
    };
    if in_bounds(door_tile.0, door_tile.1) {
        return Some(door_tile);
    }
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let nx = door_tile.0 + dx;
        let ny = door_tile.1 + dy;
        if in_bounds(nx, ny) {
            return Some((nx, ny));
        }
    }
    None
}

/// Compose a full path from `(from_room, from_tile)` to `(to_room,
/// to_tile)`. Returns `Vec<(room_id, tile_waypoints_inside_that_room)>`.
/// The door tile is appended as the last waypoint of every transitional
/// segment (i.e. every segment except the destination room's), so the
/// agent "steps onto" the door for one tick before entering the next
/// room. This is the M1.5 → M1.6 contract bridge.
pub fn full_route(
    graph: &RoomGraph,
    room_bounds: &HashMap<String, (i32, i32, i32, i32)>,
    from: (&str, (i32, i32)),
    to: (&str, (i32, i32)),
) -> Option<Vec<(String, Vec<(i32, i32)>)>> {
    let waypoints = graph.route(from.0, to.0)?;

    // Same-room case: just one tile_path.
    if waypoints.is_empty() {
        let bounds = room_bounds.get(from.0).copied()?;
        let segment = tile_path(bounds, from.1, to.1)?;
        return Some(vec![(from.0.to_string(), segment)]);
    }

    let mut current_room = from.0.to_string();
    let mut current_tile = from.1;
    let mut out: Vec<(String, Vec<(i32, i32)>)> = Vec::new();

    for (next_room, door_tile) in &waypoints {
        let bounds = room_bounds.get(&current_room).copied()?;
        // Find the in-bounds exit tile (adjacent to or equal to door_tile).
        let exit_tile = adjacent_inside(bounds, *door_tile)?;
        let mut segment = tile_path(bounds, current_tile, exit_tile)?;
        // Step onto the door tile as the last waypoint of this segment.
        segment.push(*door_tile);
        out.push((current_room.clone(), segment));

        // Enter the next room: starting tile is the in-bounds neighbor of
        // the door tile in the new room.
        let next_bounds = room_bounds.get(next_room).copied()?;
        let entry_tile = adjacent_inside(next_bounds, *door_tile)?;
        current_room = next_room.clone();
        current_tile = entry_tile;
    }

    // Final segment inside destination room (current_tile is already inside it).
    let bounds = room_bounds.get(&current_room).copied()?;
    let segment = tile_path(bounds, current_tile, to.1)?;
    out.push((current_room, segment));
    Some(out)
}
