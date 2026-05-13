//! P2.1 integration tests — boot loop_::spawn, exercise Cmd handlers,
//! observe last_seen_at + health transitions via view_rx.

use cliptown_world::health::Health;
use cliptown_world::loop_::{self, Cmd};
use cliptown_world::state::{AvatarView, WorldView};
use cliptown_world::storage;
use tokio::sync::{broadcast, mpsc, oneshot};

async fn boot() -> (loop_::Handle, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = storage::open(dir.path().join("test.db").to_str().unwrap())
        .await
        .unwrap();
    cliptown_world::seed::seed_if_empty(&pool).await.unwrap();
    let (event_tx, _event_rx) = broadcast::channel(64);
    let handle = loop_::spawn(WorldView::default(), pool, event_tx);
    (handle, dir)
}

fn fake_avatar(agent_id: &str, role: &str) -> AvatarView {
    AvatarView {
        agent_id: agent_id.to_string(),
        startup_id: "s1".to_string(),
        role: role.to_string(),
        backend: "claude_code".to_string(),
        current_pos: (0, 0),
        target_pos: None,
        room_id: "lobby".to_string(),
        status: "idle".to_string(),
        last_seen_at: None,
        health: Health::Offline,
    }
}

async fn snapshot(handle: &loop_::Handle) -> WorldView {
    // Use Tick as a sync barrier: send it into the FIFO channel, then wait
    // for the resulting view_tx.send (Tick always broadcasts).  All commands
    // sent before this call are guaranteed to be processed first.
    //
    // We use HandleConsoleMsg here (not a raw Tick) so we have a reply
    // rendezvous — once the reply arrives we know the loop has flushed all
    // prior commands AND published a fresh view snapshot.
    let (reply_tx, reply_rx) = oneshot::channel();
    handle
        .tx
        .send(Cmd::HandleConsoleMsg {
            msg: serde_json::json!({"type": "ping"}),
            identity: cliptown_world::auth::OperatorIdentity::admin_for_tests(),
            reply: reply_tx,
        })
        .await
        .unwrap();
    // Await the reply; by then view_tx.send has been called after all prior cmds.
    let _ = reply_rx.await;
    let view = handle.view_rx.borrow().clone();
    view
}

#[tokio::test]
async fn register_sets_last_seen_and_marks_online() {
    let (handle, _dir) = boot().await;
    handle
        .tx
        .send(Cmd::InsertAvatars {
            avatars: vec![fake_avatar("a1", "engineer")],
            claim_suite: None,
        })
        .await
        .unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<serde_json::Value>(8);
    handle
        .tx
        .send(Cmd::RegisterWorker {
            agent_id: "a1".to_string(),
            tx: out_tx,
        })
        .await
        .unwrap();

    let view = snapshot(&handle).await;
    let av = view.avatars.get("a1").expect("avatar a1 present");
    assert!(av.last_seen_at.is_some(), "register should set last_seen_at");
    assert_eq!(av.health, Health::Online);
}

#[tokio::test]
async fn handle_msg_refreshes_last_seen() {
    let (handle, _dir) = boot().await;
    handle
        .tx
        .send(Cmd::InsertAvatars {
            avatars: vec![fake_avatar("a1", "engineer")],
            claim_suite: None,
        })
        .await
        .unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<serde_json::Value>(8);
    handle
        .tx
        .send(Cmd::RegisterWorker {
            agent_id: "a1".to_string(),
            tx: out_tx,
        })
        .await
        .unwrap();
    let v0 = snapshot(&handle).await;
    let ts0 = v0.avatars["a1"].last_seen_at.unwrap();

    // Wait long enough for unix_now() to advance by at least 1 second.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let (reply_tx, _reply_rx) = oneshot::channel();
    handle
        .tx
        .send(Cmd::HandleWorkerMsg {
            agent_id: "a1".to_string(),
            msg: serde_json::json!({"type":"noop"}),
            reply: reply_tx,
        })
        .await
        .unwrap();

    let v1 = snapshot(&handle).await;
    let ts1 = v1.avatars["a1"].last_seen_at.unwrap();
    assert!(
        ts1 > ts0,
        "HandleWorkerMsg should refresh last_seen_at (was {}, now {})",
        ts0,
        ts1
    );
    assert_eq!(v1.avatars["a1"].health, Health::Online);
}

#[tokio::test]
async fn unregister_preserves_last_seen_marks_recently_lost() {
    let (handle, _dir) = boot().await;
    handle
        .tx
        .send(Cmd::InsertAvatars {
            avatars: vec![fake_avatar("a1", "engineer")],
            claim_suite: None,
        })
        .await
        .unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<serde_json::Value>(8);
    handle
        .tx
        .send(Cmd::RegisterWorker {
            agent_id: "a1".to_string(),
            tx: out_tx,
        })
        .await
        .unwrap();
    let v0 = snapshot(&handle).await;
    let ts0 = v0.avatars["a1"].last_seen_at.unwrap();

    handle
        .tx
        .send(Cmd::UnregisterWorker {
            agent_id: "a1".to_string(),
        })
        .await
        .unwrap();

    let v1 = snapshot(&handle).await;
    let av = v1.avatars.get("a1").expect("avatar still present");
    // last_seen_at must be preserved (>= ts0; time may have advanced via
    // the post-unregister tick refresh, but no Cmd has touched the field).
    assert_eq!(av.last_seen_at, Some(ts0), "last_seen_at preserved across unregister");
    // Disconnected just now → still inside the recently_lost window.
    assert_eq!(av.health, Health::RecentlyLost);
}
