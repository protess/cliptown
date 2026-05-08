use cliptown_world::{
    proximity,
    state::{AvatarView, WorldView},
};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc;

fn av(id: &str, sid: &str, role: &str, room: &str) -> AvatarView {
    AvatarView {
        agent_id: id.into(),
        startup_id: sid.into(),
        role: role.into(),
        backend: "claude_code".into(),
        current_pos: (3, 3),
        target_pos: None,
        room_id: room.into(),
        status: "idle".into(),
    }
}

#[tokio::test]
async fn each_member_sees_full_room_roster() {
    let mut w = WorldView::default();
    w.avatars.insert("a".into(), av("a", "s1", "engineer", "lobby"));
    w.avatars.insert("b".into(), av("b", "s2", "founder", "lobby"));
    w.avatars
        .insert("c".into(), av("c", "s1", "designer", "suite_1")); // different room

    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (tx_a, mut rx_a) = mpsc::channel(8);
    let (tx_b, mut rx_b) = mpsc::channel(8);
    let (tx_c, mut rx_c) = mpsc::channel(8);
    out_bus.insert("a".into(), tx_a);
    out_bus.insert("b".into(), tx_b);
    out_bus.insert("c".into(), tx_c);

    proximity::compute_and_emit(&w, &out_bus);

    // a + b are in lobby — both see each other plus self.
    let m_a = rx_a.try_recv().unwrap();
    assert_eq!(m_a["type"], "proximity_tick");
    assert_eq!(m_a["room_id"], "lobby");
    let peers_a = m_a["peers"].as_array().unwrap();
    assert_eq!(peers_a.len(), 2);
    let ids_a: Vec<&str> = peers_a
        .iter()
        .map(|p| p["agent_id"].as_str().unwrap())
        .collect();
    assert!(ids_a.contains(&"a"));
    assert!(ids_a.contains(&"b"));

    let m_b = rx_b.try_recv().unwrap();
    assert_eq!(m_b["room_id"], "lobby");

    // c is alone in suite_1 — sees only self.
    let m_c = rx_c.try_recv().unwrap();
    assert_eq!(m_c["room_id"], "suite_1");
    let peers_c = m_c["peers"].as_array().unwrap();
    assert_eq!(peers_c.len(), 1);
    assert_eq!(peers_c[0]["agent_id"], "c");
}

#[tokio::test]
async fn cross_startup_peers_included_in_public_room() {
    let mut w = WorldView::default();
    w.avatars.insert(
        "alpha_eng".into(),
        av("alpha_eng", "alpha", "engineer", "cafe"),
    );
    w.avatars.insert(
        "beta_des".into(),
        av("beta_des", "beta", "designer", "cafe"),
    );

    let mut out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    let (tx, mut rx) = mpsc::channel(8);
    out_bus.insert("alpha_eng".into(), tx);

    proximity::compute_and_emit(&w, &out_bus);

    let msg = rx.try_recv().unwrap();
    let peers = msg["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 2);
    let startups: Vec<&str> = peers
        .iter()
        .map(|p| p["startup_id"].as_str().unwrap())
        .collect();
    assert!(startups.contains(&"alpha"));
    assert!(startups.contains(&"beta"));
}

#[tokio::test]
async fn empty_world_no_emit() {
    let w = WorldView::default();
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    // Should not panic.
    proximity::compute_and_emit(&w, &out_bus);
}

#[tokio::test]
async fn agent_without_out_bus_entry_skipped() {
    let mut w = WorldView::default();
    w.avatars.insert("a".into(), av("a", "s1", "engineer", "lobby"));
    let out_bus: HashMap<String, mpsc::Sender<Value>> = HashMap::new();
    // No bus entry — no panic, no emit.
    proximity::compute_and_emit(&w, &out_bus);
}
