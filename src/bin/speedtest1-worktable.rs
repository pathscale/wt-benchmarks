use std::hint::black_box;
use std::str::FromStr;
use std::time::Instant;

use serde::Serialize;
use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::kv::{text_checksum, text_value};
use wt_benchmarks::rng::Rng;

worktable!(
    name: SpeedInt,
    columns: {
        id: u64 primary_key,
        group_id: u64,
        counter: u64,
        payload: String,
    },
    indexes: {
        group_idx: group_id,
    },
    queries: {
        update: {
            Counter(counter) by id,
            Payload(payload) by id,
        }
    }
);

worktable!(
    name: SpeedText,
    columns: {
        key: String primary_key,
        value: u64,
        payload: String,
    }
);

#[derive(Clone, Debug)]
struct Config {
    rows: u64,
    operations: u64,
    repetitions: usize,
    scan_length: u64,
    payload_bytes: usize,
    groups: u64,
    seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rows: 100_000,
            operations: 100_000,
            repetitions: 5,
            scan_length: 100,
            payload_bytes: 64,
            groups: 1_000,
            seed: 42,
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
                    "speedtest1-worktable options:\n\
                     --rows N             rows per table (default 100000)\n\
                     --operations N       point/range/update/delete attempts (default 100000)\n\
                     --repetitions N      fresh repetitions (default 5)\n\
                     --scan-length N      rows requested per PK range (default 100)\n\
                     --payload-bytes N    string bytes per row (default 64)\n\
                     --groups N           secondary-index cardinality (default 1000)\n\
                     --seed N             deterministic seed (default 42)"
                );
                std::process::exit(0);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--rows" => config.rows = parse(&flag, &value)?,
                "--operations" => config.operations = parse(&flag, &value)?,
                "--repetitions" => config.repetitions = parse(&flag, &value)?,
                "--scan-length" => config.scan_length = parse(&flag, &value)?,
                "--payload-bytes" => config.payload_bytes = parse(&flag, &value)?,
                "--groups" => config.groups = parse(&flag, &value)?,
                "--seed" => config.seed = parse(&flag, &value)?,
                _ => return Err(format!("unknown option: {flag}")),
            }
        }
        if config.rows == 0
            || config.operations == 0
            || config.repetitions == 0
            || config.scan_length == 0
            || config.payload_bytes == 0
            || config.groups == 0
        {
            return Err("counts, repetitions, sizes, and groups must be non-zero".into());
        }
        if config.scan_length > config.rows {
            return Err("--scan-length cannot exceed --rows".into());
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

#[derive(Serialize)]
struct ResultRow<'a> {
    schema_version: u32,
    suite: &'static str,
    engine: &'static str,
    phase: &'a str,
    repetition: usize,
    rows: u64,
    operations: u64,
    payload_bytes: usize,
    scan_length: u64,
    groups: u64,
    elapsed_ns: u128,
    ops_per_second: f64,
    checksum: u64,
    transaction_semantics: &'static str,
    read_ownership: &'static str,
    target_arch: &'static str,
    target_os: &'static str,
}

fn emit(
    config: &Config,
    phase: &str,
    repetition: usize,
    operations: u64,
    started: Instant,
    checksum: u64,
) {
    let elapsed_ns = started.elapsed().as_nanos();
    let result = ResultRow {
        schema_version: 1,
        suite: "sqlite-speedtest1-core-shape",
        engine: "worktable",
        phase,
        repetition,
        rows: config.rows,
        operations,
        payload_bytes: config.payload_bytes,
        scan_length: config.scan_length,
        groups: config.groups,
        elapsed_ns,
        ops_per_second: operations as f64 / (elapsed_ns as f64 / 1_000_000_000.0),
        checksum,
        transaction_semantics: "per-operation; no bulk transaction",
        read_ownership: "materialized-owned-row",
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    };
    println!(
        "{}",
        serde_json::to_string(&result).expect("result must serialize")
    );
}

