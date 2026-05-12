//! M11 real bench harness. Both benches drive a real `loop_::spawn` world
//! handle. `tick_latency_real_loop` measures one Cmd::Tick round-trip
//! (move_sys::step_all + scheduler::tick + proximity::compute_and_emit +
//! view_tx.send). `console_dispatch_throughput_100_msgs` fires 100
//! Cmd::HandleConsoleMsg with oneshot replies — the same dispatcher lane
//! frontend / operator commands travel.

use cliptown_world::{loop_, protocol::ConsoleOutbound, seed, state::WorldView, storage};
use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;
use tempfile::TempDir;
use tokio::runtime::Runtime;
use tokio::sync::{broadcast, oneshot};

/// Build a real cliptown world: tmpdir SQLite, seeded schema, broadcast
/// channel, and a spawned `loop_::spawn` handle. Returns the handle plus
/// the tempdir guard (must be held alive for the lifetime of the bench).
async fn make_world() -> (loop_::Handle, TempDir, broadcast::Receiver<ConsoleOutbound>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("bench.db");
    let pool = storage::open(db_path.to_str().unwrap()).await.unwrap();
    seed::seed_if_empty(&pool).await.unwrap();
    let (event_tx, event_rx) = broadcast::channel(64);
    let handle = loop_::spawn(WorldView::default(), pool, event_tx);
    (handle, dir, event_rx)
}

fn bench_tick_latency(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    // Setup once per bench function; held alive for all iters.
    let (handle, _dir, _event_rx) = rt.block_on(make_world());

    c.bench_function("tick_latency_real_loop", |b| {
        b.iter(|| {
            rt.block_on(async {
                // Cloning the watch::Receiver per-iter resets its "has-changed"
                // flag so the first .changed() call after sending Cmd::Tick
                // actually waits for the next view broadcast (the watch channel
                // pre-seeds with the initial view).
                let mut view_rx = handle.view_rx.clone();
                handle.tx.send(loop_::Cmd::Tick).await.unwrap();
                view_rx.changed().await.unwrap();
            });
        });
    });
}

fn bench_console_dispatch_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (handle, _dir, _event_rx) = rt.block_on(make_world());

    c.bench_function("console_dispatch_throughput_100_msgs", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut replies = Vec::with_capacity(100);
                for _ in 0..100 {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    // Unknown message shape: cmd_console::dispatch returns
                    // {"type":"error","reason":"parse",...} via its initial
                    // serde_json::from_value guard — fast path, no DB write,
                    // no broadcast. We're measuring the mpsc → parse → oneshot
                    // round-trip.
                    let msg = serde_json::json!({"type":"bench_noop"});
                    handle
                        .tx
                        .send(loop_::Cmd::HandleConsoleMsg { msg, reply: reply_tx })
                        .await
                        .unwrap();
                    replies.push(reply_rx);
                }
                for rx in replies {
                    rx.await.unwrap();
                }
            });
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(3)).sample_size(10);
    targets = bench_tick_latency, bench_console_dispatch_throughput
}
criterion_main!(benches);
