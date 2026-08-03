use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use worktable::prelude::*;
use worktable::worktable;

struct CountingAllocator;

static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

// SAFETY: every operation delegates to `System` with the original layout and
// only updates independent atomic accounting after a successful allocation.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let size = layout.size() as u64;
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(size, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            let size = layout.size() as u64;
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(size, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let size = layout.size() as u64;
        DEALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        DEALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
        LIVE_BYTES.fetch_sub(size, Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, old_layout: Layout, new_size: usize) -> *mut u8 {
        let new_pointer = unsafe { System.realloc(pointer, old_layout, new_size) };
        if !new_pointer.is_null() {
            let old_size = old_layout.size() as u64;
            let new_size = new_size as u64;
            REALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size, Ordering::Relaxed);
            DEALLOCATED_BYTES.fetch_add(old_size, Ordering::Relaxed);
            if new_size >= old_size {
                LIVE_BYTES.fetch_add(new_size - old_size, Ordering::Relaxed);
            } else {
                LIVE_BYTES.fetch_sub(old_size - new_size, Ordering::Relaxed);
            }
        }
        new_pointer
    }
}

type Payload = [u8; 32];

worktable!(
    name: BackendWti,
    persist: false,
    columns: {
        id: u64 primary_key autoincrement using worktables_index,
        lookup: u64,
        payload: Payload,
    },
    indexes: {
        lookup_idx: lookup unique using worktables_index,
    }
);

worktable!(
    name: BackendIndexset,
    persist: false,
    columns: {
        id: u64 primary_key autoincrement using indexset,
        lookup: u64,
        payload: Payload,
    },
    indexes: {
        lookup_idx: lookup unique using indexset,
    }
);

worktable!(
    name: BackendCongee,
    persist: false,
    columns: {
        id: u64 primary_key autoincrement using congee,
        lookup: u64,
        payload: Payload,
    },
    indexes: {
        lookup_idx: lookup unique using congee,
    }
);

worktable!(
    name: BackendArctic,
    persist: false,
    columns: {
        id: u64 primary_key autoincrement using arctic,
        lookup: u64,
        payload: Payload,
    },
    indexes: {
        lookup_idx: lookup unique using arctic,
    }
);

#[derive(Clone, Copy, Debug)]
enum Backend {
    WorkTablesIndex,
    Indexset,
    Congee,
    Arctic,
}

impl Backend {
    fn as_str(self) -> &'static str {
        match self {
            Self::WorkTablesIndex => "worktables_index",
            Self::Indexset => "indexset",
            Self::Congee => "congee",
            Self::Arctic => "arctic",
        }
    }
}

impl FromStr for Backend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "worktables_index" | "wti" | "default" => Ok(Self::WorkTablesIndex),
            "indexset" => Ok(Self::Indexset),
            "congee" => Ok(Self::Congee),
            "arctic" => Ok(Self::Arctic),
            _ => Err(format!("unknown backend: {value}")),
        }
    }
}

#[derive(Clone, Debug)]
struct Config {
    backend: Backend,
    rows: u64,
    operations: u64,
    mutations: u64,
    sample_every: u64,
    repetition: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: Backend::WorkTablesIndex,
            rows: 100_000,
            operations: 1_000_000,
            mutations: 50_000,
            sample_every: 128,
            repetition: 1,
        }
    }
}

impl Config {
    fn from_args() -> Result<Self, String> {
        let mut config = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            if flag == "--help" || flag == "-h" {
                println!(
                    "index-backends options:\n\
                     --backend worktables_index|indexset|congee|arctic\n\
                     --rows N            initial sequential rows (default 100000)\n\
                     --operations N      point reads per phase (default 1000000)\n\
                     --mutations N       inserts/deletes per phase (default 50000)\n\
                     --sample-every N    latency sample interval (default 128)\n\
                     --repetition N      result label (default 1)"
                );
                std::process::exit(0);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--backend" => config.backend = value.parse()?,
                "--rows" => config.rows = parse(&flag, &value)?,
                "--operations" => config.operations = parse(&flag, &value)?,
                "--mutations" => config.mutations = parse(&flag, &value)?,
                "--sample-every" => config.sample_every = parse(&flag, &value)?,
                "--repetition" => config.repetition = parse(&flag, &value)?,
                _ => return Err(format!("unknown option: {flag}")),
            }
        }
        if config.rows == 0
            || config.operations == 0
            || config.mutations == 0
            || config.sample_every == 0
            || config.repetition == 0
        {
            return Err("counts, sampling interval, and repetition must be non-zero".into());
        }
        config
            .rows
            .checked_add(config.mutations)
            .ok_or_else(|| "--rows plus --mutations overflows u64".to_owned())?;
        Ok(config)
    }
}

