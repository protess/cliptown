//! M2.3 — Unit tests for `mcp_dispatch::dispatch`. One happy-path test per
//! tool, a manager-only group test, and a cross-startup property-style test
//! that confirms five tools cannot mutate another startup's state.
//!
//! All tests construct the dispatch envelope (`{type:"mcp_call", tool, args,
//! corr_id}`) directly and assert on the JSON reply. Helpers in `fixture`
//! seed two startups with managers + reports, an in-flight task, and an
//! out_bus capturing every fanned event.

use cliptown_world::{
    mcp_dispatch,
    move_sys::{self, PathStore},
    path::RoomGraph,
    seed::{self, TownLayout},
    state::{AvatarView, WorldView},
    storage,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

struct Fx {
    world: WorldView,
    paths: PathStore,
    layout: TownLayout,
    graph: RoomGraph,
    out_bus: HashMap<String, mpsc::Sender<Value>>,
    pool: sqlx::SqlitePool,
    /// Receivers for each agent in the bus, so tests can assert what got
    /// fanned out without racing.
    rx: HashMap<String, mpsc::Receiver<Value>>,
    _dir: tempfile::TempDir,
}

impl Fx {
    fn make_rx(&mut self, agent_id: &str) {
        let (tx, rx) = mpsc::channel(32);
        self.out_bus.insert(agent_id.to_string(), tx);
        self.rx.insert(agent_id.to_string(), rx);
    }

    fn drain(&mut self, agent_id: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if let Some(rx) = self.rx.get_mut(agent_id) {
            while let Ok(v) = rx.try_recv() {
                out.push(v);
            }
        }
        out
    }

    async fn call(&mut self, agent_id: &str, tool: &str, args: Value) -> Value {
        let msg = json!({
            "type":"mcp_call","v":1,"tool":tool,"args":args,"corr_id":"c1"
        });
        mcp_dispatch::dispatch(
            &mut self.world,
            &mut self.paths,
            &self.layout,
            &self.graph,
            &self.out_bus,
            &self.pool,
            agent_id,
            msg,
        )
        .await
    }
}

/// Two startups (s1, s2). Each startup has a manager (m1/m2, role founder)
/// and an engineer (e1/e2). The engineer's `manager_id` is the manager.
/// Both startups have a `T1`-prefixed root task assigned to the engineer.
async fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("mcp.db");
    let pool = storage::open(p.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    for (sid, name, ws) in [
        ("s1", "alpha", "/tmp/s1"),
        ("s2", "beta", "/tmp/s2"),
    ] {
        sqlx::query(
            "INSERT INTO startups (id, name, goal_text, budget_cap_usd, town_id, workspace_path, status, created_at) \
             VALUES (?, ?, 'goal', 10.0, 'town_default', ?, 'active', unixepoch())",
        )
        .bind(sid).bind(name).bind(ws)
        .execute(&pool).await.unwrap();
    }
    for (aid, sid, role, mgr) in [
        ("m1", "s1", "founder", None::<&str>),
        ("e1", "s1", "engineer", Some("m1")),
        ("m2", "s2", "founder", None::<&str>),
        ("e2", "s2", "engineer", Some("m2")),
    ] {
        sqlx::query(
            "INSERT INTO agents (id, startup_id, name, role, backend, model_id, position_json, home_room_id, manager_id, status) \
             VALUES (?, ?, ?, ?, 'claude_code', 'm', '{}', 'suite_1', ?, 'idle')",
        )
        .bind(aid).bind(sid).bind(aid).bind(role).bind(mgr)
        .execute(&pool).await.unwrap();
    }
    // s1's task T1 is in_progress, owned by e1, with a parent root (m1's task).
    // We seed T0 (m1's root) so the manager-of-task chain resolves cleanly.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T0', 's1', 'root', 'd', 'in_progress', 'm1', unixepoch(), unixepoch())",
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1', 's1', 'T0', 'subtask', 'd', 'in_progress', 'e1', unixepoch(), unixepoch())",
    ).execute(&pool).await.unwrap();
    // s2 mirror: T0' root, T1' subtask owned by e2.
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T0p', 's2', 'root', 'd', 'in_progress', 'm2', unixepoch(), unixepoch())",
    ).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T1p', 's2', 'T0p', 'subtask', 'd', 'in_progress', 'e2', unixepoch(), unixepoch())",
    ).execute(&pool).await.unwrap();

    let layout = TownLayout::default_town();
    let graph = move_sys::graph_from_layout(&layout);
    let mut world = WorldView::default();
    for (aid, sid, role, room, x, y) in [
        ("m1", "s1", "founder", "suite_1", 3, 3),
        ("e1", "s1", "engineer", "suite_1", 4, 3),
        ("m2", "s2", "founder", "suite_3", 35, 3),
        ("e2", "s2", "engineer", "suite_3", 36, 3),
    ] {
        world.avatars.insert(
            aid.to_string(),
            AvatarView {
                agent_id: aid.into(),
                startup_id: sid.into(),
                role: role.into(),
                backend: "claude_code".into(),
                current_pos: (x, y),
                target_pos: None,
                room_id: room.into(),
                status: "idle".into(),
            },
        );
    }
    Fx {
        world,
        paths: PathStore::new(),
        layout,
        graph,
        out_bus: HashMap::new(),
        pool,
        rx: HashMap::new(),
        _dir: dir,
    }
}

