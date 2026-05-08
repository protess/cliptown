use cliptown_world::{loop_, state::WorldView, storage};

#[tokio::test]
async fn tick_advances_seq() {
    // Real-time: the loop emits a Tick every second. Wait > 1s and confirm the
    // sequence advanced. Using paused time would require setting up the pool
    // before pausing; this is simpler and still fast in CI (~1.2s).
    let pool = storage::open(":memory:").await.unwrap();
    let h = loop_::spawn(WorldView::default(), pool);
    let initial = h.view_rx.borrow().tick_seq;
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(h.view_rx.borrow().tick_seq > initial);
}