fn parse<T>(flag: &str, value: &str) -> Result<T, String>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| format!("invalid value for {flag}: {error}"))
}

#[derive(Clone, Copy)]
struct AllocationSnapshot {
    alloc_calls: u64,
    realloc_calls: u64,
    dealloc_calls: u64,
    allocated_bytes: u64,
    deallocated_bytes: u64,
    live_bytes: u64,
}

fn reset_allocation_events() {
    ALLOC_CALLS.store(0, Ordering::Relaxed);
    REALLOC_CALLS.store(0, Ordering::Relaxed);
    DEALLOC_CALLS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    DEALLOCATED_BYTES.store(0, Ordering::Relaxed);
}

fn allocations() -> AllocationSnapshot {
    AllocationSnapshot {
        alloc_calls: ALLOC_CALLS.load(Ordering::Relaxed),
        realloc_calls: REALLOC_CALLS.load(Ordering::Relaxed),
        dealloc_calls: DEALLOC_CALLS.load(Ordering::Relaxed),
        allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        deallocated_bytes: DEALLOCATED_BYTES.load(Ordering::Relaxed),
        live_bytes: LIVE_BYTES.load(Ordering::Relaxed),
    }
}

#[derive(Debug, Serialize)]
struct LatencySummary {
    samples: usize,
    p50_ns: Option<u64>,
    p95_ns: Option<u64>,
    p99_ns: Option<u64>,
    p999_ns: Option<u64>,
    max_ns: Option<u64>,
}

impl LatencySummary {
    fn from_samples(mut samples: Vec<u64>) -> Self {
        samples.sort_unstable();
        Self {
            samples: samples.len(),
            p50_ns: percentile(&samples, 0.50),
            p95_ns: percentile(&samples, 0.95),
            p99_ns: percentile(&samples, 0.99),
            p999_ns: percentile(&samples, 0.999),
            max_ns: samples.last().copied(),
        }
    }
}

fn percentile(samples: &[u64], fraction: f64) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let rank = ((samples.len() - 1) as f64 * fraction).ceil() as usize;
    samples.get(rank).copied()
}

#[derive(Serialize)]
struct PhaseResult {
    schema_version: u32,
    suite: &'static str,
    backend: &'static str,
    operation: &'static str,
    repetition: usize,
    rows_initial: u64,
    operations: u64,
    sequential_u64_keys: bool,
    persistence: bool,
    payload_bytes: usize,
    elapsed_ns: u128,
    ops_per_second: f64,
    sample_every: u64,
    latency: LatencySummary,
    alloc_calls: u64,
    realloc_calls: u64,
    dealloc_calls: u64,
    allocated_bytes: u64,
    deallocated_bytes: u64,
    allocation_calls_per_op: f64,
    allocated_bytes_per_op: f64,
    live_bytes_before: u64,
    live_bytes_after: u64,
    live_bytes_delta: i128,
    rss_bytes_before: Option<u64>,
    rss_bytes_after: Option<u64>,
    rss_bytes_delta: Option<i128>,
    target_arch: &'static str,
    target_os: &'static str,
}

