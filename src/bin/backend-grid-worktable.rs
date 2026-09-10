//! **Index backend x concurrency**, which is the axis every other benchmark
//! here leaves out.
//!
//! # Why this exists
//!
//! Every table picks one of three secondary index backends, and the rest of
//! this suite measures whichever one happens to be the default. A comparison
//! that leaves the backend out is really a comparison of that default, and it
//! attributes to `worktable` whatever belongs to `WorkTablesIndex`, `arctic`
//! or `congee`.
//!
//! That matters concretely: a beta.19 tree resolving `WorkTablesIndex` from the
//! registry and a 1.9 tree resolving it from a local path are not running the
//! same index, so a difference between them is not necessarily a difference in
//! the engine.
//!
//! # Why concurrency is the other axis
//!
//! The backends differ most in how they behave under contention rather than in
//! raw single-threaded cost, so a fixed thread count answers half the question.
//! The sweep is over readers and writers together, holding total work constant.
//!
//! **In memory only, so far.** Arctic and congee persist through native
//! checkpoint and WAL adapters rather than the shared page format, so a
//! persisted comparison is a different write path and these numbers say nothing
//! about it. `persisted-grid` varies page size but declares no secondary index,
//! so the two axes have never crossed.
//!
//! The secondary index is **unique**, because `congee` supports only unique
//! indexes and an arm that used a different index shape would not be comparable.
//! Writes take their indexed value from a process-wide counter, so every upsert
//! is a real index mutation and none is rejected as a duplicate: writing a value
//! already stored lets a unique index short circuit, and the workload then
//! measures a rejected write.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use wt_benchmarks::grid::GridRow;
use wt_benchmarks::result::LatencySummary;
use worktable::prelude::*;
use worktable::worktable;

const ROWS: u64 = 100_000;
const SAMPLE_EVERY: u64 = 512;

/// Supplies a distinct indexed value per write, so no upsert is rejected as a
/// duplicate on a unique index.
static NEXT_TAG: AtomicU64 = AtomicU64::new(ROWS);

