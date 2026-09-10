//! **Persisted WorkTable, every dimension at once.**
//!
//! Page size x runtime x concurrency, with throughput, cores busy, and p50 and
//! p99 for reads and writes, emitted as one structured row per cell.
//!
//! # Why persisted is the case that matters for page size
//!
//! On an in-memory table page size only decides how rows are packed, and
//! measuring it found nothing: a -4.3% to +8.6% band across eleven rounds,
//! with identical read latency at every size. On a persisted table **a page is
//! the unit that gets written**, so page size sets write amplification
//! directly, and that is the effect the setting exists for.
//!
//! # Why the runtime should matter here more, not less
//!
//! A persisted table has a background flush loop, which is one of only two
//! places the engine spawns. An in-memory table has neither, so the runtime
//! only ever saw the client tasks. Here it schedules the engine's own work
//! against the client's, which is the contention a real deployment has.
//!
//! # Reading the output
//!
//! `WT_JSON=1` gives one JSON row per cell, then `scripts/pivot.sh` renders it
//! as markdown, TOON or TSV by whichever dimensions you name. Nothing here
//! needs to be parsed out of a formatted table.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::grid::GridRow;
use wt_benchmarks::result::LatencySummary;

const ROWS: u64 = 20_000;
const SAMPLE_EVERY: u64 = 16;

fn readers() -> usize {
    env_count("WT_READERS", 6)
}

fn writers() -> usize {
    env_count("WT_WRITERS", 2)
}

fn ops() -> u64 {
    std::env::var("WT_OPS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(40_000 / (readers() + writers()) as u64)
}

fn rounds() -> usize {
    env_count("WT_ROUNDS_N", 3)
}

fn worker_threads() -> usize {
    std::env::var("WT_RUNTIME_WORKERS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, std::num::NonZeroUsize::get))
}

fn env_count(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|count| *count > 0)
        .unwrap_or(fallback)
}

/// Which runtime drives the client tasks.
#[derive(Clone, Copy)]
enum Driver {
    /// Reads and writes on separate nagoya pools, the split the mixed-runtime
    /// benchmark found worth 16% to 41% above six concurrent tasks.
    NagoyaSplit,
    /// One nagoya pool at the default flavor, so the split can be told apart
    /// from nagoya-versus-tokio.
    NagoyaDefault,
    /// One tokio runtime. The table's internals are still `NagoyaRt`, so this
    /// is a tokio-driven client, which is the question an existing tokio
    /// service has.
    Tokio,
}

impl Driver {
    const ALL: [Driver; 3] = [Driver::NagoyaSplit, Driver::NagoyaDefault, Driver::Tokio];

    const fn runtime(self) -> &'static str {
        match self {
            Driver::NagoyaSplit | Driver::NagoyaDefault => "nagoya",
            Driver::Tokio => "tokio",
        }
    }

    const fn tuning(self) -> &'static str {
        match self {
            Driver::NagoyaSplit => "split reads=spread writes=locality",
            Driver::NagoyaDefault => "uniform shared_slot",
            Driver::Tokio => "tokio multi_thread",
        }
    }
}

fn key(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state % ROWS
}

// Four page sizes, spanning the default. 16384 is what an omitted `config`
// block gives, from `data_bucket::PAGE_SIZE`, which is `4096 * 4`. It is worth
// saying because the value is easy to assume is 4096 and is not.
worktable!(
    name: Disk4k,
    persist: true,
    columns: { id: u64 primary_key, payload: u64 },
    config: { page_size: 4096 },
);
worktable!(
    name: Disk8k,
    persist: true,
    columns: { id: u64 primary_key, payload: u64 },
    config: { page_size: 8192 },
);
worktable!(
    name: Disk16k,
    persist: true,
    columns: { id: u64 primary_key, payload: u64 },
    config: { page_size: 16384 },
);
worktable!(
    name: Disk32k,
    persist: true,
    columns: { id: u64 primary_key, payload: u64 },
    config: { page_size: 32768 },
);

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
                let _ = table.upsert($row { id, payload: state }).await;
                if let Some(at) = at {
                    mine.push(u64::try_from(at.elapsed().as_nanos()).unwrap_or(u64::MAX));
                }
            }
            latency.lock().expect("the samples").extend(mine);
        }
    }};
}