#[allow(clippy::too_many_arguments)]
fn emit(
    config: &Config,
    operation: &'static str,
    operations: u64,
    elapsed_ns: u128,
    samples: Vec<u64>,
    allocation: AllocationSnapshot,
    live_before: u64,
    rss_before: Option<u64>,
    rss_after: Option<u64>,
) {
    let result = PhaseResult {
        schema_version: 1,
        suite: "worktable-index-backends",
        backend: config.backend.as_str(),
        operation,
        repetition: config.repetition,
        rows_initial: config.rows,
        operations,
        sequential_u64_keys: true,
        persistence: false,
        payload_bytes: std::mem::size_of::<Payload>(),
        elapsed_ns,
        ops_per_second: operations as f64 / (elapsed_ns as f64 / 1_000_000_000.0),
        sample_every: config.sample_every,
        latency: LatencySummary::from_samples(samples),
        alloc_calls: allocation.alloc_calls,
        realloc_calls: allocation.realloc_calls,
        dealloc_calls: allocation.dealloc_calls,
        allocated_bytes: allocation.allocated_bytes,
        deallocated_bytes: allocation.deallocated_bytes,
        allocation_calls_per_op: (allocation.alloc_calls + allocation.realloc_calls) as f64
            / operations as f64,
        allocated_bytes_per_op: allocation.allocated_bytes as f64 / operations as f64,
        live_bytes_before: live_before,
        live_bytes_after: allocation.live_bytes,
        live_bytes_delta: allocation.live_bytes as i128 - live_before as i128,
        rss_bytes_before: rss_before,
        rss_bytes_after: rss_after,
        rss_bytes_delta: rss_before
            .zip(rss_after)
            .map(|(before, after)| after as i128 - before as i128),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    };
    println!(
        "{}",
        serde_json::to_string(&result).expect("result must serialize")
    );
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn rss_bytes() -> Option<u64> {
    let mut info = unsafe { std::mem::zeroed::<libc::mach_task_basic_info>() };
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    let status = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            (&raw mut info).cast::<libc::integer_t>(),
            &mut count,
        )
    };
    (status == libc::KERN_SUCCESS).then_some(info.resident_size)
}

#[cfg(not(target_os = "macos"))]
fn rss_bytes() -> Option<u64> {
    None
}

