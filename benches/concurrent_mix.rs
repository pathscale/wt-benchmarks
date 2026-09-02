//! Mixed read/write across threads, per index backend.
//!
//!   cargo bench --bench concurrent_mix
//!   cargo bench --bench concurrent_mix -- 'concurrent/wti'   # one axis
//!
//! Three write ratios against three backends. A backend can win read-heavy and
//! lose write-heavy, so one blended workload would hide the only thing this is
//! for. Ratio 0 is pure readers plus the dedicated writer threads; the writer
//! threads always write.
//!
//! Read the arms against each other within a ratio. Absolute throughput on a
//! shared machine is not worth much, but the ordering between three backends
//! running the identical operation script survives load in a way the magnitudes
//! do not.
//!
//! Adapted from `WorkTablesIndex/benches/concurrent.rs`, which lived in a repo
//! nobody opens and compared the raw set against `scc` and `crossbeam`. That
//! answers a different question, and it could not cover arctic or congee, which
//! exist only as WorkTable index backends.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use wt_benchmarks::concurrent_mix::{self, OPS_PER_THREAD, READERS, WRITE_RATIOS, WRITERS};

// Bounded on purpose. Every sample rebuilds the table and spawns the full
// thread set, so the budget is the fixture size times the sample count and
// nothing else. The original ran 40 threads by 100,000 operations, which is
// four million operations per sample.
const SAMPLES: usize = 10;
const MEASURE: Duration = Duration::from_secs(2);
const WARM_UP: Duration = Duration::from_millis(200);

macro_rules! axis {
    ($fn_name:ident, $module:ident, $label:literal) => {
        fn $fn_name(c: &mut Criterion) {
            use wt_benchmarks::concurrent_mix::$module;

            let mut group = c.benchmark_group(concat!("concurrent/", $label));
            group.sample_size(SAMPLES);
            group.measurement_time(MEASURE);
            group.warm_up_time(WARM_UP);
            group.throughput(Throughput::Elements(
                OPS_PER_THREAD * (READERS + WRITERS) as u64,
            ));

            for &ratio in &WRITE_RATIOS {
                group.bench_with_input(
                    BenchmarkId::new("write_pct", ratio),
                    &ratio,
                    |b, &ratio| {
                        // Scripts are generated once and shared by every arm, so
                        // all three backends execute the identical sequence.
                        let scripts = concurrent_mix::scripts(ratio);
                        b.iter_batched(
                            $module::populated,
                            |table| {
                                let counts = $module::run(&table, &scripts);
                                // Returned so the table is dropped outside the
                                // timed region: freeing a populated table is
                                // more work than the workload.
                                (counts, table)
                            },
                            criterion::BatchSize::PerIteration,
                        );
                    },
                );
            }
            group.finish();
        }
    };
}

axis!(wti, wti, "wti");
axis!(arctic, arctic, "arctic");
axis!(congee, congee, "congee");

criterion_group!(benches, wti, arctic, congee);
criterion_main!(benches);
