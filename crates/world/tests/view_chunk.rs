use cliptown_world::state::{AvatarView, WorldView};
use cliptown_world::view::{
    build_worker_snapshot, chunk_snapshot, reassemble, ChunkFrame, MessageStub, MESSAGE_CAP,
    PEER_CAP,
};

fn avatar(agent_id: &str, startup_id: &str, room: &str) -> AvatarView {
    AvatarView {
        agent_id: agent_id.to_string(),
        startup_id: startup_id.to_string(),
        role: "engineer".to_string(),
        backend: "claude_code".to_string(),
        current_pos: (0, 0),
        target_pos: None,
        room_id: room.to_string(),
        status: "idle".to_string(),
        last_seen_at: None,
        health: cliptown_world::health::Health::Offline,
    }
}

#[test]
fn worker_snapshot_includes_own_avatar() {
    let mut world = WorldView::default();
    world.avatars.insert("a1".into(), avatar("a1", "s1", "lobby"));
    let snap = build_worker_snapshot(&world, "a1", "s1", vec![], None);
    assert!(snap.own_avatar.is_some());
    assert_eq!(snap.own_avatar.unwrap().agent_id, "a1");
}

#[test]
fn worker_snapshot_lists_only_same_room_peers() {
    let mut world = WorldView::default();
    world.avatars.insert("a1".into(), avatar("a1", "s1", "lobby"));
    world.avatars.insert("a2".into(), avatar("a2", "s1", "lobby")); // same room, same startup
    world.avatars.insert("a3".into(), avatar("a3", "s2", "lobby")); // same room, different startup
    world
        .avatars
        .insert("a4".into(), avatar("a4", "s1", "library")); // different room
    let snap = build_worker_snapshot(&world, "a1", "s1", vec![], None);
    let peer_ids: Vec<&str> = snap
        .peers_in_room
        .iter()
        .map(|a| a.agent_id.as_str())
        .collect();
    assert!(peer_ids.contains(&"a2"));
    assert!(peer_ids.contains(&"a3")); // cross-startup peers in common rooms ARE visible (invariant 7 substrate)
    assert!(!peer_ids.contains(&"a4"));
    assert!(!peer_ids.contains(&"a1")); // not own
}

#[test]
fn worker_snapshot_caps_peers_at_16() {
    let mut world = WorldView::default();
    world.avatars.insert("a0".into(), avatar("a0", "s1", "cafe"));
    for i in 1..=20u32 {
        let id = format!("peer{i}");
        world.avatars.insert(id.clone(), avatar(&id, "s1", "cafe"));
    }
    let snap = build_worker_snapshot(&world, "a0", "s1", vec![], None);
    assert!(snap.peers_in_room.len() <= PEER_CAP);
    assert_eq!(snap.peers_in_room.len(), PEER_CAP);
}

#[test]
fn worker_snapshot_caps_messages_at_20() {
    let world = WorldView::default();
    let messages: Vec<MessageStub> = (0..50)
        .map(|i| MessageStub {
            id: format!("m{i}"),
            author_id: "x".into(),
            body: "hi".into(),
            kind: "chat".into(),
            ts: i,
        })
        .collect();
    let snap = build_worker_snapshot(&world, "a1", "s1", messages, None);
    assert!(snap.recent_messages.len() <= MESSAGE_CAP);
    assert_eq!(snap.recent_messages.len(), MESSAGE_CAP);
    // Tail-truncation: should keep the LATEST messages.
    assert_eq!(snap.recent_messages.first().unwrap().ts, 30);
    assert_eq!(snap.recent_messages.last().unwrap().ts, 49);
}

#[test]
fn chunk_small_payload_fits_one_frame() {
    let small = "x".repeat(1024);
    let frames = chunk_snapshot(&small).unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].seq, 0);
    assert_eq!(frames[0].total, 1);
}

#[test]
fn chunk_large_payload_splits_correctly() {
    // Build a payload comfortably above CHUNK_THRESHOLD (256 KiB).
    let big = vec!["block".to_string(); 80_000]; // serialized JSON ~ 720 KiB
    let frames = chunk_snapshot(&big).unwrap();
    assert!(
        frames.len() > 1,
        "expected multiple chunks, got {}",
        frames.len()
    );
    let total = frames[0].total;
    assert_eq!(total as usize, frames.len());
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f.seq, i as u32);
        assert_eq!(f.total, total);
    }
}

#[test]
fn reassemble_round_trips() {
    let payload: Vec<String> = (0..10_000).map(|i| format!("item-{i}")).collect();
    let frames = chunk_snapshot(&payload).unwrap();
    let restored: Vec<String> = reassemble(&frames).unwrap();
    assert_eq!(restored, payload);
}

#[test]
fn reassemble_round_trips_unicode_at_chunk_boundaries() {
    // Ensure UTF-8-aware chunking handles multi-byte codepoints at boundaries.
    let payload: Vec<String> = (0..50_000).map(|i| format!("café-{i}-é-世界")).collect();
    let frames = chunk_snapshot(&payload).unwrap();
    let restored: Vec<String> = reassemble(&frames).unwrap();
    assert_eq!(restored, payload);
}

#[test]
fn reassemble_rejects_mismatched_frame_count() {
    let payload = vec!["a".to_string(); 100_000];
    let frames = chunk_snapshot(&payload).unwrap();
    if frames.len() > 1 {
        // Drop the last frame and try to reassemble.
        let truncated: Vec<ChunkFrame> = frames.iter().take(frames.len() - 1).cloned().collect();
        assert!(reassemble::<Vec<String>>(&truncated).is_err());
    }
}
