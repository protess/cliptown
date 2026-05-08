use crate::move_sys::{self, MoveEvent, PathStore};
use crate::path::RoomGraph;
use crate::seed::TownLayout;
use crate::state::{AvatarView, WorldView};
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Debug)]
pub enum Cmd {
    Tick,
    HandleConsoleMsg {
        msg: serde_json::Value,
        reply: oneshot::Sender<serde_json::Value>,
    },
    HandleWorkerMsg {
        agent_id: String,
        msg: serde_json::Value,
        reply: oneshot::Sender<serde_json::Value>,
    },
    RegisterWorker {
        agent_id: String,
        tx: mpsc::Sender<serde_json::Value>,
    },
    UnregisterWorker {
        agent_id: String,
    },
    BackendCatalogUpdated(HashMap<String, serde_json::Value>),
    /// Insert freshly-provisioned avatars into the in-memory world view, and
    /// optionally mark a suite as privately owned by a startup. Sent by
    /// `api_startups::create_startup` after the SQL transaction commits.
    ///
    /// Without this, `mcp_dispatch` lookups + the scheduler can't see the new
    /// agents, and `move_sys::can_enter_layout_room` keeps treating the
    /// claimed suite as public (since the loop's layout was built once at
    /// startup and never reloads from SQL).
    InsertAvatars {
        avatars: Vec<AvatarView>,
        /// `(suite_id, startup_id)` — when `Some`, the layout's matching room
        /// has its `private_to_startup_id` set. Mirrors what the SQL UPDATE
        /// already wrote to `rooms.private_to_startup_id`.
        claim_suite: Option<(String, String)>,
    },
    /// Release any suites owned by `startup_id` in the in-memory layout (set
    /// `private_to_startup_id = None`) and drop all avatars belonging to that
    /// startup from the in-memory world view. Mirrors `delete_startup`'s SQL
    /// `UPDATE rooms SET private_to_startup_id = NULL WHERE
    /// private_to_startup_id = ?` plus the agent cleanup so the freed suite
    /// immediately stops rejecting other startups in
    /// `move_sys::can_enter_layout_room` AND the dissolved startup's avatars
    /// disappear from console snapshots, proximity, and the scheduler.
    ReleaseSuite {
        startup_id: String,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
    pub view_rx: watch::Receiver<WorldView>,
}

pub fn spawn(initial: WorldView, pool: SqlitePool) -> Handle {
    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    spawn_with_layout(initial, pool, layout, graph)
}

pub fn spawn_with_layout(
    initial: WorldView,
    pool: SqlitePool,
    layout: TownLayout,
    graph: RoomGraph,
) -> Handle {
    let (tx, mut rx) = mpsc::channel::<Cmd>(1024);
    let (view_tx, view_rx) = watch::channel(initial.clone());
    let mut w = initial;
    let mut paths: PathStore = HashMap::new();
    let mut out_bus: HashMap<String, mpsc::Sender<serde_json::Value>> = HashMap::new();
    // `layout` is owned + `mut` so `Cmd::InsertAvatars`/`Cmd::ReleaseSuite`
    // can flip suite ownership in lock-step with the SQL writes done by
    // `api_startups::{create_startup,delete_startup}`. Without this, every
    // suite stays public to `move_sys::can_enter_layout_room` for the lifetime
    // of the process.
    let mut layout = layout;

    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Tick => {
                    w.tick_seq = w.tick_seq.wrapping_add(1);
                    let events = move_sys::step_all(&mut w, &mut paths);
                    for e in events {
                        match e {
                            MoveEvent::Complete { agent_id, room_id } => {
                                if let Some(tx) = out_bus.get(&agent_id) {
                                    let payload = serde_json::json!({
                                        "type":"move_complete","v":1,"room_id":room_id
                                    });
                                    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                                        tx.try_send(payload)
                                    {
                                        tracing::warn!(component = "loop", agent_id = %agent_id, "out_bus full, dropping move_complete");
                                    }
                                }
                            }
                        }
                    }
                    let _ = crate::scheduler::tick(
                        &mut w, &mut paths, &layout, &graph, &out_bus, &pool,
                    )
                    .await;
                    crate::proximity::compute_and_emit(&w, &out_bus);
                    let _ = view_tx.send(w.clone());
                }
                Cmd::HandleConsoleMsg { msg, reply } => {
                    let result = crate::cmd_console::dispatch(&mut w, &pool, &out_bus, msg).await;
                    let _ = view_tx.send(w.clone());
                    let _ = reply.send(result);
                }
                Cmd::HandleWorkerMsg { agent_id, msg, reply } => {
                    let result = crate::cmd_worker::dispatch(
                        &mut w, &mut paths, &layout, &graph, &out_bus, &pool, &agent_id, msg,
                    )
                    .await;
                    let _ = view_tx.send(w.clone());
                    let _ = reply.send(result);
                }
                Cmd::RegisterWorker { agent_id, tx: out_tx } => {
                    out_bus.insert(agent_id, out_tx);
                }
                Cmd::UnregisterWorker { agent_id } => {
                    out_bus.remove(&agent_id);
                }
                Cmd::BackendCatalogUpdated(c) => {
                    w.backend_catalog = c;
                    let _ = view_tx.send(w.clone());
                }
                Cmd::InsertAvatars { avatars, claim_suite } => {
                    for a in avatars {
                        w.avatars.insert(a.agent_id.clone(), a);
                    }
                    if let Some((suite_id, startup_id)) = claim_suite {
                        if let Some(room) =
                            layout.rooms.iter_mut().find(|r| r.id == suite_id)
                        {
                            room.private_to_startup_id = Some(startup_id);
                        }
                    }
                    let _ = view_tx.send(w.clone());
                }
                Cmd::ReleaseSuite { startup_id } => {
                    for room in layout.rooms.iter_mut() {
                        if room.private_to_startup_id.as_deref() == Some(startup_id.as_str()) {
                            room.private_to_startup_id = None;
                        }
                    }
                    // Drop avatars belonging to the dissolved startup so console
                    // snapshots, proximity, and the scheduler stop seeing ghost
                    // agents. Without this, `DELETE /api/startups/:id` only
                    // freed the suite — the dissolved startup's avatars
                    // lingered in `w.avatars` until process restart.
                    w.avatars.retain(|_, a| a.startup_id != startup_id);
                    let _ = view_tx.send(w.clone());
                }
                Cmd::Shutdown => break,
            }
        }
    });

    let timer_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            if timer_tx.send(Cmd::Tick).await.is_err() {
                break;
            }
        }
    });

    Handle { tx, view_rx }
}