fn ops() -> u64 {
    std::env::var("WT_OPS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(20_000)
}

fn rounds() -> u32 {
    std::env::var("WT_ROUNDS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(9)
}

fn workers() -> usize {
    std::env::var("WT_RUNTIME_WORKERS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, std::num::NonZeroUsize::get))
}

/// A cheap deterministic index, so every backend touches the same keys in the
/// same order and none gets a friendlier access pattern.
fn key(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state % ROWS
}

macro_rules! backend_arm {
    ($module:ident, $backend:ident, $label:literal) => {
        mod $module {
            use super::*;

            worktable!(
                name: Bench,
                persist: false,
                columns: {
                    id: u64 primary_key,
                    value: u64,
                    tag: u64,
                },
                indexes: {
                    tag_idx: tag unique using $backend,
                },
            );

            fn loaded() -> Arc<BenchWorkTable> {
                let table = Arc::new(BenchWorkTable::default());
                for id in 0..ROWS {
                    futures::executor::block_on(table.insert(BenchRow {
                        id,
                        value: id,
                        tag: id,
                    }))
                    .expect("fresh key");
                }
                table
            }

            async fn read_task(
                table: Arc<BenchWorkTable>,
                seed: u64,
                hits: Arc<AtomicU64>,
                latency: Arc<std::sync::Mutex<Vec<u64>>>,
            ) {
                let mut state = seed | 1;
                let mut found = 0u64;
                let mut mine = Vec::with_capacity((ops() / SAMPLE_EVERY) as usize + 1);
                for index in 0..ops() {
                    let at = index.is_multiple_of(SAMPLE_EVERY).then(Instant::now);
                    if table.select(key(&mut state)).is_some() {
                        found += 1;
                    }
                    if let Some(at) = at {
                        mine.push(u64::try_from(at.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    }
                }
                hits.fetch_add(found, Ordering::Relaxed);
                latency.lock().expect("the samples").extend(mine);
            }

            async fn write_task(
                table: Arc<BenchWorkTable>,
                seed: u64,
                done: Arc<AtomicU64>,
                latency: Arc<std::sync::Mutex<Vec<u64>>>,
            ) {
                let mut state = seed | 1;
                let mut applied = 0u64;
                let mut mine = Vec::with_capacity((ops() / SAMPLE_EVERY) as usize + 1);
                for index in 0..ops() {
                    let id = key(&mut state);
                    let at = index.is_multiple_of(SAMPLE_EVERY).then(Instant::now);
                    if table
                        .upsert(BenchRow {
                            id,
                            value: state,
                            tag: NEXT_TAG.fetch_add(1, Ordering::Relaxed),
                        })
                        .await
                        .is_ok()
                    {
                        applied += 1;
                    }
                    if let Some(at) = at {
                        mine.push(u64::try_from(at.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    }
                }
                done.fetch_add(applied, Ordering::Relaxed);
                latency.lock().expect("the samples").extend(mine);
            }

            pub fn run(readers: usize, writers: usize, runtime: &str) {
                let table = loaded();
                let hits = Arc::new(AtomicU64::new(0));
                let writes = Arc::new(AtomicU64::new(0));
                let read_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
                let write_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));

                let cpu_before = wt_benchmarks::cpu::cpu_seconds();
                let started = Instant::now();
                if runtime == "tokio" {
                    let rt = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(workers())
                        .build()
                        .expect("a tokio runtime");
                    rt.block_on(async {
                        let mut handles = Vec::with_capacity(readers + writers);
                        for n in 0..readers {
                            handles.push(tokio::spawn(read_task(
                                Arc::clone(&table),
                                0x9E37_79B9_7F4A_7C15 ^ n as u64,
                                Arc::clone(&hits),
                                Arc::clone(&read_latency),
                            )));
                        }
                        for n in 0..writers {
                            handles.push(tokio::spawn(write_task(
                                Arc::clone(&table),
                                0xD1B5_4A32_D192_ED03 ^ n as u64,
                                Arc::clone(&writes),
                                Arc::clone(&write_latency),
                            )));
                        }
                        for handle in handles {
                            handle.await.expect("a task");
                        }
                    });
                } else {
                    let pool = engine_executor();
                    let mut handles = Vec::with_capacity(readers + writers);
                    for n in 0..readers {
                        handles.push(pool.spawn(read_task(
                            Arc::clone(&table),
                            0x9E37_79B9_7F4A_7C15 ^ n as u64,
                            Arc::clone(&hits),
                            Arc::clone(&read_latency),
                        )));
                    }
                    for n in 0..writers {
                        handles.push(pool.spawn(write_task(
                            Arc::clone(&table),
                            0xD1B5_4A32_D192_ED03 ^ n as u64,
                            Arc::clone(&writes),
                            Arc::clone(&write_latency),
                        )));
                    }
                    for handle in handles {
                        futures::executor::block_on(handle);
                    }
                }
                let elapsed = started.elapsed().as_secs_f64();
                let cpu = wt_benchmarks::cpu::cpu_seconds() - cpu_before;
                assert!(hits.load(Ordering::Relaxed) > 0, "the read arm found nothing");
                assert!(writes.load(Ordering::Relaxed) > 0, "the write arm applied nothing");

                let reads = readers as f64 * ops() as f64;
                let written = writers as f64 * ops() as f64;
                GridRow {
                    schema_version: 1,
                    suite: "backend-grid",
                    runtime: runtime.to_owned(),
                    tuning: if runtime == "tokio" {
                        "tokio multi_thread".to_owned()
                    } else {
                        describe_tuning(engine_flavor())
                    },
                    dispatch: "pool",
                    index_backend: $label,
                    repetition: wt_benchmarks::grid::repetition(),
                    page_size: None,
                    worker_threads: workers(),
                    readers,
                    writers,
                    ops_per_task: ops(),
                    elapsed_ns: started.elapsed().as_nanos(),
                    ops_per_second: (reads + written) / elapsed,
                    read_ops_per_second: reads / elapsed,
                    write_ops_per_second: written / elapsed,
                    cpu_x: cpu / elapsed,
                    read_latency: LatencySummary::from_samples(std::mem::take(
                        &mut *read_latency.lock().expect("the samples"),
                    )),
                    write_latency: LatencySummary::from_samples(std::mem::take(
                        &mut *write_latency.lock().expect("the samples"),
                    )),
                    target_arch: std::env::consts::ARCH,
                    target_os: std::env::consts::OS,
                }
                .emit();
                println!(
                    "  {:<10} {:<7} r={:<3} w={:<3} {:>12.0} ops/s {:>6.1} cores",
                    $label,
                    runtime,
                    readers,
                    writers,
                    (reads + written) / elapsed,
                    cpu / elapsed
                );
            }
        }
    };
}

backend_arm!(wti, worktables_index, "worktables_index");
backend_arm!(arctic_backend, arctic, "arctic");
backend_arm!(congee_backend, congee, "congee");

fn main() {
    let levels: [(usize, usize); 4] = [(6, 2), (12, 4), (24, 8), (48, 16)];
    println!(
        "{ROWS} rows, {} operations per task, {} repetitions, arms interleaved\n",
        ops(),
        rounds()
    );
    // Interleaved: one repetition of every arm before the next, so a machine
    // warming up over the run cannot be charged to whichever arm ran last.
    for round in 0..rounds() {
        wt_benchmarks::grid::set_repetition(round + 1);
        for (readers, writers) in levels {
            for runtime in ["nagoya", "tokio"] {
                wti::run(readers, writers, runtime);
                arctic_backend::run(readers, writers, runtime);
                congee_backend::run(readers, writers, runtime);
            }
        }
    }
}