fn sample(samples: &mut Vec<u64>, started: Option<Instant>) {
    if let Some(started) = started {
        samples.push(started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
    }
}

macro_rules! run_backend {
    ($config:expr, $table:ty, $row:ident) => {{
        let config = $config;
        let table = <$table>::default();

        let mut samples = Vec::with_capacity((config.rows / config.sample_every + 1) as usize);
        let rss_before = rss_bytes();
        let live_before = allocations().live_bytes;
        reset_allocation_events();
        let started = Instant::now();
        for sequence in 0..config.rows {
            let operation_started = sequence
                .is_multiple_of(config.sample_every)
                .then(Instant::now);
            table
                .insert($row {
                    id: table.get_next_pk().into(),
                    lookup: sequence,
                    payload: [sequence as u8; 32],
                })
                .expect("sequential backend load must insert");
            sample(&mut samples, operation_started);
        }
        let elapsed_ns = started.elapsed().as_nanos();
        let allocation = allocations();
        let rss_after = rss_bytes();
        emit(
            config,
            "insert_initial",
            config.rows,
            elapsed_ns,
            samples,
            allocation,
            live_before,
            rss_before,
            rss_after,
        );

        let mut samples =
            Vec::with_capacity((config.operations / config.sample_every + 1) as usize);
        let rss_before = rss_bytes();
        let live_before = allocations().live_bytes;
        reset_allocation_events();
        let started = Instant::now();
        let mut checksum = 0_u64;
        for sequence in 0..config.operations {
            let operation_started = sequence
                .is_multiple_of(config.sample_every)
                .then(Instant::now);
            let row = black_box(table.select(sequence % config.rows)).expect("loaded primary key");
            checksum = checksum.wrapping_add(row.lookup);
            sample(&mut samples, operation_started);
        }
        black_box(checksum);
        let elapsed_ns = started.elapsed().as_nanos();
        let allocation = allocations();
        let rss_after = rss_bytes();
        emit(
            config,
            "primary_point_hit",
            config.operations,
            elapsed_ns,
            samples,
            allocation,
            live_before,
            rss_before,
            rss_after,
        );

        let mut samples =
            Vec::with_capacity((config.operations / config.sample_every + 1) as usize);
        let rss_before = rss_bytes();
        let live_before = allocations().live_bytes;
        reset_allocation_events();
        let started = Instant::now();
        let mut checksum = 0_u64;
        for sequence in 0..config.operations {
            let operation_started = sequence
                .is_multiple_of(config.sample_every)
                .then(Instant::now);
            let row = black_box(table.select_by_lookup(sequence % config.rows))
                .expect("loaded unique key");
            checksum = checksum.wrapping_add(row.lookup);
            sample(&mut samples, operation_started);
        }
        black_box(checksum);
        let elapsed_ns = started.elapsed().as_nanos();
        let allocation = allocations();
        let rss_after = rss_bytes();
        emit(
            config,
            "unique_point_hit",
            config.operations,
            elapsed_ns,
            samples,
            allocation,
            live_before,
            rss_before,
            rss_after,
        );

        let mut samples =
            Vec::with_capacity((config.operations / config.sample_every + 1) as usize);
        let rss_before = rss_bytes();
        let live_before = allocations().live_bytes;
        reset_allocation_events();
        let started = Instant::now();
        let mut misses = 0_u64;
        for sequence in 0..config.operations {
            let operation_started = sequence
                .is_multiple_of(config.sample_every)
                .then(Instant::now);
            misses += u64::from(
                black_box(table.select(config.rows + config.mutations + sequence % config.rows))
                    .is_none(),
            );
            sample(&mut samples, operation_started);
        }
        assert_eq!(misses, config.operations);
        let elapsed_ns = started.elapsed().as_nanos();
        let allocation = allocations();
        let rss_after = rss_bytes();
        emit(
            config,
            "primary_point_miss",
            config.operations,
            elapsed_ns,
            samples,
            allocation,
            live_before,
            rss_before,
            rss_after,
        );

        let mut inserted_keys = Vec::with_capacity(config.mutations as usize);
        let mut samples = Vec::with_capacity((config.mutations / config.sample_every + 1) as usize);
        let rss_before = rss_bytes();
        let live_before = allocations().live_bytes;
        reset_allocation_events();
        let started = Instant::now();
        for sequence in 0..config.mutations {
            let operation_started = sequence
                .is_multiple_of(config.sample_every)
                .then(Instant::now);
            inserted_keys.push(
                table
                    .insert($row {
                        id: table.get_next_pk().into(),
                        lookup: config.rows + sequence,
                        payload: [sequence as u8; 32],
                    })
                    .expect("sequential mutation key must insert"),
            );
            sample(&mut samples, operation_started);
        }
        let elapsed_ns = started.elapsed().as_nanos();
        let allocation = allocations();
        let rss_after = rss_bytes();
        emit(
            config,
            "insert_steady",
            config.mutations,
            elapsed_ns,
            samples,
            allocation,
            live_before,
            rss_before,
            rss_after,
        );

        let mut samples = Vec::with_capacity((config.mutations / config.sample_every + 1) as usize);
        let rss_before = rss_bytes();
        let live_before = allocations().live_bytes;
        reset_allocation_events();
        let started = Instant::now();
        for (sequence, key) in inserted_keys.into_iter().enumerate() {
            let operation_started = (sequence as u64)
                .is_multiple_of(config.sample_every)
                .then(Instant::now);
            table
                .delete(key)
                .await
                .expect("inserted mutation key must delete");
            sample(&mut samples, operation_started);
        }
        let elapsed_ns = started.elapsed().as_nanos();
        let allocation = allocations();
        let rss_after = rss_bytes();
        emit(
            config,
            "delete_steady",
            config.mutations,
            elapsed_ns,
            samples,
            allocation,
            live_before,
            rss_before,
            rss_after,
        );
    }};
}

#[tokio::main]
async fn main() {
    let config = Config::from_args().unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    match config.backend {
        Backend::WorkTablesIndex => run_backend!(&config, BackendWtiWorkTable, BackendWtiRow),
        Backend::Indexset => {
            run_backend!(&config, BackendIndexsetWorkTable, BackendIndexsetRow)
        }
        Backend::Congee => run_backend!(&config, BackendCongeeWorkTable, BackendCongeeRow),
        Backend::Arctic => run_backend!(&config, BackendArcticWorkTable, BackendArcticRow),
    }
}
