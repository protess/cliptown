use crate::state::WorldView;
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Debug)]
pub enum Cmd {
    Tick,
    HandleConsoleMsg { msg: serde_json::Value, reply: oneshot::Sender<serde_json::Value> },
    HandleWorkerMsg { agent_id: String, msg: serde_json::Value, reply: oneshot::Sender<serde_json::Value> },
    BackendCatalogUpdated(std::collections::HashMap<String, serde_json::Value>),
    Shutdown,
}

#[derive(Clone)]
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
    pub view_rx: watch::Receiver<WorldView>,
}

pub fn spawn(initial: WorldView) -> Handle {
    let (tx, mut rx) = mpsc::channel::<Cmd>(1024);
    let (view_tx, view_rx) = watch::channel(initial.clone());
    let mut w = initial;
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Tick => { w.tick_seq = w.tick_seq.wrapping_add(1); let _ = view_tx.send(w.clone()); }
                Cmd::HandleConsoleMsg { msg: _, reply } => { let _ = reply.send(serde_json::json!({"ok": true})); }
                Cmd::HandleWorkerMsg { agent_id: _, msg: _, reply } => { let _ = reply.send(serde_json::json!({"ok": true})); }
                Cmd::BackendCatalogUpdated(c) => { w.backend_catalog = c; let _ = view_tx.send(w.clone()); }
                Cmd::Shutdown => break,
            }
        }
    });
    let timer_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop { interval.tick().await; if timer_tx.send(Cmd::Tick).await.is_err() { break; } }
    });
    Handle { tx, view_rx }
}