#[tokio::main]
async fn main() {
    let config = Config::from_args().unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    let keys = random_keys(config.operations, config.rows, config.seed);
    let scan_starts = random_keys(
        config.operations,
        config.rows - config.scan_length + 1,
        config.seed ^ 0x55aa,
    );

    for repetition in 1..=config.repetitions {
        run_repetition(&config, repetition, &keys, &scan_starts).await;
    }
}

async fn run_repetition(config: &Config, repetition: usize, keys: &[u64], scan_starts: &[u64]) {
    let integers = SpeedIntWorkTable::default();
    let started = Instant::now();
    for id in 0..config.rows {
        integers
            .insert(SpeedIntRow {
                id,
                group_id: id % config.groups,
                counter: id,
                payload: text_value(id, config.payload_bytes),
            })
            .expect("sequential integer key must insert");
    }
    emit(
        config,
        "integer_insert_sequential",
        repetition,
        config.rows,
        started,
        integers.count() as u64,
    );

    let started = Instant::now();
    let checksum = keys.iter().fold(0_u64, |sum, key| {
        let row = black_box(integers.select(*key)).expect("loaded key");
        sum.wrapping_add(row.counter)
            .wrapping_add(text_checksum(row.id, &row.payload))
    });
    emit(
        config,
        "integer_point_read_random",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    let mut checksum = 0_u64;
    for start in scan_starts {
        let rows = integers
            .select_by_pk_range(*start..*start + config.scan_length)
            .execute()
            .expect("primary range");
        checksum = rows
            .into_iter()
            .fold(checksum, |sum, row| sum.wrapping_add(row.counter));
    }
    emit(
        config,
        "integer_range_read",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    let mut checksum = 0_u64;
    for group in keys.iter().map(|key| key % config.groups) {
        let rows = integers
            .select_by_group_id(group)
            .execute()
            .expect("secondary lookup");
        checksum = checksum.wrapping_add(rows.len() as u64);
    }
    emit(
        config,
        "integer_secondary_fanout",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    for key in keys {
        integers
            .update_counter(
                CounterQuery {
                    counter: key.wrapping_mul(17),
                },
                *key,
            )
            .await
            .expect("loaded key must update");
    }
    emit(
        config,
        "integer_update_random",
        repetition,
        config.operations,
        started,
        integers.count() as u64,
    );

    let started = Instant::now();
    let rows = integers
        .select_by_pk_range(0..config.rows)
        .execute()
        .expect("ordered full scan");
    let checksum = rows
        .into_iter()
        .fold(0_u64, |sum, row| sum.wrapping_add(row.counter));
    emit(
        config,
        "integer_ordered_full_scan",
        repetition,
        config.rows,
        started,
        checksum,
    );

    let text = SpeedTextWorkTable::default();
    let started = Instant::now();
    for id in 0..config.rows {
        text.insert(SpeedTextRow {
            key: text_key(id),
            value: id,
            payload: text_value(id, config.payload_bytes),
        })
        .expect("sequential text key must insert");
    }
    emit(
        config,
        "text_insert_sequential",
        repetition,
        config.rows,
        started,
        text.count() as u64,
    );

    let started = Instant::now();
    let checksum = keys.iter().fold(0_u64, |sum, key| {
        let row = black_box(text.select(text_key(*key))).expect("loaded text key");
        sum.wrapping_add(row.value)
            .wrapping_add(row.payload.len() as u64)
    });
    emit(
        config,
        "text_point_read_random",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    let mut deleted = 0_u64;
    for key in keys {
        if integers.delete(*key).await.is_ok() {
            deleted += 1;
        }
    }
    emit(
        config,
        "integer_delete_random",
        repetition,
        config.operations,
        started,
        deleted,
    );
}

fn random_keys(count: u64, upper: u64, seed: u64) -> Vec<u64> {
    let mut rng = Rng::new(seed);
    (0..count).map(|_| rng.below(upper)).collect()
}

fn text_key(value: u64) -> String {
    format!("key-{value:020}")
}
