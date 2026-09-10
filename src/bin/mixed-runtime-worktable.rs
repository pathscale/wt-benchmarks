//! **Selects on one runtime, upserts on another, against one table.**
//!
//! # The question
//!
//! YCSB says read-dominated tables want `spread` and write-heavy tables want
//! `locality`, and the gap is 1.5x to 2.2x. That is a property of the whole
//! table. The obvious next question is whether the split is really *per
//! operation class*: could one table put its reads on one pool and its writes
//! on another and beat either single choice?
//!
//! The DSL has syntax for it (`update runtime fast_local:`) that codegen does
//! not read, so nothing answers this today. This measures it before anybody
//! builds it.
//!
//! # The arms
//!
//! Every arm runs the same total work against the same table: `WT_READERS`
//! select tasks and `WT_WRITERS` upsert tasks, all live at once.
//!
//! | arm | reads on | writes on |
//! |---|---|---|
//! | uniform, per flavor | X | X |
//! | split | the read flavor | the write flavor |
//!
//! A split arm only means something against the **best** uniform arm, not
//! against the default: beating the default while losing to `spread` would
//! say routing is worse than picking one pool properly.
//!
//! # What it costs, which is the point
//!
//! A task handed to a pool other than the one the calling thread belongs to
//! takes the injector and a wake, measured at roughly 2,250 ns here. A select
//! that hits a hot page is tens of nanoseconds. So the hop has to be amortised
//! over a long run of same-class work or it cannot pay, and the arms below are
//! deliberately shaped that way: each task does `WT_OPS` operations of
//! one class, so the hop is paid once per task rather than once per operation.
//!
//! **That is the most favourable shape for the idea.** If it does not win
//! here, per-operation routing certainly does not.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use worktable::prelude::*;
use worktable::worktable;

worktable!(
    name: Mixed,
    persist: false,
    columns: {
        id: u64 primary_key,
        value: u64,
        payload: String,
    },
);

const ROWS: u64 = 100_000;

/// Concurrency, from the environment so a sweep needs no rebuild.
///
/// The read/write ratio is the variable that most plausibly decides whether
/// splitting pays: with one writer there is little for reads to queue behind,
/// and with many the write side may simply saturate.
fn readers() -> usize {
    env_count("WT_READERS", 6)
}

fn writers() -> usize {
    env_count("WT_WRITERS", 2)
}

/// Operations per task, scaled down as concurrency rises so that a sweep
/// keeps roughly constant total work and constant wall time per arm.
fn ops_per_task() -> u64 {
    std::env::var("WT_OPS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(320_000 / (readers() + writers()) as u64)
}

fn env_count(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|count| *count > 0)
        .unwrap_or(fallback)
}

/// A cheap deterministic index, so the arms touch the same keys in the same
/// order and no arm gets a friendlier access pattern than another.
fn key(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state % ROWS
}

async fn read_task(table: Arc<MixedWorkTable>, seed: u64, hits: Arc<AtomicU64>) {
    let mut state = seed | 1;
    let mut found = 0u64;
    for _ in 0..ops_per_task() {
        if table.select(key(&mut state)).is_some() {
            found += 1;
        }
    }
    hits.fetch_add(found, Ordering::Relaxed);
}

async fn write_task(table: Arc<MixedWorkTable>, seed: u64, done: Arc<AtomicU64>) {
    let mut state = seed | 1;
    let mut applied = 0u64;
    for _ in 0..ops_per_task() {
        let id = key(&mut state);
        if table
            .upsert(MixedRow {
                id,
                value: state,
                payload: String::new(),
            })
            .await
            .is_ok()
        {
            applied += 1;
        }
    }
    done.fetch_add(applied, Ordering::Relaxed);
}

fn loaded() -> Arc<MixedWorkTable> {
    let table = Arc::new(MixedWorkTable::default());
    for id in 0..ROWS {
        futures::executor::block_on(table.insert(MixedRow {
            id,
            value: id,
            payload: String::new(),
        }))
        .expect("fresh key");
    }
    table
}

/// One arm. Reads go to `read_on`, writes to `write_on`, everything overlaps.
fn run(read_on: Flavor, write_on: Flavor) -> (f64, f64, f64) {
    let table = loaded();
    let hits = Arc::new(AtomicU64::new(0));
    let writes = Arc::new(AtomicU64::new(0));

    let read_pool = executor_for_flavor(read_on);
    let write_pool = executor_for_flavor(write_on);

    let cpu_before = wt_benchmarks::cpu::cpu_seconds();
    let started = Instant::now();
    let mut handles = Vec::with_capacity(readers() + writers());
    for n in 0..readers() {
        handles.push(read_pool.spawn(read_task(
            Arc::clone(&table),
            0x9E37_79B9_7F4A_7C15 ^ n as u64,
            Arc::clone(&hits),
        )));
    }
    for n in 0..writers() {
        handles.push(write_pool.spawn(write_task(
            Arc::clone(&table),
            0xD1B5_4A32_D192_ED03 ^ n as u64,
            Arc::clone(&writes),
        )));
    }
    for handle in handles {
        futures::executor::block_on(handle);
    }
    let elapsed = started.elapsed().as_secs_f64();
    let cpu = wt_benchmarks::cpu::cpu_seconds() - cpu_before;

    let reads = readers() as f64 * ops_per_task() as f64;
    let written = writers() as f64 * ops_per_task() as f64;
    assert!(hits.load(Ordering::Relaxed) > 0, "the read arm found nothing");
    assert!(writes.load(Ordering::Relaxed) > 0, "the write arm applied nothing");
    (reads / elapsed, written / elapsed, cpu / elapsed)
}

