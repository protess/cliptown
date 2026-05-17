//! P5 Theme A: operator presence registry unit tests.

use cliptown_world::presence::{drop_entry, gc, new_registry, snapshot, upsert, PRESENCE_TTL_SECS};

#[tokio::test]
async fn upsert_inserts_new_entry_and_returns_true() {
    let reg = new_registry();
    let changed = upsert(&reg, "op_a", "Alice", "admin", Some("s1".into()), 1000).await;
    assert!(changed, "first upsert is a presence change");
    let snap = snapshot(&reg).await;
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].operator_id, "op_a");
    assert_eq!(snap[0].operator_name, "Alice");
    assert_eq!(snap[0].focused_startup_id.as_deref(), Some("s1"));
    assert_eq!(snap[0].last_seen_at, 1000);
}

#[tokio::test]
async fn upsert_same_focus_returns_false() {
    let reg = new_registry();
    let _ = upsert(&reg, "op_a", "Alice", "admin", Some("s1".into()), 1000).await;
    let changed = upsert(&reg, "op_a", "Alice", "admin", Some("s1".into()), 1030).await;
    assert!(!changed, "same focus heartbeat must not flag a presence change");
    let snap = snapshot(&reg).await;
    assert_eq!(snap[0].last_seen_at, 1030, "but last_seen_at must refresh");
}

#[tokio::test]
async fn upsert_focus_change_returns_true() {
    let reg = new_registry();
    let _ = upsert(&reg, "op_a", "Alice", "admin", Some("s1".into()), 1000).await;
    let changed = upsert(&reg, "op_a", "Alice", "admin", Some("s2".into()), 1030).await;
    assert!(changed, "focus change must flag a presence change");
    let snap = snapshot(&reg).await;
    assert_eq!(snap[0].focused_startup_id.as_deref(), Some("s2"));
}

#[tokio::test]
async fn drop_entry_removes_and_returns_true_once() {
    let reg = new_registry();
    let _ = upsert(&reg, "op_a", "Alice", "admin", None, 1000).await;
    assert!(drop_entry(&reg, "op_a").await);
    assert!(!drop_entry(&reg, "op_a").await, "second drop is a no-op");
    assert!(snapshot(&reg).await.is_empty());
}

#[tokio::test]
async fn gc_drops_only_stale_entries() {
    let reg = new_registry();
    // Two entries: one fresh (now), one stale (now - 2*TTL).
    let now = 10_000_i64;
    let _ = upsert(&reg, "op_fresh", "F", "admin", None, now).await;
    let _ = upsert(&reg, "op_stale", "S", "viewer", None, now - PRESENCE_TTL_SECS * 2).await;
    let dropped = gc(&reg, now).await;
    assert_eq!(dropped, 1);
    let snap = snapshot(&reg).await;
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].operator_id, "op_fresh");
}

#[tokio::test]
async fn gc_keeps_entries_at_exact_ttl_boundary() {
    let reg = new_registry();
    let now = 10_000_i64;
    // Entry at exactly the TTL boundary — `last_seen_at = now - TTL` should
    // be kept (the gc cutoff is `>= now - TTL`, not `>`).
    let _ = upsert(&reg, "op_a", "A", "admin", None, now - PRESENCE_TTL_SECS).await;
    let dropped = gc(&reg, now).await;
    assert_eq!(dropped, 0, "entry at exact TTL boundary must survive");
    assert_eq!(snapshot(&reg).await.len(), 1);
}
