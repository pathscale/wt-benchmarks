use std::hint::black_box;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use tokio::sync::Barrier;
use worktable::prelude::*;
use worktable::vacuum::WorkTableVacuum;
use worktable::worktable;

const WRITING: u64 = 1 << 63;
const GENERATION_MASK: u64 = !WRITING;

worktable!(
    name: ShadowWti,
    persist: false,
    columns: {
        id: u64 primary_key using worktables_index,
        unique_key: u64,
        bucket: u64,
        generation: u64,
        checksum: u64,
        payload: String,
    },
    indexes: {
        unique_idx: unique_key unique using worktables_index,
        bucket_idx: bucket,
    }
);

worktable!(
    name: ShadowIndexset,
    persist: false,
    columns: {
        id: u64 primary_key using indexset,
        unique_key: u64,
        bucket: u64,
        generation: u64,
        checksum: u64,
        payload: String,
    },
    indexes: {
        unique_idx: unique_key unique using indexset,
        bucket_idx: bucket,
    }
);

worktable!(
    name: ShadowCongee,
    persist: false,
    columns: {
        id: u64 primary_key using congee,
        unique_key: u64,
        bucket: u64,
        generation: u64,
        checksum: u64,
        payload: String,
    },
    indexes: {
        unique_idx: unique_key unique using congee,
        bucket_idx: bucket,
    }
);

worktable!(
    name: ShadowArctic,
    persist: false,
    columns: {
        id: u64 primary_key using arctic,
        unique_key: u64,
        bucket: u64,
        generation: u64,
        checksum: u64,
        payload: String,
    },
    indexes: {
        unique_idx: unique_key unique using arctic,
        bucket_idx: bucket,
    }
);

#[derive(Clone, Copy)]
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
            _ => Err(format!("unknown WT_INDEX_BACKEND: {value}")),
        }
    }
}

#[derive(Clone, Debug)]
struct Config {
    records: u64,
    operations: u64,
    threads: usize,
    repetitions: usize,
    payload_bytes: usize,
    buckets: u64,
    seed: u64,
    vacuum: bool,
    delete_every: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            records: 2_000,
            operations: 20_000,
            threads: 8,
            repetitions: 3,
            payload_bytes: 64,
            buckets: 16,
            seed: 42,
            vacuum: true,
            delete_every: 10,
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
                    "shadow-concurrency options:\n\
                     --records N          live rows (default 2000)\n\
                     --operations N       writer and reader operations each (default 20000)\n\
                     --threads N          total workers (default 8)\n\
                     --repetitions N      fresh-table repetitions (default 3)\n\
                     --payload-bytes N    payload width (default 64)\n\
                     --buckets N          non-unique index buckets (default 16)\n\
                     --seed N             deterministic seed (default 42)\n\
                     --vacuum true|false  overlap one vacuum (default true)\n\
                     --delete-every N     delete/reinsert every N writes; 0 disables (default 10)"
                );
                std::process::exit(0);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--records" => config.records = parse(&flag, &value)?,
                "--operations" => config.operations = parse(&flag, &value)?,
                "--threads" => config.threads = parse(&flag, &value)?,
                "--repetitions" => config.repetitions = parse(&flag, &value)?,
                "--payload-bytes" => config.payload_bytes = parse(&flag, &value)?,
                "--buckets" => config.buckets = parse(&flag, &value)?,
                "--seed" => config.seed = parse(&flag, &value)?,
                "--vacuum" => config.vacuum = parse(&flag, &value)?,
                "--delete-every" => config.delete_every = parse(&flag, &value)?,
                _ => return Err(format!("unknown option: {flag}")),
            }
        }
        if config.records == 0
            || config.operations == 0
            || config.threads == 0
            || config.repetitions == 0
            || config.payload_bytes == 0
            || config.buckets == 0
        {
            return Err(
                "records, operations, threads, repetitions, payload, and buckets must be non-zero"
                    .into(),
            );
        }
        let stride = config.records + 1;
        let maximum_generation = config.operations + 1;
        if stride.checked_mul(maximum_generation).is_none() {
            return Err("records times operations must fit u64".into());
        }
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

#[derive(Default)]
struct Counters {
    writer_errors: AtomicU64,
    writer_postcommit_primary_misses: AtomicU64,
    online_reads: AtomicU64,
    stable_primary_mismatches: AtomicU64,
    stable_unique_mismatches: AtomicU64,
    stable_stale_unique_hits: AtomicU64,
    bucket_predicate_mismatches: AtomicU64,
    torn_rows: AtomicU64,
    final_mismatches: AtomicU64,
    final_cardinality_mismatches: AtomicU64,
    final_primary_mismatches: AtomicU64,
    final_unique_mismatches: AtomicU64,
    final_stale_unique_hits: AtomicU64,
    final_bucket_mismatches: AtomicU64,
    final_bucket_missing_rows: AtomicU64,
    final_bucket_unexpected_rows: AtomicU64,
    final_bucket_duplicate_rows: AtomicU64,
}