/// One cell: build a fresh on-disk table, load it, run the mixed workload,
/// emit the row.
///
/// A fresh directory per cell, because a table that reloaded a previous cell's
/// pages would be measuring a warm file rather than the page size under test.
macro_rules! measure {
    ($table:ident, $engine:ident, $row:ident, $driver:expr, $page:literal) => {{
        let driver: Driver = $driver;
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().to_string_lossy().to_string();
        let config = DiskConfig::new_with_table_name(&path, $table::name_snake_case(), $table::version());

        let table = Arc::new(
            futures::executor::block_on(async {
                let engine = $engine::new(config).await.expect("an engine");
                $table::load(engine).await.expect("a table")
            }),
        );
        for id in 0..ROWS {
            futures::executor::block_on(table.insert($row { id, payload: id })).expect("fresh key");
        }

        let hits = Arc::new(AtomicU64::new(0));
        let read_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let write_latency = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));

        let cpu_before = wt_benchmarks::cpu::cpu_seconds();
        let started = Instant::now();
        match driver {
            Driver::NagoyaSplit | Driver::NagoyaDefault => {
                let (read_pool, write_pool) = match driver {
                    Driver::NagoyaSplit => (
                        executor_for_flavor(Flavor::Spread),
                        executor_for_flavor(Flavor::Locality),
                    ),
                    _ => {
                        let one = executor_for_flavor(Flavor::default());
                        (one, one)
                    }
                };
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
                    .enable_all()
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
        // Taken before the close: a close drains the queue, and charging that
        // to the measured window would make a slower page size look faster by
        // leaving more of its work for the close to do.
        let elapsed = started.elapsed().as_secs_f64();
        let measured_ns = started.elapsed().as_nanos();
        let cpu = wt_benchmarks::cpu::cpu_seconds() - cpu_before;
        assert!(hits.load(Ordering::Relaxed) > 0, "the read arm found nothing");

        // **Close before the directory goes away.** `TempDir` deletes on drop
        // and the engine's flush loop is still running at that point, so
        // dropping the cell without closing pulled the files out from under a
        // live background task: six workers panicked in
        // `persistence::space::data` with "should be available as pages parsed
        // from these ids". That was this harness, not the engine, and the
        // giveaway was that it happened with one writer and not with eight.
        //
        // `close` returning `Ok` is the only proof everything reached disk;
        // `wait_for_ops` deliberately is not, because it can return while the
        // operation a close would drain is still in flight.
        let table = Arc::try_unwrap(table).unwrap_or_else(|_| panic!("the tasks are joined, so this is the last handle"));
        futures::executor::block_on(table.close()).expect("a clean close");
        drop(dir);

        let total = (readers() + writers()) as f64 * ops() as f64;
        let reads = LatencySummary::from_samples(std::mem::take(
            &mut *read_latency.lock().expect("the samples"),
        ));
        let writes = LatencySummary::from_samples(std::mem::take(
            &mut *write_latency.lock().expect("the samples"),
        ));
        GridRow {
            schema_version: 1,
            suite: "persisted-grid",
            runtime: driver.runtime().to_owned(),
            tuning: driver.tuning().to_owned(),
            dispatch: "pool",
        // No secondary index on this table, so the backend axis does not
        // apply. Named rather than left blank.
        index_backend: "none",
            repetition: wt_benchmarks::grid::repetition(),
            page_size: Some($page),
            worker_threads: worker_threads(),
            readers: readers(),
            writers: writers(),
            ops_per_task: ops(),
            elapsed_ns: measured_ns,
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
            "persisted, {ROWS} rows, {} select and {} upsert tasks live at once,\n\
             {} operations per task, {} worker threads, median of {},\n\
             page sizes and runtimes interleaved, latency sampled 1 in {SAMPLE_EVERY}\n\
             the default page size is 16384, from `data_bucket::PAGE_SIZE`\n",
            readers(),
            writers(),
            ops(),
            worker_threads(),
            rounds()
        );
    }

    type Sample = (f64, f64, LatencySummary, LatencySummary);
    let sizes: [u32; 4] = [4096, 8192, 16384, 32768];
    let mut cells: Vec<Vec<Sample>> = (0..sizes.len() * Driver::ALL.len()).map(|_| Vec::new()).collect();

    for round in 0..rounds() {
        wt_benchmarks::grid::set_repetition(round as u32 + 1);
        for (index, driver) in Driver::ALL.into_iter().enumerate() {
            cells[index].push(measure!(Disk4kWorkTable, Disk4kPersistenceEngine, Disk4kRow, driver, 4096));
            cells[Driver::ALL.len() + index]
                .push(measure!(Disk8kWorkTable, Disk8kPersistenceEngine, Disk8kRow, driver, 8192));
            cells[Driver::ALL.len() * 2 + index]
                .push(measure!(Disk16kWorkTable, Disk16kPersistenceEngine, Disk16kRow, driver, 16384));
            cells[Driver::ALL.len() * 3 + index]
                .push(measure!(Disk32kWorkTable, Disk32kPersistenceEngine, Disk32kRow, driver, 32768));
        }
    }

    if !human {
        return;
    }

    let pick = |mut values: Vec<Sample>| {
        values.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("no NaN"));
        values.swap_remove(values.len() / 2)
    };
    let ns = |value: Option<u64>| value.map_or("-".to_owned(), |v| format!("{v}"));

    println!(
        "  {:<10} {:<34} {:>13} {:>7} {:>9} {:>9} {:>10} {:>10}",
        "page_size", "runtime", "total ops/s", "cores", "read p50", "read p99", "write p50", "write p99"
    );
    for (size_index, size) in sizes.iter().enumerate() {
        for (index, driver) in Driver::ALL.into_iter().enumerate() {
            let (ops_s, cores, reads, writes) = pick(cells[size_index * Driver::ALL.len() + index].clone());
            let marker = if *size == 16384 { "*" } else { " " };
            println!(
                "  {size:<9}{marker} {:<34} {ops_s:>13.0} {cores:>6.2}x {:>9} {:>9} {:>10} {:>10}",
                driver.tuning(),
                ns(reads.p50_ns),
                ns(reads.p99_ns),
                ns(writes.p50_ns),
                ns(writes.p99_ns),
            );
        }
    }
    println!("\n  * the default page size");
}
