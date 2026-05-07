//! Movement subsystem: per-tick avatar stepping along precomputed paths.
//!
//! Paths are stored OUTSIDE `WorldView` (in the loop's closure) since clients
//! don't need to see them. `start_move` computes a full route via
//! `path::full_route` and stashes the flattened tile sequence in `PathStore`.
//! `step_all` advances every avatar one tile per call (intended to be invoked
//! once per 1Hz tick) and emits `MoveEvent::Complete` for any avatar that
//! reached its `target_pos`.
//!
//! Door crossings: when the new tile matches a door tile, the avatar's
//! `room_id` is updated to the door's other side. `path::full_route` includes
//! the door tile as the last waypoint of the exiting segment, so detection is
//! a simple lookup by tile coordinate.

use crate::path::{full_route, Door, RoomGraph};
use crate::seed::TownLayout;
use crate::state::WorldView;
use std::collections::{HashMap, VecDeque};

/// One step in a precomputed route: the tile to move onto, plus an optional
/// room transition that should fire when the avatar arrives at that tile.
/// The transition is set ONLY for the door-crossing waypoints emitted by
/// `path::full_route` (so a coincidental door tile passed *through* by
/// `tile_path` won't accidentally flip rooms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub tile: (i32, i32),
    pub enter_room: Option<String>,
}

/// Per-agent precomputed path. Populated by `start_move`; consumed one
/// `Step` per tick by `step_all`.
pub type PathStore = HashMap<String, VecDeque<Step>>;

/// Result from `start_move`; the caller turns this into a worker reply.
#[derive(Debug, PartialEq, Eq)]
pub enum StartMoveResult {
    Ok,
    NoPath,
    PermissionDenied,
    NoSuchAgent,
}

/// Build a `RoomGraph` for the given layout (used by callers that don't have
/// one already; tests, in particular).
pub fn graph_from_layout(layout: &TownLayout) -> RoomGraph {
    RoomGraph::from_doors(
        layout
            .doors
            .iter()
            .map(|d| Door {
                a: d.a_room.clone(),
                b: d.b_room.clone(),
                tile: d.tile,
            })
            .collect(),
    )
}

/// Permission check matching `permissions::can_enter_room`'s rule but driven
/// purely from the layout (no `AgentRef` plumbing needed inside the world loop).
fn can_enter_layout_room(layout: &TownLayout, startup_id: &str, room_id: &str) -> bool {
    match layout.room(room_id) {
        Some(r) => match &r.private_to_startup_id {
            None => true,
            Some(owner) => owner == startup_id,
        },
        None => false,
    }
}

/// Begin a move for `agent_id` toward `(target_x, target_y)` in room
/// `target_room`. Computes the path via `path::full_route`, stores it, and
/// sets `target_pos` on the avatar. The first waypoint of the route is
/// the agent's current tile and is dropped (we step starting from waypoint 1).
pub fn start_move(
    world: &mut WorldView,
    paths: &mut PathStore,
    layout: &TownLayout,
    graph: &RoomGraph,
    agent_id: &str,
    target_room: &str,
    target_x: i32,
    target_y: i32,
) -> StartMoveResult {
    let avatar = match world.avatars.get_mut(agent_id) {
        Some(a) => a,
        None => return StartMoveResult::NoSuchAgent,
    };
    let startup_id = avatar.startup_id.clone();
    if !can_enter_layout_room(layout, &startup_id, target_room) {
        return StartMoveResult::PermissionDenied;
    }
    let from = avatar.current_pos;
    let from_room = avatar.room_id.clone();
    let bounds = layout.room_bounds_map();
    let route = match full_route(
        graph,
        &bounds,
        (&from_room, from),
        (target_room, (target_x, target_y)),
    ) {
        Some(r) => r,
        None => return StartMoveResult::NoPath,
    };

    // Build a Step queue from the segmented route. Each segment except the
    // last ends with a door tile; that door tile gets `enter_room = next room`.
    // Internal waypoints carry `enter_room = None`. We drop the very first
    // tile of the route (the agent's current position) so we don't tick in
    // place, and dedupe consecutive duplicates that arise when an in-bounds
    // exit tile already equals the door tile.
    let mut steps: Vec<Step> = Vec::new();
    let mut last_tile: Option<(i32, i32)> = None;
    for (i, (_room, seg)) in route.iter().enumerate() {
        let is_last_segment = i + 1 == route.len();
        let last_idx = seg.len().saturating_sub(1);
        for (j, &t) in seg.iter().enumerate() {
            if last_tile == Some(t) {
                continue;
            }
            let enter_room = if !is_last_segment && j == last_idx {
                // Door waypoint — agent transitions into the next segment's room.
                Some(route[i + 1].0.clone())
            } else {
                None
            };
            steps.push(Step { tile: t, enter_room });
            last_tile = Some(t);
        }
    }
    // Drop the leading "step" if it equals the start tile (no actual move).
    if steps.first().map(|s| s.tile) == Some(from) {
        steps.remove(0);
    }

    avatar.target_pos = Some((target_x, target_y));
    if steps.is_empty() {
        // Already at target; clear target_pos. No path entry, no event next tick.
        avatar.target_pos = None;
        return StartMoveResult::Ok;
    }
    paths.insert(agent_id.to_string(), steps.into_iter().collect());
    StartMoveResult::Ok
}

/// Per-tick step. For each agent in `paths`, advance one tile.
/// On arrival (path empty after this step), clears `target_pos` and emits
/// `MoveEvent::Complete`. Door-tile transitions are taken from the planned
/// `Step.enter_room`, never inferred from coincidental tile matches.
pub fn step_all(
    world: &mut WorldView,
    paths: &mut PathStore,
    _layout: &TownLayout,
) -> Vec<MoveEvent> {
    let mut events = Vec::new();
    let mut to_remove: Vec<String> = Vec::new();

    for (agent_id, path) in paths.iter_mut() {
        let avatar = match world.avatars.get_mut(agent_id) {
            Some(a) => a,
            None => {
                to_remove.push(agent_id.clone());
                continue;
            }
        };
        let step = match path.pop_front() {
            Some(s) => s,
            None => {
                to_remove.push(agent_id.clone());
                avatar.target_pos = None;
                continue;
            }
        };
        avatar.current_pos = step.tile;
        if let Some(next_room) = step.enter_room {
            avatar.room_id = next_room;
        }

        if path.is_empty() {
            avatar.target_pos = None;
            to_remove.push(agent_id.clone());
            events.push(MoveEvent::Complete {
                agent_id: agent_id.clone(),
                room_id: avatar.room_id.clone(),
            });
        }
    }

    for k in to_remove {
        paths.remove(&k);
    }
    events
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveEvent {
    Complete { agent_id: String, room_id: String },
}
