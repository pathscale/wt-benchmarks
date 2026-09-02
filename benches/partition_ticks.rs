//! Routing to a partition and reading from it, under load and under churn.
//!
//! Criterion drives `iter_custom`, so the reported time is the window the
//! workload measured itself — after the barrier, excluding thread spawn — and
//! `Throughput::Elements(ticks)` turns it into ticks/sec.
//!
//! Two groups, answering two different questions.
//!
//! `throughput` sweeps the routing strategies against reader count. The thing
//! to read off it is not which strategy wins at one reader, which is a
//! microbenchmark question the repository already answers, but how each one
//! *scales*: `partition_arc` pays two atomic RMWs on a line every reader
//! shares, so on a hot symbol it should bend while the pinned forms stay flat.
//!
//! `batch` sweeps how many ticks a single pin covers. `pinned` amortises one
//! `SeqCst` fence across the batch, so the interesting number is where the
//! curve flattens: that is how long a batch has to be before pinning once is
//! worth the reclamation it delays.
//!
//! Both run with churn on by default, because a partition set that never
//! changes never exercises the grace period, and then every reclamation scheme
//! measures the same. `throughput/no_churn` is the control that shows what the
//! churn costs.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use wt_benchmarks::partition_ticks::{Routing, Shape, Spread, run};

/// Ticks each reader handles per repetition. Large enough that thread spawn
/// and the barrier are noise inside the measured window.
const TICKS_PER_READER: u64 = 200_000;

fn shape(routing: Routing, readers: usize, spread: Spread, churn: bool) -> Shape {
    Shape {
        readers,
        routing,
        spread,
        churn,
        ..Shape::default()
    }
}

fn throughput(c: &mut Criterion) {
    for (spread, churn, name) in [
        (Spread::Hot, true, "partition_ticks/hot_symbol"),
        (Spread::Spread, true, "partition_ticks/spread"),
        (Spread::Hot, false, "partition_ticks/hot_symbol_no_churn"),
    ] {
        let mut group = c.benchmark_group(name);
        group.measurement_time(Duration::from_secs(3));
        group.warm_up_time(Duration::from_secs(1));
        group.sample_size(10);

        for readers in [1usize, 2, 4, 8] {
            group.throughput(Throughput::Elements(TICKS_PER_READER * readers as u64));
            for &routing in Routing::available() {
                group.bench_with_input(
                    BenchmarkId::new(routing.label(), readers),
                    &readers,
                    |b, &readers| {
                        b.iter_custom(|iters| {
                            let mut total = Duration::ZERO;
                            for _ in 0..iters {
                                total +=
                                    run(shape(routing, readers, spread, churn), TICKS_PER_READER)
                                        .elapsed;
                            }
                            total
                        })
                    },
                );
            }
        }
        group.finish();
    }
}

/// How long a batch has to be before pinning once pays.
#[cfg(feature = "partition-pinned")]
fn batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("partition_ticks/batch");
    group.measurement_time(Duration::from_secs(3));
    group.warm_up_time(Duration::from_secs(1));
    group.sample_size(10);
    group.throughput(Throughput::Elements(TICKS_PER_READER * 4));

    for size in [1usize, 4, 16, 64, 256, 1024] {
        // A batch of one is `partition_ref` with extra steps, and is here as
        // the floor the curve starts from rather than as a serious option.
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut s = shape(Routing::Pinned, 4, Spread::Hot, true);
                    s.batch = size;
                    total += run(s, TICKS_PER_READER).elapsed;
                }
                total
            })
        });
    }
    group.finish();
}

#[cfg(not(feature = "partition-pinned"))]
fn batch(_: &mut Criterion) {}

criterion_group!(partition_tick_benchmarks, throughput, batch);
criterion_main!(partition_tick_benchmarks);