/// The tokio control: the same tasks, on a tokio runtime instead of nagoya
/// pools.
///
/// **What this is and is not.** The table's own internals are still
/// `NagoyaRt`, so this is a *tokio-driven client* arm, not a full `TokioRt`
/// build. It answers "would my application be faster driving WorkTable from
/// tokio", which is the question an existing tokio service actually has. The
/// full backend swap needs `runtime: tokio` on the declaration and lives in
/// the sibling benchmark tree.
///
/// One runtime, not two, because tokio has no equivalent of the split being
/// tested: a second `Runtime` would be a second thread pool and the comparison
/// would stop being about scheduling.
fn tokio_arm(worker_threads: usize) -> (f64, f64, f64) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .build()
        .expect("a tokio runtime");
    let table = loaded();
    let hits = Arc::new(AtomicU64::new(0));
    let writes = Arc::new(AtomicU64::new(0));

    let cpu_before = wt_benchmarks::cpu::cpu_seconds();
    let started = Instant::now();
    runtime.block_on(async {
        let mut handles = Vec::with_capacity(readers() + writers());
        for n in 0..readers() {
            handles.push(tokio::spawn(read_task(
                Arc::clone(&table),
                0x9E37_79B9_7F4A_7C15 ^ n as u64,
                Arc::clone(&hits),
            )));
        }
        for n in 0..writers() {
            handles.push(tokio::spawn(write_task(
                Arc::clone(&table),
                0xD1B5_4A32_D192_ED03 ^ n as u64,
                Arc::clone(&writes),
            )));
        }
        for handle in handles {
            handle.await.expect("a task");
        }
    });
    let elapsed = started.elapsed().as_secs_f64();
    let cpu = wt_benchmarks::cpu::cpu_seconds() - cpu_before;
    let reads = readers() as f64 * ops_per_task() as f64;
    let written = writers() as f64 * ops_per_task() as f64;
    assert!(hits.load(Ordering::Relaxed) > 0, "the tokio read arm found nothing");
    (reads / elapsed, written / elapsed, cpu / elapsed)
}

fn main() {
    println!(
        "{ROWS} rows, {} select tasks and {} upsert tasks live at once,\n\
         {} operations per task, median of 3, arms interleaved\n",
        readers(),
        writers(),
        ops_per_task()
    );

    let uniform = [
        Flavor::SharedSlot,
        Flavor::Locality,
        Flavor::Spread,
        Flavor::Throughput,
        Flavor::LowLatency,
    ];
    // The split arms worth trying: YCSB says reads want spread and writes want
    // locality, so that pairing is the hypothesis. Its mirror is included
    // because a hypothesis that only looks right in one direction is not
    // being tested.
    let split = [
        (Flavor::Spread, Flavor::Locality),
        (Flavor::Locality, Flavor::Spread),
        (Flavor::Spread, Flavor::SharedSlot),
        (Flavor::SharedSlot, Flavor::Locality),
    ];

    let mut rows: Vec<(String, f64, f64, f64)> = Vec::new();
    // Interleaved: one repetition of every arm, then the next, so that a
    // machine warming up over the run cannot be charged to whichever arm ran
    // last.
    let workers = std::env::var("WT_RUNTIME_WORKERS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, std::num::NonZeroUsize::get));
    let mut tokio_samples: Vec<(f64, f64, f64)> = Vec::new();
    let mut samples: Vec<Vec<(f64, f64, f64)>> = vec![Vec::new(); uniform.len() + split.len()];
    for _ in 0..3 {
        tokio_samples.push(tokio_arm(workers));
        for (slot, flavor) in uniform.iter().enumerate() {
            samples[slot].push(run(*flavor, *flavor));
        }
        for (offset, (read_on, write_on)) in split.iter().enumerate() {
            samples[uniform.len() + offset].push(run(*read_on, *write_on));
        }
    }

    let median = |mut values: Vec<(f64, f64, f64)>| {
        values.sort_by(|a, b| (a.0 + a.1).partial_cmp(&(b.0 + b.1)).expect("no NaN"));
        values[values.len() / 2]
    };
    {
        let (r, w, c) = median(tokio_samples.clone());
        rows.push(("tokio (control)".to_owned(), r, w, c));
    }
    for (slot, flavor) in uniform.iter().enumerate() {
        let (r, w, c) = median(samples[slot].clone());
        rows.push((format!("uniform {}", flavor.name()), r, w, c));
    }
    for (offset, (read_on, write_on)) in split.iter().enumerate() {
        let (r, w, c) = median(samples[uniform.len() + offset].clone());
        rows.push((
            format!("split r={} w={}", read_on.name(), write_on.name()),
            r,
            w,
            c,
        ));
    }

    println!(
        "  {:<34} {:>14} {:>14} {:>14} {:>8}",
        "arm", "selects/s", "upserts/s", "total ops/s", "cores"
    );
    let tokio_total = rows[0].1 + rows[0].2;
    let best_uniform = rows[1..=uniform.len()]
        .iter()
        .map(|row| row.1 + row.2)
        .fold(0.0f64, f64::max);
    for (label, reads, writes, cores) in &rows {
        let total = reads + writes;
        println!(
            "  {label:<34} {reads:>14.0} {writes:>14.0} {total:>14.0} {cores:>7.1}x{}",
            if label.starts_with("tokio") {
                String::new()
            } else {
                format!(
                    "   {:+.1}% vs tokio,{:+.1}% vs best uniform",
                    (total / tokio_total - 1.0) * 100.0,
                    (total / best_uniform - 1.0) * 100.0
                )
            }
        );
    }
}
