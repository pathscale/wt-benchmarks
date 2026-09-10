//! **What `config: { page_size: N }` is worth**, at the best runtime selection.
//!
//! `page_size` has been in the DSL since 2024 and no table in any consumer
//! project sets it. This measures whether that is a missed knob or a
//! non-event.
//!
//! # What page size actually controls here
//!
//! It sizes the data pages rows are packed into. A small page means more
//! pages for the same rows, so more page boundaries to cross on a scan and
//! more page headers per byte of payload. A large page means fewer, bigger
//! allocations and more of a page touched to reach one row.
//!
//! These tables are `persist: false`, so this isolates the **in-memory
//! layout** effect. On a persisted table page size also decides write
//! amplification, because a page is the unit that gets written, and that is
//! very likely the larger effect. It needs its own benchmark and this is not
//! it.
//!
//! # The runtime
//!
//! Reads and writes go to separate pools, which the mixed-runtime benchmark
//! found is worth 16% to 41% above six concurrent tasks. Page size is measured
//! on top of that rather than instead of it, so the number is what a tuned
//! deployment would actually see.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::grid::GridRow;
use wt_benchmarks::result::LatencySummary;

const ROWS: u64 = 100_000;
/// Concurrency, from the environment so one binary covers the sweep.
fn readers() -> usize {
    env_count("WT_READERS", 12)
}

fn writers() -> usize {
    env_count("WT_WRITERS", 4)
}

