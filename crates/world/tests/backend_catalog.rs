#[tokio::test]
async fn probe_returns_three_entries() {
    let m = cliptown_world::backend_catalog::probe_all().await;
    assert_eq!(m.len(), 3, "expected 3 entries (claude_code, codex, opencode), got {}", m.len());
    assert!(m.contains_key("claude_code"));
    assert!(m.contains_key("codex"));
    assert!(m.contains_key("opencode"));
    // We don't assert availability — depends on host. But every entry should have last_checked_ts > 0.
    for (_, info) in &m {
        assert!(info.last_checked_ts > 0);
    }
}
