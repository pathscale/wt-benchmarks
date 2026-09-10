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

/// Repetitions of every arm.
///
/// Three was the old hardcoded value and it is not enough. A page-size effect
/// read at +31% from three rounds was between -4.3% and +8.6% at eleven, and a
/// flavor read as 85% better on seven repetitions was 0.2% better on sixteen.
/// The default is raised and the knob exists so a campaign can raise it again.
fn rounds() -> u32 {
    std::env::var("WT_ROUNDS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|count| *count > 0)
        .unwrap_or(9)
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

/// One sample every this many operations.
///
/// A timestamp per operation would dominate a read that costs tens of
/// nanoseconds, so the sample rate is part of the measurement rather than a
/// detail. Every task keeps its own buffer and merges once at the end, so the
/// shared lock is taken once per task instead of once per sample.
const SAMPLE_EVERY: u64 = 1024;

async fn read_task(
    table: Arc<MixedWorkTable>,
    seed: u64,
    hits: Arc<AtomicU64>,
    latency: Arc<std::sync::Mutex<Vec<u64>>>,
) {
    let mut state = seed | 1;
    let mut found = 0u64;
    let mut mine = Vec::with_capacity((ops_per_task() / SAMPLE_EVERY) as usize + 1);
    for index in 0..ops_per_task() {
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
    table: Arc<MixedWorkTable>,
    seed: u64,
    done: Arc<AtomicU64>,
    latency: Arc<std::sync::Mutex<Vec<u64>>>,
) {
    let mut state = seed | 1;
    let mut applied = 0u64;
    let mut mine = Vec::with_capacity((ops_per_task() / SAMPLE_EVERY) as usize + 1);
    for index in 0..ops_per_task() {
        let id = key(&mut state);
        let at = index.is_multiple_of(SAMPLE_EVERY).then(Instant::now);
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
        if let Some(at) = at {
            mine.push(u64::try_from(at.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
    }
    done.fetch_add(applied, Ordering::Relaxed);
    latency.lock().expect("the samples").extend(mine);
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
    let read_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let write_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));

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
            Arc::clone(&read_latency),
        )));
    }
    for n in 0..writers() {
        handles.push(write_pool.spawn(write_task(
            Arc::clone(&table),
            0xD1B5_4A32_D192_ED03 ^ n as u64,
            Arc::clone(&writes),
            Arc::clone(&write_latency),
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
    emit_row(
        &read_latency,
        &write_latency,
        format!("reads={} writes={}", read_on.name(), write_on.name()),
        "nagoya",
        started.elapsed().as_nanos(),
        (reads + written) / elapsed,
        reads / elapsed,
        written / elapsed,
        cpu / elapsed,
    );
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
    let read_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let write_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));

    let cpu_before = wt_benchmarks::cpu::cpu_seconds();
    let started = Instant::now();
    runtime.block_on(async {
        let mut handles = Vec::with_capacity(readers() + writers());
        for n in 0..readers() {
            handles.push(tokio::spawn(read_task(
                Arc::clone(&table),
                0x9E37_79B9_7F4A_7C15 ^ n as u64,
                Arc::clone(&hits),
                Arc::clone(&read_latency),
            )));
        }
        for n in 0..writers() {
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
    let elapsed = started.elapsed().as_secs_f64();
    let cpu = wt_benchmarks::cpu::cpu_seconds() - cpu_before;
    let reads = readers() as f64 * ops_per_task() as f64;
    let written = writers() as f64 * ops_per_task() as f64;
    assert!(hits.load(Ordering::Relaxed) > 0, "the tokio read arm found nothing");
    emit_row(
        &read_latency,
        &write_latency,
        "tokio multi_thread".to_owned(),
        "tokio",
        started.elapsed().as_nanos(),
        (reads + written) / elapsed,
        reads / elapsed,
        written / elapsed,
        cpu / elapsed,
    );
    (reads / elapsed, written / elapsed, cpu / elapsed)
}

/// One grid row per arm per repetition, so this benchmark can be summarised
/// by the same tool as every other and compared with a range rather than a
/// median. The printed table stays: it is what a person reads.
fn emit_row(
    read_latency: &Arc<std::sync::Mutex<Vec<u64>>>,
    write_latency: &Arc<std::sync::Mutex<Vec<u64>>>,
    tuning: String,
    runtime: &str,
    elapsed_ns: u128,
    ops_per_second: f64,
    read_ops_per_second: f64,
    write_ops_per_second: f64,
    cpu_x: f64,
) {
    wt_benchmarks::grid::GridRow {
        schema_version: 1,
        suite: "mixed-runtime",
        runtime: runtime.to_owned(),
        tuning,
        dispatch: "pool",
        repetition: wt_benchmarks::grid::repetition(),
        page_size: None,
        worker_threads: std::env::var("WT_RUNTIME_WORKERS")
            .ok()
            .and_then(|raw| raw.trim().parse().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, std::num::NonZeroUsize::get)),
        readers: readers(),
        writers: writers(),
        ops_per_task: ops_per_task(),
        elapsed_ns,
        ops_per_second,
        read_ops_per_second,
        write_ops_per_second,
        cpu_x,
        read_latency: wt_benchmarks::result::LatencySummary::from_samples(std::mem::take(
            &mut *read_latency.lock().expect("the samples"),
        )),
        write_latency: wt_benchmarks::result::LatencySummary::from_samples(std::mem::take(
            &mut *write_latency.lock().expect("the samples"),
        )),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    }
    .emit();
}

fn main() {
    println!(
        "{ROWS} rows, {} select tasks and {} upsert tasks live at once,\n\
         {} operations per task, arms interleaved\n",
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
    println!("  {} repetitions of every arm, interleaved\n", rounds());
    for round in 0..rounds() {
        wt_benchmarks::grid::set_repetition(round + 1);
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
