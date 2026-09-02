//! The agentcode storage profile.
//!
//!   cargo bench --bench codegraph
//!
//! Four groups, each one an operation agentcode's latency actually depends on.
//! Read `src/codegraph.rs` for why this shape and not another.
//!
//! The number to look at first is `publish/persisted` against `publish/memory`
//! at the same size. agentcode measured that ratio at 22x, 10.24 us per
//! persisted row against 0.46 us into a memory table, and it is the dominant
//! term in a real incremental update. No index choice moves it. If a WorkTable
//! change is going to make or break this consumer, it shows up there.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use wt_benchmarks::codegraph::{CHANGED_FILES, FILES, Generation, driver, rows_per_generation};

// Publishing builds a store from empty every iteration, so samples are
// expensive and few. Fixed rather than left to Criterion: the comparisons here
// are between arms at the same size, which does not need tight intervals, and a
// larger budget would make a persisted regression take minutes to report.
const SAMPLES: usize = 10;
const MEASURE: Duration = Duration::from_secs(12);
const WARM_UP: Duration = Duration::from_secs(1);

fn runtime() -> Runtime {
    Runtime::new().expect("tokio runtime")
}

/// The ratio that decides whether WorkTable carries this workload.
fn publish(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("codegraph/publish");
    group.sample_size(SAMPLES);
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for files in FILES {
        group.throughput(Throughput::Elements(rows_per_generation(files)));

        // Durable. The flush is inside the timed region on purpose: a publish
        // that has not reached disk has not happened, and agentcode waits for
        // the drain before it calls a generation published.
        group.bench_with_input(BenchmarkId::new("persisted", files), &files, |b, &files| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for i in 0..iters {
                    let dir = tempfile::tempdir().expect("state dir");
                    let store = rt.block_on(driver::persisted::Store::open(dir.path()));
                    let generation = Generation::new(i, files);
                    let start = std::time::Instant::now();
                    store.publish(&generation);
                    rt.block_on(store.flush());
                    total += start.elapsed();
                }
                total
            })
        });

        // The same rows, same indexes, no durability. The gap between this and
        // the arm above is the whole ask.
        group.bench_with_input(BenchmarkId::new("memory", files), &files, |b, &files| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for i in 0..iters {
                    let store = driver::memory::Store::default();
                    let generation = Generation::new(i, files);
                    let start = std::time::Instant::now();
                    store.publish(&generation);
                    total += start.elapsed();
                }
                total
            })
        });
    }
    group.finish();
}

/// One file changes. agentcode's measured whole-update cost is about 790 ms
/// against a 58 ms floor of delta writes alone, so what this isolates is the
/// marginal write, not the walk that decides what changed.
fn incremental(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("codegraph/incremental");
    group.throughput(Throughput::Elements(rows_per_generation(CHANGED_FILES)));
    group.sample_size(SAMPLES);
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for files in FILES {
        group.bench_with_input(BenchmarkId::new("persisted", files), &files, |b, &files| {
            b.iter_custom(|iters| {
                let dir = tempfile::tempdir().expect("state dir");
                let store = rt.block_on(driver::persisted::Store::open(dir.path()));
                // A populated store, because the cost of adding a row to an
                // empty index is not the cost of adding one to a full index.
                store.publish(&Generation::new(0, files));
                rt.block_on(store.flush());

                let mut total = Duration::ZERO;
                for i in 0..iters {
                    let delta = Generation::new(i + 1, CHANGED_FILES);
                    let start = std::time::Instant::now();
                    store.publish(&delta);
                    rt.block_on(store.flush());
                    total += start.elapsed();
                }
                total
            })
        });
    }
    group.finish();
}

/// Enumerating a generation through the hot non-unique index. Fan-out is the
/// whole generation, which is the distribution that made `WorkTablesIndex`
/// 0.0.8 scan on insert.
fn generation_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("codegraph/generation_scan");
    group.sample_size(SAMPLES);
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for files in FILES {
        group.throughput(Throughput::Elements(files * 18));
        group.bench_with_input(BenchmarkId::new("memory", files), &files, |b, &files| {
            let store = driver::memory::Store::default();
            let generation = Generation::new(0, files);
            store.publish(&generation);
            b.iter(|| store.generation_scan(&generation))
        });
    }
    group.finish();
}

/// The adjacency walk `dependencies.query` performs on every call: incoming and
/// outgoing edges for one node, both through `u128` indexes on Arctic.
fn dependency_walk(c: &mut Criterion) {
    let mut group = c.benchmark_group("codegraph/dependency_walk");
    group.throughput(Throughput::Elements(1));
    group.sample_size(SAMPLES);
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for files in FILES {
        group.bench_with_input(BenchmarkId::new("memory", files), &files, |b, &files| {
            let store = driver::memory::Store::default();
            let generation = Generation::new(0, files);
            store.publish(&generation);
            let node = generation.probe_node();
            b.iter(|| store.dependency_walk(node))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    publish,
    incremental,
    generation_scan,
    dependency_walk
);
criterion_main!(benches);