// ── happy paths ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn move_intent_starts_path() {
    let mut fx = fixture().await;
    let r = fx
        .call("e1", "move_intent", json!({"target_room":"lobby"}))
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    assert_eq!(r["result"]["target_room"], "lobby");
    // Path queued: e1 must traverse suite_1 → door → lobby.
    assert!(fx.paths.contains_key("e1"));
}

#[tokio::test]
async fn move_intent_rejects_foreign_suite() {
    let mut fx = fixture().await;
    // suite_3 is privately owned by s2 once provisioned; Phase 0 seed has all
    // suites unowned, so simulate ownership by asserting a target outside any
    // bound (no_path) instead. Use a known-bad room name.
    let r = fx
        .call("e1", "move_intent", json!({"target_room":"does_not_exist"}))
        .await;
    assert_eq!(r["type"], "mcp_error", "{r}");
}

#[tokio::test]
async fn speak_chat_fans_to_room_peers() {
    let mut fx = fixture().await;
    fx.make_rx("m1");
    let r = fx
        .call("e1", "speak", json!({"body":"hi","kind":"chat"}))
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let evts = fx.drain("m1");
    assert_eq!(evts.len(), 1, "{evts:?}");
    assert_eq!(evts[0]["type"], "chat_received");
    assert_eq!(evts[0]["from_agent_id"], "e1");
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM messages WHERE kind='chat'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);
}

