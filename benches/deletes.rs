//! `delete`, `delete_many` and `delete_range` against batch size and backend.
//!
//!   cargo bench --bench deletes
//!
//! Three axes:
//!
//! - **API**: a loop of `delete`, one `delete_many` over a key list, one
//!   `delete_range` over a span.
//! - **key distribution**: scattered, which is what evicting one generation
//!   from an interleaved table looks like, and contiguous. `delete_range`
//!   appears only in the contiguous sweep, because it cannot express a
//!   scattered set and reporting it there would mean quietly changing the
//!   workload to suit the API.
//! - **secondary index backend**: `worktables_index`, `arctic`, `congee`. A
//!   delete removes an entry from every index, so the backend is part of the
//!   cost and the three do not pay it equally.
//!
//! What to read. The batch arms carry a fixed setup cost (one lock acquisition
//! over the key set, one grace marker, one reclaim pass) and save a per-row
//! cost, so at a batch of one the loop should be competitive and the batch arms
//! should pull ahead as the batch grows. `delete_range` additionally replaces
//! `k` key lookups with one index walk, which is `O(log n)` per key against
//! `O(log n + k)` for the walk: that saving grows with the table, which is why
//! the table here is a quarter of a million rows rather than ten thousand.
//!
//! **The crossover is the result**, not any single ratio. An arm that loses
//! where it should lose is this bench working; an arm that wins everywhere is
//! worth distrusting before it is worth believing.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use wt_benchmarks::deletes::{self, BATCH};

// Every measured delete needs a table nobody has already deleted from, and
// building one is far more expensive than the delete. Criterion sizes its
// iteration count from `measurement_time` and reruns the setup closure for each
// one, so a three-second budget over a hundred-microsecond routine asks for
// thousands of table builds and the bench never finishes. It is not that the
// numbers would be wrong (setup is excluded from the timing) but that the wall
// clock is unbounded.
//
// So the iteration count is fixed here rather than inferred: `iter_custom`
// builds a table, times only the delete, and repeats a set number of times.
// Wall clock is then `REPS` builds per benchmark id and nothing else.
const SAMPLES: usize = 10;
// Proof of life: Criterion names each id on stderr as it starts. Run one axis at
// a time (`--bench 'deletes/wti/contiguous'`) rather than the whole grid, so a
// run is minutes and can be read as it goes.
const REPS: u64 = 4;
const MEASURE: Duration = Duration::from_secs(1);
const WARM_UP: Duration = Duration::from_millis(1);

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// One backend's two sweeps.
///
/// The tables returned by each arm are handed back to Criterion rather than
/// dropped inside the closure. Dropping a populated table frees every page,
/// which is far more work than the delete: a first draft timed it and reported
/// 268 us to delete a single row, which was table teardown.
macro_rules! sweeps {
    ($fn_name:ident, $module:ident, $label:literal) => {
        fn $fn_name(c: &mut Criterion) {
            use wt_benchmarks::deletes::$module;
            let rt = runtime();

            let mut group = c.benchmark_group(concat!("deletes/", $label, "/scattered"));
            group.sample_size(SAMPLES);
            group.sampling_mode(criterion::SamplingMode::Flat);
            group.measurement_time(MEASURE);
            group.warm_up_time(WARM_UP);
            for &count in &BATCH {
                group.throughput(Throughput::Elements(count));
                group.bench_with_input(
                    BenchmarkId::new("delete_loop", count),
                    &count,
                    |b, &count| {
                        let keys = deletes::scattered_keys(count);
                        b.iter_custom(|_| {
                            let mut total = Duration::ZERO;
                            for _ in 0..REPS {
                                let table = $module::populated();
                                let start = std::time::Instant::now();
                                rt.block_on($module::delete_loop(&table, &keys));
                                total += start.elapsed();
                                drop(table);
                            }
                            total / REPS as u32
                        });
                    },
                );
                group.bench_with_input(
                    BenchmarkId::new("delete_many", count),
                    &count,
                    |b, &count| {
                        let keys = deletes::scattered_keys(count);
                        b.iter_custom(|_| {
                            let mut total = Duration::ZERO;
                            for _ in 0..REPS {
                                let table = $module::populated();
                                let start = std::time::Instant::now();
                                $module::delete_many(&table, &keys);
                                total += start.elapsed();
                                drop(table);
                            }
                            total / REPS as u32
                        });
                    },
                );
            }
            group.finish();

            let mut group = c.benchmark_group(concat!("deletes/", $label, "/contiguous"));
            group.sample_size(SAMPLES);
            group.sampling_mode(criterion::SamplingMode::Flat);
            group.measurement_time(MEASURE);
            group.warm_up_time(WARM_UP);
            for &count in &BATCH {
                group.throughput(Throughput::Elements(count));
                group.bench_with_input(
                    BenchmarkId::new("delete_loop", count),
                    &count,
                    |b, &count| {
                        let keys = deletes::contiguous_keys(count);
                        b.iter_custom(|_| {
                            let mut total = Duration::ZERO;
                            for _ in 0..REPS {
                                let table = $module::populated();
                                let start = std::time::Instant::now();
                                rt.block_on($module::delete_loop(&table, &keys));
                                total += start.elapsed();
                                drop(table);
                            }
                            total / REPS as u32
                        });
                    },
                );
                group.bench_with_input(
                    BenchmarkId::new("delete_many", count),
                    &count,
                    |b, &count| {
                        let keys = deletes::contiguous_keys(count);
                        b.iter_custom(|_| {
                            let mut total = Duration::ZERO;
                            for _ in 0..REPS {
                                let table = $module::populated();
                                let start = std::time::Instant::now();
                                $module::delete_many(&table, &keys);
                                total += start.elapsed();
                                drop(table);
                            }
                            total / REPS as u32
                        });
                    },
                );
                group.bench_with_input(
                    BenchmarkId::new("delete_range", count),
                    &count,
                    |b, &count| {
                        b.iter_custom(|_| {
                            let mut total = Duration::ZERO;
                            for _ in 0..REPS {
                                let table = $module::populated();
                                let start = std::time::Instant::now();
                                $module::delete_range(&table, count);
                                total += start.elapsed();
                                drop(table);
                            }
                            total / REPS as u32
                        });
                    },
                );
            }
            group.finish();
        }
    };
}

sweeps!(wti, wti, "wti");
sweeps!(arctic, arctic, "arctic");
sweeps!(congee, congee, "congee");

criterion_group!(benches, wti, arctic, congee);
criterion_main!(benches);