/// Operations per task, scaled so total work stays roughly constant as
/// concurrency rises and every point takes about the same wall time.
fn ops() -> u64 {
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

fn worker_threads() -> usize {
    std::env::var("WT_RUNTIME_WORKERS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, std::num::NonZeroUsize::get))
}
/// Latency is sampled rather than measured on every operation: an
/// `Instant::now` pair costs more than a select does, so timing all of them
/// would report the clock. One in 64 is enough for a p99 over 320,000
/// operations and cheap enough not to move the throughput number.
const SAMPLE_EVERY: u64 = 64;

/// Rounds, from the environment. Three was not enough: the first run put
/// 4096 18% below sizes either side of it, which is not a shape a layout
/// effect has, so the number was noise rather than signal.
fn rounds() -> usize {
    std::env::var("WT_ROUNDS_N")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(3)
}

/// Which runtime drives the client tasks.
#[derive(Clone, Copy)]
enum Driver {
    /// Reads on `spread`, writes on `locality`, which is the split the
    /// mixed-runtime benchmark found worth 16% to 41% above six tasks.
    Nagoya,
    /// One tokio runtime. The table's internals are still `NagoyaRt`, so this
    /// is a tokio-driven *client*, which is the question an existing tokio
    /// service has.
    Tokio,
}

/// The read and write bodies, as macros so that both drivers run byte
/// identical code and any difference is the runtime.
macro_rules! read_body {
    ($table:expr, $hits:expr, $latency:expr, $n:expr) => {{
        let table = Arc::clone(&$table);
        let hits = Arc::clone(&$hits);
        let latency = Arc::clone(&$latency);
        let n = $n;
        async move {
            let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ n as u64 | 1;
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
    }};
}

macro_rules! write_body {
    ($table:expr, $row:ident, $latency:expr, $n:expr) => {{
        let table = Arc::clone(&$table);
        let latency = Arc::clone(&$latency);
        let n = $n;
        async move {
            let mut state = 0xD1B5_4A32_D192_ED03u64 ^ n as u64 | 1;
            let mut mine = Vec::with_capacity((ops() / SAMPLE_EVERY) as usize + 1);
            for index in 0..ops() {
                let id = key(&mut state);
                let at = index.is_multiple_of(SAMPLE_EVERY).then(Instant::now);
                let _ = table
                    .upsert($row { id, value: state, payload: String::new() })
                    .await;
                if let Some(at) = at {
                    mine.push(u64::try_from(at.elapsed().as_nanos()).unwrap_or(u64::MAX));
                }
            }
            latency.lock().expect("the samples").extend(mine);
        }
    }};
}

/// One table per page size.
///
/// Written out rather than generated by a `macro_rules!`, because
/// `worktable!` needs a real `Literal` token for `page_size` and a
/// `$bytes:literal` metavariable arrives wrapped in an invisible delimiter
/// group that its parser rejects. Five near-identical declarations is the
/// honest cost of the value being a compile-time constant.
worktable!(
    name: Page1k,
    persist: false,
    columns: { id: u64 primary_key, value: u64, payload: String },
    config: { page_size: 1024 },
);
worktable!(
    name: Page4k,
    persist: false,
    columns: { id: u64 primary_key, value: u64, payload: String },
    config: { page_size: 4096 },
);
worktable!(
    name: Page8k,
    persist: false,
    columns: { id: u64 primary_key, value: u64, payload: String },
    config: { page_size: 8192 },
);
worktable!(
    name: Page16k,
    persist: false,
    columns: { id: u64 primary_key, value: u64, payload: String },
    config: { page_size: 16384 },
);
worktable!(
    name: Page32k,
    persist: false,
    columns: { id: u64 primary_key, value: u64, payload: String },
    config: { page_size: 32768 },
);

fn key(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state % ROWS
}

/// Runs the mixed workload against one table type and returns total ops/s and
/// cores busy.
macro_rules! measure {
    ($table:ident, $row:ident, $driver:expr, $page:literal) => {{
        let table = Arc::new($table::default());
        for id in 0..ROWS {
            futures::executor::block_on(table.insert($row {
                id,
                value: id,
                payload: String::new(),
            }))
            .expect("fresh key");
        }
        let hits = Arc::new(AtomicU64::new(0));
        let read_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let write_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        // `Driver::Nagoya` uses the selection the mixed-runtime benchmark
        // settled on, reads and writes on separate pools. `Driver::Tokio`
        // runs the identical tasks on one tokio runtime, because tokio has no
        // equivalent of the split and giving it two runtimes would make the
        // comparison about thread count instead of scheduling.
        let driver: Driver = $driver;

        let cpu_before = wt_benchmarks::cpu::cpu_seconds();
        let started = Instant::now();
        match driver {
            Driver::Nagoya => {
                let read_pool = executor_for_flavor(Flavor::Spread);
                let write_pool = executor_for_flavor(Flavor::Locality);
                let mut handles = Vec::with_capacity(readers() + writers());
                for n in 0..readers() {
                    handles.push(read_pool.spawn(read_body!(table, hits, read_latency, n)));
                }
                for n in 0..writers() {
                    handles.push(write_pool.spawn(write_body!(table, $row, write_latency, n)));
                }
                for handle in handles {
                    futures::executor::block_on(handle);
                }
            }
            Driver::Tokio => {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(worker_threads())
                    .build()
                    .expect("a tokio runtime");
                runtime.block_on(async {
                    let mut handles = Vec::with_capacity(readers() + writers());
                    for n in 0..readers() {
                        handles.push(tokio::spawn(read_body!(table, hits, read_latency, n)));
                    }
                    for n in 0..writers() {
                        handles.push(tokio::spawn(write_body!(table, $row, write_latency, n)));
                    }
                    for handle in handles {
                        handle.await.expect("a task");
                    }
                });
            }
        }
        let elapsed = started.elapsed().as_secs_f64();
        let cpu = wt_benchmarks::cpu::cpu_seconds() - cpu_before;
        assert!(hits.load(Ordering::Relaxed) > 0, "the read arm found nothing");
        let total = (readers() + writers()) as f64 * ops() as f64;
        let reads = LatencySummary::from_samples(std::mem::take(
            &mut *read_latency.lock().expect("the samples"),
        ));
        let writes = LatencySummary::from_samples(std::mem::take(
            &mut *write_latency.lock().expect("the samples"),
        ));
        GridRow {
            schema_version: 1,
            suite: "page-size",
            runtime: match driver {
                Driver::Nagoya => "nagoya".to_owned(),
                Driver::Tokio => "tokio".to_owned(),
            },
            tuning: match driver {
                Driver::Nagoya => "reads=spread writes=locality".to_owned(),
                Driver::Tokio => "tokio multi_thread".to_owned(),
            },
            dispatch: "pool",
            repetition: wt_benchmarks::grid::repetition(),
            page_size: Some($page),
            worker_threads: worker_threads(),
            readers: readers(),
            writers: writers(),
            ops_per_task: ops(),
            elapsed_ns: started.elapsed().as_nanos(),
            ops_per_second: total / elapsed,
            read_ops_per_second: readers() as f64 * ops() as f64 / elapsed,
            write_ops_per_second: writers() as f64 * ops() as f64 / elapsed,
            cpu_x: cpu / elapsed,
            read_latency: reads.clone(),
            write_latency: writes.clone(),
            target_arch: std::env::consts::ARCH,
            target_os: std::env::consts::OS,
        }
        .emit();
        (total / elapsed, cpu / elapsed, reads, writes)
    }};
}

fn main() {
    let human = !GridRow::wanted();
    if human {
    println!(
        "{ROWS} rows, {} select tasks and {} upsert tasks live at once,\n\
         {} operations per task, {} worker threads, median of {},\n\
         page sizes and runtimes interleaved, latency sampled 1 in {SAMPLE_EVERY}\n",
        readers(),
        writers(),
        ops(),
        worker_threads(),
        rounds()
    );
    }

    type Sample = (f64, f64, LatencySummary, LatencySummary);
    // Ten cells: five page sizes on each of two runtimes, every one of them
    // run once per round so a machine warming up cannot be charged to
    // whichever cell happened to run last.
    let mut samples: Vec<Vec<Sample>> = (0..10).map(|_| Vec::new()).collect();
    for round in 0..rounds() {
        wt_benchmarks::grid::set_repetition(round as u32 + 1);
        samples[0].push(measure!(Page1kWorkTable, Page1kRow, Driver::Nagoya, 1024));
        samples[1].push(measure!(Page1kWorkTable, Page1kRow, Driver::Tokio, 1024));
        samples[2].push(measure!(Page4kWorkTable, Page4kRow, Driver::Nagoya, 4096));
        samples[3].push(measure!(Page4kWorkTable, Page4kRow, Driver::Tokio, 4096));
        samples[4].push(measure!(Page8kWorkTable, Page8kRow, Driver::Nagoya, 8192));
        samples[5].push(measure!(Page8kWorkTable, Page8kRow, Driver::Tokio, 8192));
        samples[6].push(measure!(Page16kWorkTable, Page16kRow, Driver::Nagoya, 16384));
        samples[7].push(measure!(Page16kWorkTable, Page16kRow, Driver::Tokio, 16384));
        samples[8].push(measure!(Page32kWorkTable, Page32kRow, Driver::Nagoya, 32768));
        samples[9].push(measure!(Page32kWorkTable, Page32kRow, Driver::Tokio, 32768));
    }

    // Median by throughput, and the latency summary from that same run, so
    // the two columns describe one run rather than being medians of different
    // ones.
    let pick = |mut values: Vec<Sample>| {
        values.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("no NaN"));
        values.swap_remove(values.len() / 2)
    };

    println!(
        "  {:<10} {:<8} {:>14} {:>7} {:>9} {:>9} {:>10} {:>10}",
        "page_size", "runtime", "total ops/s", "cores", "read p50", "read p99", "write p50", "write p99"
    );
    let ns = |value: Option<u64>| value.map_or("-".to_owned(), |v| format!("{v}"));
    let sizes = ["1024", "4096", "8192", "16384", "32768"];
    let mut nagoya_by_size = Vec::new();
    let mut tokio_by_size = Vec::new();
    for (index, label) in sizes.iter().enumerate() {
        for (offset, runtime) in [(0usize, "nagoya"), (1, "tokio")] {
            let (ops_s, cores, reads, writes) = pick(samples[index * 2 + offset].clone());
            if offset == 0 {
                nagoya_by_size.push(ops_s);
            } else {
                tokio_by_size.push(ops_s);
            }
            println!(
                "  {label:<10} {runtime:<8} {ops_s:>14.0} {cores:>6.1}x {:>9} {:>9} {:>10} {:>10}",
                ns(reads.p50_ns),
                ns(reads.p99_ns),
                ns(writes.p50_ns),
                ns(writes.p99_ns),
            );
        }
    }

    println!("\n  page size, as a fraction of that runtime's own best:");
    for (values, name) in [(&nagoya_by_size, "nagoya"), (&tokio_by_size, "tokio ")] {
        let best = values.iter().copied().fold(0.0f64, f64::max);
        let spread: Vec<String> = values
            .iter()
            .zip(sizes)
            .map(|(value, size)| format!("{size}={:.0}%", value / best * 100.0))
            .collect();
        println!("    {name}  {}", spread.join("  "));
    }

    println!("\n  nagoya against tokio, per page size:");
    for (index, label) in sizes.iter().enumerate() {
        println!(
            "    {label:<7} {:>+7.1}%",
            (nagoya_by_size[index] / tokio_by_size[index] - 1.0) * 100.0
        );
    }
}