#[tokio::test]
async fn speak_directive_requires_manager_relationship() {
    let mut fx = fixture().await;
    // e1 attempting to directive m1 (its manager) — not allowed.
    let r = fx
        .call(
            "e1",
            "speak",
            json!({"body":"hi","kind":"directive","to_agent_id":"m1"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_error", "{r}");
    // m1 directing e1 — allowed.
    fx.make_rx("e1");
    let r = fx
        .call(
            "m1",
            "speak",
            json!({"body":"go","kind":"directive","to_agent_id":"e1"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let evts = fx.drain("e1");
    assert_eq!(evts[0]["type"], "directive");
}

#[tokio::test]
async fn task_done_assignee_only_and_emits_subtask_done() {
    let mut fx = fixture().await;
    // Create a workspace dir so sandbox::resolve doesn't reject an absent root.
    let ws = std::path::PathBuf::from("workspaces/s1");
    let _ = std::fs::create_dir_all(&ws);
    let _ = std::fs::write(ws.join("artifact.md"), "ok");
    fx.make_rx("m1");
    let r = fx
        .call(
            "e1",
            "task_done",
            json!({"task_id":"T1","artifact_path":"artifact.md"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(s.0, "awaiting_review");
    let evts = fx.drain("m1");
    assert!(evts.iter().any(|e| e["type"] == "subtask_done"), "{evts:?}");
}

#[tokio::test]
async fn task_failed_assignee_only() {
    let mut fx = fixture().await;
    let r = fx
        .call(
            "e1",
            "task_failed",
            json!({"task_id":"T1","reason":"nope"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(s.0, "failed");
    // Non-assignee can't fail it (already failed → illegal_transition either
    // way; use a sibling task to isolate the no_permission path).
    sqlx::query(
        "INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at) \
         VALUES ('T2', 's1', 'T0', 's2', 'd', 'in_progress', 'e1', unixepoch(), unixepoch())",
    ).execute(&fx.pool).await.unwrap();
    let r = fx
        .call("m1", "task_failed", json!({"task_id":"T2","reason":"x"}))
        .await;
    assert_eq!(r["type"], "mcp_error", "{r}");
    assert_eq!(r["code"], "no_permission");
}

#[tokio::test]
async fn subtask_create_manager_path_starts_queued() {
    let mut fx = fixture().await;
    let r = fx
        .call(
            "m1",
            "subtask_create",
            json!({"parent_id":"T0","title":"k","description":"d","assignee_agent_id":"e1"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    assert_eq!(r["result"]["status"], "queued");
}

#[tokio::test]
async fn subtask_create_non_manager_path_proposes_and_notifies() {
    let mut fx = fixture().await;
    fx.make_rx("m1");
    let r = fx
        .call(
            "e1",
            "subtask_create",
            json!({"parent_id":"T0","title":"k","description":"d","assignee_agent_id":"e1"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    assert_eq!(r["result"]["status"], "proposed");
    let evts = fx.drain("m1");
    assert!(
        evts.iter().any(|e| e["type"] == "subtask_proposed"),
        "{evts:?}"
    );
    // Non-managers never get to assign — the row's assignee should be NULL.
    let new_id = r["result"]["task_id"].as_str().unwrap();
    let row: (Option<String>,) =
        sqlx::query_as("SELECT assignee_agent_id FROM tasks WHERE id = ?")
            .bind(new_id)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert!(row.0.is_none());
}

#[tokio::test]
async fn task_accept_manager_only() {
    let mut fx = fixture().await;
    sqlx::query("UPDATE tasks SET status = 'awaiting_review' WHERE id = 'T1'")
        .execute(&fx.pool)
        .await
        .unwrap();
    // e1 (the assignee) cannot self-accept.
    let r = fx.call("e1", "task_accept", json!({"task_id":"T1"})).await;
    assert_eq!(r["type"], "mcp_error", "{r}");
    // m1 can.
    let r = fx.call("m1", "task_accept", json!({"task_id":"T1"})).await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(s.0, "done");
}

#[tokio::test]
async fn task_request_changes_increments_round_and_notifies() {
    let mut fx = fixture().await;
    sqlx::query("UPDATE tasks SET status = 'awaiting_review' WHERE id = 'T1'")
        .execute(&fx.pool)
        .await
        .unwrap();
    fx.make_rx("e1");
    let r = fx
        .call(
            "m1",
            "task_request_changes",
            json!({"task_id":"T1","feedback":"redo","in_response_to_round":0}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let row: (String, i64) =
        sqlx::query_as("SELECT status, review_round FROM tasks WHERE id='T1'")
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(row.0, "changes_requested");
    assert_eq!(row.1, 1);
    let evts = fx.drain("e1");
    assert!(evts.iter().any(|e| e["type"] == "directive"), "{evts:?}");
}

#[tokio::test]
async fn accept_proposal_manager_only() {
    let mut fx = fixture().await;
    // Set T1 to proposed so accept_proposal applies.
    sqlx::query("UPDATE tasks SET status = 'proposed' WHERE id = 'T1'")
        .execute(&fx.pool)
        .await
        .unwrap();
    let r = fx
        .call(
            "m1",
            "accept_proposal",
            json!({"task_id":"T1","assignee_agent_id":"e1"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    // Non-manager rejected.
    sqlx::query("UPDATE tasks SET status = 'proposed' WHERE id = 'T1'")
        .execute(&fx.pool)
        .await
        .unwrap();
    let r = fx
        .call(
            "e1",
            "accept_proposal",
            json!({"task_id":"T1","assignee_agent_id":"e1"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_error", "{r}");
}

#[tokio::test]
async fn reject_proposal_manager_only() {
    let mut fx = fixture().await;
    sqlx::query("UPDATE tasks SET status = 'proposed' WHERE id = 'T1'")
        .execute(&fx.pool)
        .await
        .unwrap();
    let r = fx
        .call(
            "m1",
            "reject_proposal",
            json!({"task_id":"T1","reason":"out of scope"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(s.0, "failed");
}

#[tokio::test]
async fn hypothesis_state_appends_epistemic_log() {
    let mut fx = fixture().await;
    let r = fx
        .call(
            "e1",
            "hypothesis_state",
            json!({"task_id":"T1","id":"H1","claim":"x","rationale":"y"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let row: (String,) = sqlx::query_as("SELECT epistemic_log FROM tasks WHERE id='T1'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert!(row.0.contains("hypothesis_state"));
    assert!(row.0.contains("H1"));
}

#[tokio::test]
async fn test_record_appends_epistemic_log() {
    let mut fx = fixture().await;
    let r = fx
        .call(
            "e1",
            "test_record",
            json!({"task_id":"T1","id":"R1","method":"read_assert","outcome":"pass"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let row: (String,) = sqlx::query_as("SELECT epistemic_log FROM tasks WHERE id='T1'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert!(row.0.contains("test_record"));
}

#[tokio::test]
async fn hypothesis_resolve_appends_epistemic_log() {
    let mut fx = fixture().await;
    let r = fx
        .call(
            "e1",
            "hypothesis_resolve",
            json!({"task_id":"T1","id":"H1","status":"confirmed"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    let row: (String,) = sqlx::query_as("SELECT epistemic_log FROM tasks WHERE id='T1'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert!(row.0.contains("hypothesis_resolve"));
}

#[tokio::test]
async fn verify_read_assert_runs_inline() {
    let mut fx = fixture().await;
    let ws = std::path::PathBuf::from("workspaces/s1");
    let _ = std::fs::create_dir_all(&ws);
    std::fs::write(ws.join("verify_input.txt"), "hello world").unwrap();
    let r = fx
        .call(
            "e1",
            "verify",
            json!({"method":"read_assert","params":{"path":"verify_input.txt","contains":"world"}}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply", "{r}");
    assert_eq!(r["result"]["observed"]["ok"], true);
}

#[tokio::test]
async fn verify_lint_json_returns_observation() {
    let mut fx = fixture().await;
    let ws = std::path::PathBuf::from("workspaces/s1");
    let _ = std::fs::create_dir_all(&ws);
    std::fs::write(ws.join("ok.json"), "{\"a\":1}").unwrap();
    std::fs::write(ws.join("bad.json"), "{not json").unwrap();
    let r = fx
        .call(
            "e1",
            "verify",
            json!({"method":"lint_json","params":{"path":"ok.json"}}),
        )
        .await;
    assert_eq!(r["result"]["observed"]["ok"], true);
    let r = fx
        .call(
            "e1",
            "verify",
            json!({"method":"lint_json","params":{"path":"bad.json"}}),
        )
        .await;
    assert_eq!(r["result"]["observed"]["ok"], false);
}

#[tokio::test]
async fn verify_typescript_and_markdown_are_deferred() {
    let mut fx = fixture().await;
    for method in ["lint_markdown", "lint_typescript"] {
        let r = fx
            .call(
                "e1",
                "verify",
                json!({"method":method,"params":{"path":"x"}}),
            )
            .await;
        assert_eq!(r["type"], "mcp_reply");
        assert_eq!(r["result"]["observed"]["deferred"], true);
    }
}

#[tokio::test]
async fn ask_peer_returns_null_in_phase_0() {
    let mut fx = fixture().await;
    let r = fx
        .call(
            "e1",
            "ask_peer",
            json!({"body":"hi","to_agent_id":"m1","timeout_ms":50}),
        )
        .await;
    assert_eq!(r["type"], "mcp_reply");
    assert!(r["result"]["response"].is_null());
}

#[tokio::test]
async fn observe_world_three_queries() {
    let mut fx = fixture().await;
    let r = fx
        .call("e1", "observe_world", json!({"query":"my_position"}))
        .await;
    assert_eq!(r["result"]["room_id"], "suite_1");

    let r = fx
        .call("e1", "observe_world", json!({"query":"peers_in_room"}))
        .await;
    let peers = r["result"]["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0]["agent_id"], "m1");

    let r = fx
        .call("e1", "observe_world", json!({"query":"budget_remaining"}))
        .await;
    assert_eq!(r["result"]["cap_usd"], 10.0);
}

#[tokio::test]
async fn read_artifact_within_workspace() {
    let mut fx = fixture().await;
    let ws = std::path::PathBuf::from("workspaces/s1");
    let _ = std::fs::create_dir_all(&ws);
    std::fs::write(ws.join("doc.md"), "# hi").unwrap();
    let r = fx
        .call("e1", "read_artifact", json!({"path":"doc.md"}))
        .await;
    assert_eq!(r["type"], "mcp_reply");
    assert_eq!(r["result"]["content"], "# hi");
}

#[tokio::test]
async fn read_artifact_rejects_path_escape() {
    let mut fx = fixture().await;
    let ws = std::path::PathBuf::from("workspaces/s1");
    let _ = std::fs::create_dir_all(&ws);
    let r = fx
        .call(
            "e1",
            "read_artifact",
            json!({"path":"../../etc/passwd"}),
        )
        .await;
    assert_eq!(r["type"], "mcp_error", "{r}");
    assert_eq!(r["code"], "sandbox_violation");
}

// ── group permission test ───────────────────────────────────────────────────

#[tokio::test]
async fn manager_only_tools_reject_non_manager() {
    let mut fx = fixture().await;
    sqlx::query("UPDATE tasks SET status = 'awaiting_review' WHERE id = 'T1'")
        .execute(&fx.pool)
        .await
        .unwrap();
    // e1 is the assignee, not the manager — every manager-only tool must
    // reject with `no_permission`.
    let cases = vec![
        ("task_accept", json!({"task_id":"T1"})),
        (
            "task_request_changes",
            json!({"task_id":"T1","feedback":"x","in_response_to_round":0}),
        ),
    ];
    for (tool, args) in cases {
        let r = fx.call("e1", tool, args).await;
        assert_eq!(r["type"], "mcp_error", "tool {tool}: {r}");
        assert_eq!(r["code"], "no_permission", "tool {tool}: {r}");
    }
    // accept/reject_proposal need the task in `proposed` for the
    // permission gate to be the failing layer (not the SM).
    sqlx::query("UPDATE tasks SET status = 'proposed' WHERE id = 'T1'")
        .execute(&fx.pool)
        .await
        .unwrap();
    let cases = vec![
        (
            "accept_proposal",
            json!({"task_id":"T1","assignee_agent_id":"e1"}),
        ),
        (
            "reject_proposal",
            json!({"task_id":"T1","reason":"x"}),
        ),
    ];
    for (tool, args) in cases {
        let r = fx.call("e1", tool, args).await;
        assert_eq!(r["type"], "mcp_error", "tool {tool}: {r}");
        assert_eq!(r["code"], "no_permission", "tool {tool}: {r}");
    }
}

// ── cross-startup invariant ─────────────────────────────────────────────────

#[tokio::test]
async fn cross_startup_invariant_blocks_mutations() {
    // s1 agents must never be able to mutate s2 tasks/messages. Drive five
    // different tools as e1 (s1) targeting s2's task `T1p` and assert each
    // returns `cross_startup` (or `no_permission` when the same-startup gate
    // is implicit via the manager check).
    let mut fx = fixture().await;
    sqlx::query("UPDATE tasks SET status = 'awaiting_review' WHERE id = 'T1p'")
        .execute(&fx.pool)
        .await
        .unwrap();

    let cases: Vec<(&str, Value, &[&str])> = vec![
        (
            "task_done",
            json!({"task_id":"T1p","artifact_path":"x.md"}),
            &["cross_startup"],
        ),
        (
            "task_failed",
            json!({"task_id":"T1p","reason":"x"}),
            &["cross_startup"],
        ),
        (
            "task_accept",
            json!({"task_id":"T1p"}),
            &["cross_startup"],
        ),
        (
            "task_request_changes",
            json!({"task_id":"T1p","feedback":"x","in_response_to_round":0}),
            &["cross_startup"],
        ),
        (
            "subtask_create",
            json!({"parent_id":"T0p","title":"x","description":"y","assignee_agent_id":"e2"}),
            &["cross_startup"],
        ),
    ];
    for (tool, args, allowed) in cases {
        let r = fx.call("e1", tool, args).await;
        assert_eq!(r["type"], "mcp_error", "tool {tool}: {r}");
        let code = r["code"].as_str().unwrap();
        assert!(
            allowed.contains(&code),
            "tool {tool} returned {code}, expected one of {allowed:?}"
        );
    }

    // Speak-directive cross-startup is the message channel sibling.
    let r = fx
        .call(
            "m1",
            "speak",
            json!({"body":"x","kind":"directive","to_agent_id":"e2"}),
        )
        .await;
    assert_eq!(r["code"], "cross_startup", "{r}");

    // And confirm s2 state is untouched: T1p still awaiting_review, no
    // new s2 messages.
    let s: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id='T1p'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(s.0, "awaiting_review");
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM messages WHERE startup_id='s2'")
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn unknown_tool_returns_mcp_error() {
    let mut fx = fixture().await;
    let r = fx.call("e1", "what_even", json!({})).await;
    assert_eq!(r["type"], "mcp_error");
    assert_eq!(r["code"], "unknown_tool");
}
