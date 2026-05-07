use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::time::Duration;
use tokio::runtime::Runtime;

fn bench_tick_latency(c: &mut Criterion) {
    c.bench_function("tick_latency_per_loop_iter", |b| {
        let rt = Runtime::new().unwrap();
        b.iter(|| {
            rt.block_on(async {
                // Phase 0 placeholder: a tick is a loop iteration over an in-memory state.
                // Full bench wires WorldView + scheduler + proximity. M10 ships the harness.
                let mut sum = 0u64;
                for i in 0u64..1000 {
                    sum = black_box(sum.wrapping_add(i));
                }
                sum
            });
        });
    });
}

fn bench_mpsc_throughput(c: &mut Criterion) {
    c.bench_function("mpsc_throughput_1k_msgs", |b| {
        let rt = Runtime::new().unwrap();
        b.iter(|| {
            rt.block_on(async {
                let (tx, mut rx) = tokio::sync::mpsc::channel::<u64>(1024);
                let prod = tokio::spawn(async move {
                    for i in 0u64..1000 {
                        let _ = tx.send(i).await;
                    }
                });
                let mut sum = 0u64;
                while let Some(v) = rx.recv().await {
                    sum = sum.wrapping_add(v);
                }
                let _ = prod.await;
                sum
            });
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(3)).sample_size(10);
    targets = bench_tick_latency, bench_mpsc_throughput
}
criterion_main!(benches);