impl Counters {
    fn failures(&self) -> u64 {
        self.writer_errors.load(Ordering::Relaxed)
            + self
                .writer_postcommit_primary_misses
                .load(Ordering::Relaxed)
            + self.stable_primary_mismatches.load(Ordering::Relaxed)
            + self.stable_unique_mismatches.load(Ordering::Relaxed)
            + self.stable_stale_unique_hits.load(Ordering::Relaxed)
            + self.bucket_predicate_mismatches.load(Ordering::Relaxed)
            + self.torn_rows.load(Ordering::Relaxed)
            + self.final_mismatches.load(Ordering::Relaxed)
    }
}

#[derive(Serialize)]
struct ResultRecord {
    schema_version: u32,
    suite: &'static str,
    backend: &'static str,
    repetition: usize,
    records: u64,
    writer_operations_requested: u64,
    reader_operations_requested: u64,
    threads: usize,
    writer_threads: usize,
    reader_threads: usize,
    payload_bytes: usize,
    buckets: u64,
    vacuum_enabled: bool,
    delete_every: u64,
    specialized_stable_index_reads: bool,
    elapsed_ns: u128,
    online_reads: u64,
    writer_errors: u64,
    writer_postcommit_primary_misses: u64,
    stable_primary_mismatches: u64,
    stable_unique_mismatches: u64,
    stable_stale_unique_hits: u64,
    bucket_predicate_mismatches: u64,
    torn_rows: u64,
    final_mismatches: u64,
    final_cardinality_mismatches: u64,
    final_primary_mismatches: u64,
    final_unique_mismatches: u64,
    final_stale_unique_hits: u64,
    final_bucket_mismatches: u64,
    final_bucket_missing_rows: u64,
    final_bucket_unexpected_rows: u64,
    final_bucket_duplicate_rows: u64,
    vacuum_errors: u64,
    vacuum_pages_processed: usize,
    vacuum_pages_freed: usize,
    passed: bool,
    target_arch: &'static str,
    target_os: &'static str,
}

fn payload_byte(id: u64, generation: u64) -> u8 {
    b'a' + ((id.wrapping_mul(17) ^ generation.wrapping_mul(29)) % 26) as u8
}

fn make_payload(id: u64, generation: u64, payload_bytes: usize) -> String {
    String::from_utf8(vec![payload_byte(id, generation); payload_bytes])
        .expect("ASCII payload must be UTF-8")
}

