use std::hint::black_box;
use std::str::FromStr;
use std::time::Instant;

use serde::Serialize;
use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::kv::{text_checksum, text_value};
use wt_benchmarks::kv_table::IndexBackend;
use wt_benchmarks::rng::Rng;

trait SpeedBackend: Default {
    fn insert_integer(&self, id: u64, group_id: u64, payload: String);
    fn integer_count(&self) -> u64;
    fn integer_point_checksum(&self, key: u64) -> u64;
    fn integer_range_checksum(&self, start: u64, length: u64) -> u64;
    fn integer_group_count(&self, group: u64) -> u64;
    async fn update_integer(&self, key: u64, counter: u64);
    fn integer_full_scan_checksum(&self, rows: u64) -> u64;
    async fn delete_integer(&self, key: u64) -> bool;
}

macro_rules! speed_backend {
    ($module:ident, $using:ident) => {
        mod $module {
            use super::*;

            worktable!(
                name: SpeedInt,
                persist: false,
                columns: {
                    id: u64 primary_key using $using,
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

            pub(super) struct Driver(SpeedIntWorkTable);

            impl Default for Driver {
                fn default() -> Self {
                    Self(SpeedIntWorkTable::default())
                }
            }

            impl SpeedBackend for Driver {
                fn insert_integer(&self, id: u64, group_id: u64, payload: String) {
                    self.0
                        .insert(SpeedIntRow { id, group_id, counter: id, payload })
                        .expect("sequential integer key must insert");
                }

                fn integer_count(&self) -> u64 { self.0.count() as u64 }

                fn integer_point_checksum(&self, key: u64) -> u64 {
                    let row = black_box(self.0.select(key)).expect("loaded key");
                    row.counter.wrapping_add(text_checksum(row.id, &row.payload))
                }

                fn integer_range_checksum(&self, start: u64, length: u64) -> u64 {
                    self.0
                        .select_by_pk_range(start..start + length)
                        .execute()
                        .expect("primary range")
                        .into_iter()
                        .fold(0, |sum, row| sum.wrapping_add(row.counter))
                }

                fn integer_group_count(&self, group: u64) -> u64 {
                    self.0
                        .select_by_group_id(group)
                        .execute()
                        .expect("secondary lookup")
                        .len() as u64
                }

                async fn update_integer(&self, key: u64, counter: u64) {
                    self.0
                        .update_counter(CounterQuery { counter }, key)
                        .await
                        .expect("loaded key must update");
                }

                fn integer_full_scan_checksum(&self, rows: u64) -> u64 {
                    self.0
                        .select_by_pk_range(0..rows)
                        .execute()
                        .expect("ordered full scan")
                        .into_iter()
                        .fold(0, |sum, row| sum.wrapping_add(row.counter))
                }

                async fn delete_integer(&self, key: u64) -> bool {
                    self.0.delete(key).await.is_ok()
                }
            }
        }
    };
}

speed_backend!(wti_backend, worktables_index);
speed_backend!(congee_backend, congee);
speed_backend!(arctic_backend, arctic);

mod wti_text {
    use super::*;

    worktable!(
        name: SpeedText,
        persist: false,
        columns: {
            key: String primary_key using worktables_index,
            value: u64,
            payload: String,
        }
    );

    pub(super) fn run(config: &Config, repetition: usize, keys: &[u64]) {
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
            "worktable",
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
            "worktable",
            "text_point_read_random",
            repetition,
            config.operations,
            started,
            checksum,
        );
    }
}

#[derive(Clone, Debug)]
struct Config {
    rows: u64,
    operations: u64,
    repetitions: usize,
    scan_length: u64,
    payload_bytes: usize,
    groups: u64,
    seed: u64,
    index_backend: String,
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
            index_backend: "worktables_index".to_owned(),
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
                     --seed N             deterministic seed (default 42)\n\
                     --index-backend B    worktables_index, congee, or arctic"
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
                "--index-backend" => config.index_backend = value,
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
    engine: &'static str,
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
        engine,
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
    let backend = IndexBackend::parse(&config.index_backend).unwrap_or_else(|| {
        eprintln!("error: unknown index backend: {}", config.index_backend);
        std::process::exit(2);
    });
    let keys = random_keys(config.operations, config.rows, config.seed);
    let scan_starts = random_keys(
        config.operations,
        config.rows - config.scan_length + 1,
        config.seed ^ 0x55aa,
    );

    for repetition in 1..=config.repetitions {
        match backend {
            IndexBackend::WorktablesIndex => {
                run_repetition::<wti_backend::Driver>(
                    &config,
                    backend.benchmark_label(),
                    repetition,
                    &keys,
                    &scan_starts,
                )
                .await;
                wti_text::run(&config, repetition, &keys);
            }
            IndexBackend::Congee => {
                run_repetition::<congee_backend::Driver>(
                    &config,
                    backend.benchmark_label(),
                    repetition,
                    &keys,
                    &scan_starts,
                )
                .await
            }
            IndexBackend::Arctic => {
                run_repetition::<arctic_backend::Driver>(
                    &config,
                    backend.benchmark_label(),
                    repetition,
                    &keys,
                    &scan_starts,
                )
                .await
            }
        }
    }
}

async fn run_repetition<T: SpeedBackend>(
    config: &Config,
    engine: &'static str,
    repetition: usize,
    keys: &[u64],
    scan_starts: &[u64],
) {
    let tables = T::default();
    let started = Instant::now();
    for id in 0..config.rows {
        tables.insert_integer(id, id % config.groups, text_value(id, config.payload_bytes));
    }
    emit(
        config,
        engine,
        "integer_insert_sequential",
        repetition,
        config.rows,
        started,
        tables.integer_count(),
    );

    let started = Instant::now();
    let checksum = keys.iter().fold(0_u64, |sum, key| {
        sum.wrapping_add(tables.integer_point_checksum(*key))
    });
    emit(
        config,
        engine,
        "integer_point_read_random",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    let mut checksum = 0_u64;
    for start in scan_starts {
        checksum = checksum.wrapping_add(tables.integer_range_checksum(*start, config.scan_length));
    }
    emit(
        config,
        engine,
        "integer_range_read",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    let mut checksum = 0_u64;
    for group in keys.iter().map(|key| key % config.groups) {
        checksum = checksum.wrapping_add(tables.integer_group_count(group));
    }
    emit(
        config,
        engine,
        "integer_secondary_fanout",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    for key in keys {
        tables.update_integer(*key, key.wrapping_mul(17)).await;
    }
    emit(
        config,
        engine,
        "integer_update_random",
        repetition,
        config.operations,
        started,
        tables.integer_count(),
    );

    let started = Instant::now();
    let checksum = tables.integer_full_scan_checksum(config.rows);
    emit(
        config,
        engine,
        "integer_ordered_full_scan",
        repetition,
        config.rows,
        started,
        checksum,
    );

    let started = Instant::now();
    let mut deleted = 0_u64;
    for key in keys {
        if tables.delete_integer(*key).await {
            deleted += 1;
        }
    }
    emit(
        config,
        engine,
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
