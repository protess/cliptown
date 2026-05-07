use crate::move_sys::{self, MoveEvent, PathStore};
use crate::path::RoomGraph;
use crate::seed::TownLayout;
use crate::state::WorldView;
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
                                        tracing::warn!(agent_id = %agent_id, "out_bus full, dropping move_complete");
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