fn row_checksum(id: u64, unique_key: u64, bucket: u64, generation: u64, payload: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in [id, unique_key, bucket, generation] {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for byte in payload.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn unique_key(id: u64, generation: u64, stride: u64) -> u64 {
    generation * stride + id
}

fn bucket(id: u64, generation: u64, buckets: u64) -> u64 {
    (id + generation) % buckets
}

macro_rules! make_row {
    ($row:ident, $id:expr, $generation:expr, $config:expr, $stride:expr) => {{
        let id = $id;
        let generation = $generation;
        let unique_key = unique_key(id, generation, $stride);
        let bucket = bucket(id, generation, $config.buckets);
        let payload = make_payload(id, generation, $config.payload_bytes);
        let checksum = row_checksum(id, unique_key, bucket, generation, &payload);
        $row {
            id,
            unique_key,
            bucket,
            generation,
            checksum,
            payload,
        }
    }};
}

macro_rules! valid_row {
    ($row:expr, $config:expr, $stride:expr) => {{
        let row = $row;
        row.unique_key == unique_key(row.id, row.generation, $stride)
            && row.bucket == bucket(row.id, row.generation, $config.buckets)
            && row.payload.len() == $config.payload_bytes
            && row
                .payload
                .as_bytes()
                .iter()
                .all(|byte| *byte == payload_byte(row.id, row.generation))
            && row.checksum
                == row_checksum(
                    row.id,
                    row.unique_key,
                    row.bucket,
                    row.generation,
                    &row.payload,
                )
    }};
}

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

macro_rules! run_repetition {
    ($config:expr, $backend:expr, $repetition:expr, $table:ty, $row:ident) => {{
        let config = $config;
        let stride = config.records + 1;
        let table = Arc::new(<$table>::default());
        let shadow: Arc<Vec<AtomicU64>> =
            Arc::new((0..config.records).map(|_| AtomicU64::new(0)).collect());
        let counters = Arc::new(Counters::default());

        for id in 0..config.records {
            table
                .insert(make_row!($row, id, 0, config, stride))
                .expect("initial shadow row must insert");
        }

        // Create fragmentation outside the measured interval so the concurrent
        // vacuum has real relocation work from the beginning.
        if config.vacuum {
            for id in config.records..config.records + config.records / 2 {
                table
                    .insert(make_row!($row, id, 0, config, stride))
                    .expect("filler row must insert");
            }
            for id in config.records..config.records + config.records / 2 {
                table.delete(id).await.expect("filler row must delete");
            }
        }

        let reader_threads = config.threads / 2;
        let writer_threads = config.threads - reader_threads;
        let participant_count = writer_threads + reader_threads + usize::from(config.vacuum);
        let start = Arc::new(Barrier::new(participant_count + 1));
        let mut handles = Vec::with_capacity(writer_threads + reader_threads);

        for worker in 0..writer_threads {
            let table = Arc::clone(&table);
            let shadow = Arc::clone(&shadow);
            let counters = Arc::clone(&counters);
            let start = Arc::clone(&start);
            let config = config.clone();
            handles.push(tokio::spawn(async move {
                let owned: Vec<u64> = (worker as u64..config.records)
                    .step_by(writer_threads)
                    .collect();
                let operations = config.operations / writer_threads as u64
                    + u64::from((worker as u64) < config.operations % writer_threads as u64);
                start.wait().await;
                for sequence in 0..operations {
                    let id = owned[(sequence as usize) % owned.len()];
                    let previous = shadow[id as usize].load(Ordering::Acquire) & GENERATION_MASK;
                    shadow[id as usize].store(WRITING | previous, Ordering::Release);
                    let generation = previous + 1;
                    let next_row = make_row!($row, id, generation, &config, stride);
                    let success = if config.delete_every != 0
                        && sequence.is_multiple_of(config.delete_every)
                    {
                        if table.delete(id).await.is_err() {
                            false
                        } else {
                            table.insert(next_row).is_ok()
                        }
                    } else {
                        table.update(next_row).await.is_ok()
                    };
                    if success {
                        shadow[id as usize].store(generation, Ordering::Release);
                    } else {
                        counters.writer_errors.fetch_add(1, Ordering::Relaxed);
                        shadow[id as usize].store(previous, Ordering::Release);
                    }

                    let observed = black_box(table.select(id));
                    match observed {
                        None => {
                            counters
                                .writer_postcommit_primary_misses
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        Some(row) if !valid_row!(&row, &config, stride) => {
                            counters.torn_rows.fetch_add(1, Ordering::Relaxed);
                        }
                        Some(_) => {}
                    }
                }
            }));
        }

        for reader in 0..reader_threads {
            let table = Arc::clone(&table);
            let shadow = Arc::clone(&shadow);
            let counters = Arc::clone(&counters);
            let start = Arc::clone(&start);
            let config = config.clone();
            handles.push(tokio::spawn(async move {
                let operations = config.operations / reader_threads as u64
                    + u64::from((reader as u64) < config.operations % reader_threads as u64);
                let mut random =
                    config.seed ^ (reader as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                start.wait().await;
                for sequence in 0..operations {
                    let id = next_random(&mut random) % config.records;
                    let before = shadow[id as usize].load(Ordering::Acquire);
                    let observed = black_box(table.select(id));
                    let after = shadow[id as usize].load(Ordering::Acquire);
                    counters.online_reads.fetch_add(1, Ordering::Relaxed);
                    if let Some(row) = &observed
                        && !valid_row!(row, &config, stride)
                    {
                        counters.torn_rows.fetch_add(1, Ordering::Relaxed);
                    }
                    if before == after && before & WRITING == 0 {
                        let generation = before & GENERATION_MASK;
                        if observed
                            .as_ref()
                            .is_none_or(|row| row.id != id || row.generation != generation)
                        {
                            counters
                                .stable_primary_mismatches
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    let before = shadow[id as usize].load(Ordering::Acquire);
                    let generation = before & GENERATION_MASK;
                    let observed =
                        black_box(table.select_by_unique_key(unique_key(id, generation, stride)));
                    let after = shadow[id as usize].load(Ordering::Acquire);
                    counters.online_reads.fetch_add(1, Ordering::Relaxed);
                    if let Some(row) = &observed
                        && !valid_row!(row, &config, stride)
                    {
                        counters.torn_rows.fetch_add(1, Ordering::Relaxed);
                    }
                    if before == after
                        && before & WRITING == 0
                        && observed
                            .as_ref()
                            .is_none_or(|row| row.id != id || row.generation != generation)
                    {
                        counters
                            .stable_unique_mismatches
                            .fetch_add(1, Ordering::Relaxed);
                    }

                    if generation > 0 {
                        let before = shadow[id as usize].load(Ordering::Acquire);
                        let observed = black_box(table.select_by_unique_key(unique_key(
                            id,
                            generation - 1,
                            stride,
                        )));
                        let after = shadow[id as usize].load(Ordering::Acquire);
                        counters.online_reads.fetch_add(1, Ordering::Relaxed);
                        if before == after && before & WRITING == 0 && observed.is_some() {
                            counters
                                .stable_stale_unique_hits
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    if sequence.is_multiple_of(64) {
                        let selected_bucket = next_random(&mut random) % config.buckets;
                        match table.select_by_bucket(selected_bucket).execute() {
                            Ok(rows) => {
                                counters
                                    .online_reads
                                    .fetch_add(rows.len() as u64, Ordering::Relaxed);
                                for row in rows {
                                    if row.bucket != selected_bucket {
                                        counters
                                            .bucket_predicate_mismatches
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                    if !valid_row!(&row, &config, stride) {
                                        counters.torn_rows.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                            Err(_) => {
                                counters
                                    .bucket_predicate_mismatches
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }));
        }

        let vacuum_handle = if config.vacuum {
            let vacuum = table.vacuum();
            let start = Arc::clone(&start);
            Some(tokio::spawn(async move {
                start.wait().await;
                tokio::task::yield_now().await;
                vacuum.vacuum().await
            }))
        } else {
            None
        };

        let measured = Instant::now();
        start.wait().await;
        for handle in handles {
            handle.await.expect("shadow worker panicked");
        }
        let (vacuum_errors, vacuum_pages_processed, vacuum_pages_freed) =
            if let Some(handle) = vacuum_handle {
                match handle.await.expect("vacuum task panicked") {
                    Ok(stats) => (0, stats.pages_processed, stats.pages_freed),
                    Err(_) => (1, 0, 0),
                }
            } else {
                (0, 0, 0)
            };
        let elapsed_ns = measured.elapsed().as_nanos();

        if table.count() != config.records as usize {
            counters.final_mismatches.fetch_add(1, Ordering::Relaxed);
            counters
                .final_cardinality_mismatches
                .fetch_add(1, Ordering::Relaxed);
        }
        for id in 0..config.records {
            let generation = shadow[id as usize].load(Ordering::Acquire) & GENERATION_MASK;
            let expected_unique = unique_key(id, generation, stride);
            let primary = table.select(id);
            if primary.as_ref().is_none_or(|row| {
                row.id != id || row.generation != generation || !valid_row!(row, config, stride)
            }) {
                counters.final_mismatches.fetch_add(1, Ordering::Relaxed);
                counters
                    .final_primary_mismatches
                    .fetch_add(1, Ordering::Relaxed);
            }
            let unique = table.select_by_unique_key(expected_unique);
            if unique.as_ref().is_none_or(|row| {
                row.id != id || row.generation != generation || !valid_row!(row, config, stride)
            }) {
                counters.final_mismatches.fetch_add(1, Ordering::Relaxed);
                counters
                    .final_unique_mismatches
                    .fetch_add(1, Ordering::Relaxed);
            }
            if generation > 0
                && table
                    .select_by_unique_key(unique_key(id, generation - 1, stride))
                    .is_some()
            {
                counters.final_mismatches.fetch_add(1, Ordering::Relaxed);
                counters
                    .final_stale_unique_hits
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        for expected_bucket in 0..config.buckets {
            match table.select_by_bucket(expected_bucket).execute() {
                Ok(mut rows) => {
                    rows.sort_by_key(|row| row.id);
                    let expected: Vec<u64> = (0..config.records)
                        .filter(|id| {
                            let generation =
                                shadow[*id as usize].load(Ordering::Acquire) & GENERATION_MASK;
                            bucket(*id, generation, config.buckets) == expected_bucket
                        })
                        .collect();
                    let observed: Vec<u64> = rows.iter().map(|row| row.id).collect();
                    let missing = expected
                        .iter()
                        .filter(|id| observed.binary_search(id).is_err())
                        .count() as u64;
                    let unexpected = observed
                        .iter()
                        .filter(|id| expected.binary_search(id).is_err())
                        .count() as u64;
                    let duplicates = observed
                        .windows(2)
                        .filter(|pair| pair[0] == pair[1])
                        .count() as u64;
                    counters
                        .final_bucket_missing_rows
                        .fetch_add(missing, Ordering::Relaxed);
                    counters
                        .final_bucket_unexpected_rows
                        .fetch_add(unexpected, Ordering::Relaxed);
                    counters
                        .final_bucket_duplicate_rows
                        .fetch_add(duplicates, Ordering::Relaxed);
                    if observed != expected
                        || rows.iter().any(|row| !valid_row!(row, config, stride))
                    {
                        counters.final_mismatches.fetch_add(1, Ordering::Relaxed);
                        counters
                            .final_bucket_mismatches
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    counters.final_mismatches.fetch_add(1, Ordering::Relaxed);
                    counters
                        .final_bucket_mismatches
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let failures = counters.failures() + vacuum_errors;
        ResultRecord {
            schema_version: 1,
            suite: "shadow-concurrency",
            backend: $backend.as_str(),
            repetition: $repetition,
            records: config.records,
            writer_operations_requested: config.operations,
            reader_operations_requested: if reader_threads == 0 {
                0
            } else {
                config.operations
            },
            threads: config.threads,
            writer_threads,
            reader_threads,
            payload_bytes: config.payload_bytes,
            buckets: config.buckets,
            vacuum_enabled: config.vacuum,
            delete_every: config.delete_every,
            specialized_stable_index_reads: matches!($backend, Backend::WorkTablesIndex),
            elapsed_ns,
            online_reads: counters.online_reads.load(Ordering::Relaxed),
            writer_errors: counters.writer_errors.load(Ordering::Relaxed),
            writer_postcommit_primary_misses: counters
                .writer_postcommit_primary_misses
                .load(Ordering::Relaxed),
            stable_primary_mismatches: counters.stable_primary_mismatches.load(Ordering::Relaxed),
            stable_unique_mismatches: counters.stable_unique_mismatches.load(Ordering::Relaxed),
            stable_stale_unique_hits: counters.stable_stale_unique_hits.load(Ordering::Relaxed),
            bucket_predicate_mismatches: counters
                .bucket_predicate_mismatches
                .load(Ordering::Relaxed),
            torn_rows: counters.torn_rows.load(Ordering::Relaxed),
            final_mismatches: counters.final_mismatches.load(Ordering::Relaxed),
            final_cardinality_mismatches: counters
                .final_cardinality_mismatches
                .load(Ordering::Relaxed),
            final_primary_mismatches: counters.final_primary_mismatches.load(Ordering::Relaxed),
            final_unique_mismatches: counters.final_unique_mismatches.load(Ordering::Relaxed),
            final_stale_unique_hits: counters.final_stale_unique_hits.load(Ordering::Relaxed),
            final_bucket_mismatches: counters.final_bucket_mismatches.load(Ordering::Relaxed),
            final_bucket_missing_rows: counters.final_bucket_missing_rows.load(Ordering::Relaxed),
            final_bucket_unexpected_rows: counters
                .final_bucket_unexpected_rows
                .load(Ordering::Relaxed),
            final_bucket_duplicate_rows: counters
                .final_bucket_duplicate_rows
                .load(Ordering::Relaxed),
            vacuum_errors,
            vacuum_pages_processed,
            vacuum_pages_freed,
            passed: failures == 0,
            target_arch: std::env::consts::ARCH,
            target_os: std::env::consts::OS,
        }
    }};
}

#[tokio::main]
async fn main() {
    let backend = std::env::var("WT_INDEX_BACKEND")
        .unwrap_or_else(|_| "worktables_index".to_owned())
        .parse::<Backend>()
        .unwrap_or_else(|error| {
            eprintln!("error: {error}");
            std::process::exit(2);
        });
    let config = Config::from_args().unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    let mut failed = false;
    for repetition in 1..=config.repetitions {
        let result = match backend {
            Backend::WorkTablesIndex => run_repetition!(
                &config,
                backend,
                repetition,
                ShadowWtiWorkTable,
                ShadowWtiRow
            ),
            Backend::Indexset => run_repetition!(
                &config,
                backend,
                repetition,
                ShadowIndexsetWorkTable,
                ShadowIndexsetRow
            ),
            Backend::Congee => run_repetition!(
                &config,
                backend,
                repetition,
                ShadowCongeeWorkTable,
                ShadowCongeeRow
            ),
            Backend::Arctic => run_repetition!(
                &config,
                backend,
                repetition,
                ShadowArcticWorkTable,
                ShadowArcticRow
            ),
        };
        failed |= !result.passed;
        println!(
            "{}",
            serde_json::to_string(&result).expect("result must serialize")
        );
    }
    if failed {
        std::process::exit(1);
    }
}
