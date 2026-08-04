//! YCSB on Criterion, two ports (run twice, both data types):
//!
//!   1. THROUGHPUT port — Criterion `iter_custom` times the concurrent YCSB run
//!      and reports its own `elapsed_ns`; with `Throughput::Elements(ops)`
//!      Criterion prints operations/sec with confidence intervals. `iter_custom`
//!      (not `iter`) so the harness's measured window — not Criterion's — defines
//!      the timing, and the barrier/thread-spawn overhead is excluded.
//!
//!   2. LATENCY port — runs the same workload and reports the p50/p99 the YCSB
//!      harness already records per operation. Criterion drives the sampling; the
//!      value reported is the tail latency, so throughput sampling never hides it.
//!
//! Both ports call the identical `run_repetition`, so the two runs measure the
//! same workload from two angles. Feature-gated on `worktable-adapter`.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use wt_benchmarks::config::Config;
use wt_benchmarks::ycsb::{run_repetition, Workload};

const RECORDS: u64 = 50_000;
const OPERATIONS: u64 = 200_000;
const THREADS: usize = 4;

fn cfg(workload: Workload) -> Config {
    Config {
        workload,
        records: RECORDS,
        operations: OPERATIONS,
        threads: THREADS,
        ..Config::default()
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(THREADS + 1)
        .enable_all()
        .build()
        .unwrap()
}

/// The workloads to sweep. A/B/C/F are point-op mixes; D is read-latest; E is
/// short scans.
const WORKLOADS: &[(&str, Workload)] = &[
    ("A", Workload::A),
    ("B", Workload::B),
    ("C", Workload::C),
    ("F", Workload::F),
];

fn throughput(c: &mut Criterion) {
    let runtime = rt();
    let mut group = c.benchmark_group("ycsb/throughput");
    group.throughput(Throughput::Elements(OPERATIONS));
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);

    for (name, workload) in WORKLOADS {
        group.bench_function(*name, |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let result = runtime.block_on(run_repetition(&cfg(*workload), 1));
                    // Use the harness's own measured window, not wall time around
                    // thread spawn/join.
                    total += Duration::from_nanos(result.elapsed_ns as u64);
                }
                total
            })
        });
    }
    group.finish();
}

fn latency(c: &mut Criterion) {
    let runtime = rt();
    let mut group = c.benchmark_group("ycsb/latency_p99");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);

    for (name, workload) in WORKLOADS {
        // Report per-op p99 (read op is present in every workload). iter_custom
        // returns the p99 as the "duration" so Criterion's estimate IS the p99.
        group.bench_function(*name, |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let result = runtime.block_on(run_repetition(&cfg(*workload), 1));
                    let p99 = result
                        .latency
                        .get("read")
                        .and_then(|s| s.p99_ns)
                        .unwrap_or(0);
                    total += Duration::from_nanos(p99);
                }
                total
            })
        });
    }
    group.finish();
}

criterion_group!(benches, throughput, latency);
criterion_main!(benches);
