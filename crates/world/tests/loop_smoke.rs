use cliptown_world::{loop_, state::WorldView};

#[tokio::test(start_paused = true)]
async fn tick_advances_seq() {
    let h = loop_::spawn(WorldView::default());
    let initial = h.view_rx.borrow().tick_seq;
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    assert!(h.view_rx.borrow().tick_seq > initial);
}
