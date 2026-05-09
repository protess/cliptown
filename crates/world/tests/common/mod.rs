//! Shared test fixture for crates/world tests. Bundles pool, out_bus, and the
//! broadcast event channel that production dispatch handlers expect, so each
//! test can assert "this dispatch emitted exactly these console frames" without
//! repeating the channel-setup boilerplate.

#![allow(dead_code)]  // Some helpers used by only a subset of tests.

use cliptown_world::{protocol::ConsoleOutbound, storage};
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};

pub struct TestCtx {
    pub pool: SqlitePool,
    pub out_bus: HashMap<String, mpsc::Sender<serde_json::Value>>,
    pub event_tx: broadcast::Sender<ConsoleOutbound>,
    pub event_rx: broadcast::Receiver<ConsoleOutbound>,
    _dir: tempfile::TempDir,
}

impl TestCtx {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.db");
        let pool = storage::open(p.to_str().unwrap()).await.unwrap();
        cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
        let (event_tx, event_rx) = broadcast::channel(64);
        TestCtx {
            pool,
            out_bus: HashMap::new(),
            event_tx,
            event_rx,
            _dir: dir,
        }
    }

    /// Asserts no console frames were broadcast since the last drain. Drains
    /// any remaining events. Use in tests that are NOT asserting emission to
    /// catch accidental new emit sites.
    pub fn expect_no_broadcasts(&mut self) {
        let mut found = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(frame) => found.push(frame),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
        assert!(
            found.is_empty(),
            "expected no console broadcasts, found {} frame(s): {:?}",
            found.len(),
            found
        );
    }

    /// Drains all queued frames and returns them. Use to verify the exact
    /// number/shape of emissions from a single dispatch call.
    pub fn drain_broadcasts(&mut self) -> Vec<ConsoleOutbound> {
        let mut out = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(frame) => out.push(frame),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
        out
    }

    /// Convenience: drain and assert exactly one frame.
    pub fn expect_one_broadcast(&mut self) -> ConsoleOutbound {
        let frames = self.drain_broadcasts();
        assert_eq!(frames.len(), 1, "expected exactly 1 broadcast, got {}: {:?}", frames.len(), frames);
        frames.into_iter().next().unwrap()
    }
}
