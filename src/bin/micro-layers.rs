use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::Serialize;
use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::rng::Rng;

trait MicroBackend: Default {
    fn insert(&self, row: Row);
    fn point_read(&self, key: u64) -> u64;
    async fn update(&self, key: u64, value: u64);
    fn range_checksum(&self, start: u64, end: u64) -> u64;
}

macro_rules! micro_backend {
    ($module:ident, $using:ident) => {
        mod $module {
            use super::*;

            worktable!(
                name: Micro,
                persist: false,
                columns: {
                    id: u64 primary_key using $using,
                    value: u64,
                    payload: String,
                },
                queries: {
                    update: {
                        Value(value) by id,
                    }
                }
            );

            pub(super) struct Driver(MicroWorkTable);

            impl Default for Driver {
                fn default() -> Self {
                    Self(MicroWorkTable::default())
                }
            }

            impl MicroBackend for Driver {
                fn insert(&self, row: Row) {
                    self.0
                        .insert(MicroRow {
                            id: row.id,
                            value: row.value,
                            payload: row.payload,
                        })
                        .expect("unique key");
                }

                fn point_read(&self, key: u64) -> u64 {
                    let row = black_box(self.0.select(key).expect("loaded key"));
                    row.value.wrapping_add(row.payload.len() as u64)
                }

                async fn update(&self, key: u64, value: u64) {
                    self.0
                        .update_value(ValueQuery { value }, key)
                        .await
                        .expect("loaded key");
                }

                fn range_checksum(&self, start: u64, end: u64) -> u64 {
                    self.0
                        .select_by_pk_range(start..=end)
                        .execute()
                        .expect("range query")
                        .into_iter()
                        .fold(0, |sum, row| sum.wrapping_add(row.value))
                }
            }
        }
    };
}

micro_backend!(wti_backend, worktables_index);
micro_backend!(congee_backend, congee);
micro_backend!(arctic_backend, arctic);

#[derive(Clone, Debug)]
struct Row {
    id: u64,
    value: u64,
    payload: String,
}

#[derive(Clone, Debug)]
struct Config {
    rows: u64,
    operations: u64,
    scan_operations: u64,
    scan_length: u64,
    repetitions: usize,
    payload_bytes: usize,
    seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rows: 1_000_000,
            operations: 1_000_000,
            scan_operations: 10_000,
            scan_length: 100,
            repetitions: 5,
            payload_bytes: 64,
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
                    "micro-layers options:\n\
                     --rows N              loaded rows (default 1000000)\n\
                     --operations N        point reads/updates (default 1000000)\n\
                     --scan-operations N   range queries (default 10000)\n\
                     --scan-length N       rows per range (default 100)\n\
                     --repetitions N       fresh repetitions (default 5)\n\
                     --payload-bytes N     string bytes per row (default 64)\n\
                     --seed N              deterministic seed (default 42)"
                );
                std::process::exit(0);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--rows" => config.rows = parse(&flag, &value)?,
                "--operations" => config.operations = parse(&flag, &value)?,
                "--scan-operations" => config.scan_operations = parse(&flag, &value)?,
                "--scan-length" => config.scan_length = parse(&flag, &value)?,
                "--repetitions" => config.repetitions = parse(&flag, &value)?,
                "--payload-bytes" => config.payload_bytes = parse(&flag, &value)?,
                "--seed" => config.seed = parse(&flag, &value)?,
                _ => return Err(format!("unknown option: {flag}")),
            }
        }
        if config.rows == 0
            || config.operations == 0
            || config.scan_operations == 0
            || config.scan_length == 0
            || config.repetitions == 0
            || config.payload_bytes == 0
        {
            return Err("counts, repetitions, and payload size must be non-zero".into());
        }
        if config.scan_length > config.rows {
            return Err("--scan-length cannot exceed --rows".into());
        }
        Ok(config)
    }
}

fn parse<T>(flag: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
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
    engine: &'a str,
    layer: &'a str,
    operation: &'a str,
    repetition: usize,
    rows: u64,
    operations: u64,
    payload_bytes: usize,
    scan_length: u64,
    elapsed_ns: u128,
    ops_per_second: f64,
    checksum: u64,
    feature_versioned_row_publication: bool,
    target_arch: &'static str,
    target_os: &'static str,
}

#[allow(clippy::too_many_arguments)]
fn emit(
    config: &Config,
    engine: &str,
    layer: &str,
    operation: &str,
    repetition: usize,
    operations: u64,
    started: Instant,
    checksum: u64,
) {
    let elapsed_ns = started.elapsed().as_nanos();
    let result = ResultRow {
        schema_version: 1,
        suite: "micro-layers",
        engine,
        layer,
        operation,
        repetition,
        rows: config.rows,
        operations,
        payload_bytes: config.payload_bytes,
        scan_length: config.scan_length,
        elapsed_ns,
        ops_per_second: operations as f64 / (elapsed_ns as f64 / 1_000_000_000.0),
        checksum,
        feature_versioned_row_publication: cfg!(feature = "versioned-row-publication"),
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
    let point_keys = keys(config.operations, config.rows, config.seed);
    let scan_starts = keys(
        config.scan_operations,
        config.rows - config.scan_length + 1,
        config.seed ^ 0xa5a5_a5a5,
    );

    for repetition in 1..=config.repetitions {
        bench_vec(&config, repetition, &point_keys, &scan_starts);
        bench_vec_row_lock(&config, repetition, &point_keys);
        bench_hash_map(&config, repetition, &point_keys);
        bench_btree_map(&config, repetition, &point_keys, &scan_starts);
        bench_rwlock_hash_map(&config, repetition, &point_keys);
        bench_dash_map(&config, repetition, &point_keys);
        bench_worktable::<wti_backend::Driver>(
            &config,
            "worktable",
            repetition,
            &point_keys,
            &scan_starts,
        )
        .await;
        bench_worktable::<congee_backend::Driver>(
            &config,
            "worktable-congee",
            repetition,
            &point_keys,
            &scan_starts,
        )
        .await;
        bench_worktable::<arctic_backend::Driver>(
            &config,
            "worktable-arctic",
            repetition,
            &point_keys,
            &scan_starts,
        )
        .await;
    }
}

fn bench_vec(config: &Config, repetition: usize, point_keys: &[u64], scan_starts: &[u64]) {
    let rows = rows(config);
    let started = Instant::now();
    let table: Vec<_> = rows.into_iter().collect();
    emit(
        config,
        "vec",
        "L0",
        "insert",
        repetition,
        config.rows,
        started,
        table.len() as u64,
    );

    let started = Instant::now();
    let checksum = point_keys.iter().fold(0_u64, |sum, key| {
        let row = black_box(&table[*key as usize]);
        sum.wrapping_add(row.value)
            .wrapping_add(row.payload.len() as u64)
    });
    emit(
        config,
        "vec",
        "L0",
        "point_read_borrowed",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let mut table = table;
    let started = Instant::now();
    for key in point_keys {
        table[*key as usize].value = key.wrapping_mul(17);
    }
    black_box(&table);
    let checksum = table.iter().take(16).fold(0, |sum, row| sum ^ row.value);
    emit(
        config,
        "vec",
        "L0",
        "update_field",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    let checksum = scan_starts.iter().fold(0_u64, |sum, start| {
        table[*start as usize..(*start + config.scan_length) as usize]
            .iter()
            .fold(sum, |inner, row| inner.wrapping_add(row.value))
    });
    emit(
        config,
        "vec",
        "L0",
        "range_scan",
        repetition,
        config.scan_operations,
        started,
        checksum,
    );
}

fn bench_vec_row_lock(config: &Config, repetition: usize, point_keys: &[u64]) {
    let rows = rows(config);
    let started = Instant::now();
    let table: Vec<RwLock<Row>> = rows.into_iter().map(RwLock::new).collect();
    emit(
        config,
        "vec_row_lock",
        "L1",
        "insert",
        repetition,
        config.rows,
        started,
        table.len() as u64,
    );

    let started = Instant::now();
    let checksum = point_keys.iter().fold(0_u64, |sum, key| {
        let row = table[*key as usize].read();
        sum.wrapping_add(row.value)
            .wrapping_add(row.payload.len() as u64)
    });
    emit(
        config,
        "vec_row_lock",
        "L1",
        "point_read_borrowed",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    for key in point_keys {
        table[*key as usize].write().value = key.wrapping_mul(17);
    }
    black_box(&table);
    emit(
        config,
        "vec_row_lock",
        "L1",
        "update_field",
        repetition,
        config.operations,
        started,
        table[0].read().value,
    );
}

fn bench_hash_map(config: &Config, repetition: usize, point_keys: &[u64]) {
    let started = Instant::now();
    let mut table: HashMap<_, _> = rows(config).map(|row| (row.id, row)).collect();
    emit(
        config,
        "hash_map",
        "L0",
        "insert",
        repetition,
        config.rows,
        started,
        table.len() as u64,
    );

    let started = Instant::now();
    let checksum = point_keys.iter().fold(0_u64, |sum, key| {
        let row = black_box(table.get(key).expect("loaded key"));
        sum.wrapping_add(row.value)
            .wrapping_add(row.payload.len() as u64)
    });
    emit(
        config,
        "hash_map",
        "L0",
        "point_read_borrowed",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    for key in point_keys {
        table.get_mut(key).expect("loaded key").value = key.wrapping_mul(17);
    }
    black_box(&table);
    emit(
        config,
        "hash_map",
        "L0",
        "update_field",
        repetition,
        config.operations,
        started,
        table.get(&0).expect("loaded key").value,
    );
}

fn bench_btree_map(config: &Config, repetition: usize, point_keys: &[u64], scan_starts: &[u64]) {
    let started = Instant::now();
    let mut table: BTreeMap<_, _> = rows(config).map(|row| (row.id, row)).collect();
    emit(
        config,
        "btree_map",
        "L0",
        "insert",
        repetition,
        config.rows,
        started,
        table.len() as u64,
    );

    let started = Instant::now();
    let checksum = point_keys.iter().fold(0_u64, |sum, key| {
        let row = black_box(table.get(key).expect("loaded key"));
        sum.wrapping_add(row.value)
            .wrapping_add(row.payload.len() as u64)
    });
    emit(
        config,
        "btree_map",
        "L0",
        "point_read_borrowed",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    for key in point_keys {
        table.get_mut(key).expect("loaded key").value = key.wrapping_mul(17);
    }
    black_box(&table);
    emit(
        config,
        "btree_map",
        "L0",
        "update_field",
        repetition,
        config.operations,
        started,
        table.get(&0).expect("loaded key").value,
    );

    let started = Instant::now();
    let checksum = scan_starts.iter().fold(0_u64, |sum, start| {
        table
            .range(*start..)
            .take(config.scan_length as usize)
            .fold(sum, |inner, (_, row)| inner.wrapping_add(row.value))
    });
    emit(
        config,
        "btree_map",
        "L0",
        "range_scan",
        repetition,
        config.scan_operations,
        started,
        checksum,
    );
}

fn bench_rwlock_hash_map(config: &Config, repetition: usize, point_keys: &[u64]) {
    let started = Instant::now();
    let table = Arc::new(RwLock::new(HashMap::new()));
    for row in rows(config) {
        table.write().insert(row.id, row);
    }
    emit(
        config,
        "rwlock_hash_map",
        "L1",
        "insert",
        repetition,
        config.rows,
        started,
        table.read().len() as u64,
    );

    let started = Instant::now();
    let checksum = point_keys.iter().fold(0_u64, |sum, key| {
        let table = table.read();
        let row = black_box(table.get(key).expect("loaded key"));
        sum.wrapping_add(row.value)
            .wrapping_add(row.payload.len() as u64)
    });
    emit(
        config,
        "rwlock_hash_map",
        "L1",
        "point_read_borrowed",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    for key in point_keys {
        table.write().get_mut(key).expect("loaded key").value = key.wrapping_mul(17);
    }
    black_box(&table);
    emit(
        config,
        "rwlock_hash_map",
        "L1",
        "update_field",
        repetition,
        config.operations,
        started,
        table.read().get(&0).expect("loaded key").value,
    );
}

fn bench_dash_map(config: &Config, repetition: usize, point_keys: &[u64]) {
    let started = Instant::now();
    let table = DashMap::new();
    for row in rows(config) {
        table.insert(row.id, row);
    }
    emit(
        config,
        "dash_map",
        "L1",
        "insert",
        repetition,
        config.rows,
        started,
        table.len() as u64,
    );

    let started = Instant::now();
    let checksum = point_keys.iter().fold(0_u64, |sum, key| {
        let row = table.get(key).expect("loaded key");
        sum.wrapping_add(row.value)
            .wrapping_add(row.payload.len() as u64)
    });
    emit(
        config,
        "dash_map",
        "L1",
        "point_read_borrowed",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    for key in point_keys {
        table.get_mut(key).expect("loaded key").value = key.wrapping_mul(17);
    }
    black_box(&table);
    emit(
        config,
        "dash_map",
        "L1",
        "update_field",
        repetition,
        config.operations,
        started,
        table.get(&0).expect("loaded key").value,
    );
}

async fn bench_worktable<T: MicroBackend>(
    config: &Config,
    engine: &str,
    repetition: usize,
    point_keys: &[u64],
    scan_starts: &[u64],
) {
    let table = T::default();
    let started = Instant::now();
    for row in rows(config) {
        table.insert(row);
    }
    emit(
        config,
        engine,
        "L2",
        "insert",
        repetition,
        config.rows,
        started,
        config.rows,
    );

    let started = Instant::now();
    let checksum = point_keys
        .iter()
        .fold(0_u64, |sum, key| sum.wrapping_add(table.point_read(*key)));
    emit(
        config,
        engine,
        "L2",
        "point_read_materialized",
        repetition,
        config.operations,
        started,
        checksum,
    );

    let started = Instant::now();
    for key in point_keys {
        table.update(*key, key.wrapping_mul(17)).await;
    }
    emit(
        config,
        engine,
        "L2",
        "update_field",
        repetition,
        config.operations,
        started,
        table.point_read(0),
    );

    let started = Instant::now();
    let mut checksum = 0_u64;
    for start in scan_starts {
        let end = start + config.scan_length - 1;
        checksum = checksum.wrapping_add(table.range_checksum(*start, end));
    }
    emit(
        config,
        engine,
        "L2",
        "range_scan",
        repetition,
        config.scan_operations,
        started,
        checksum,
    );
}

fn rows(config: &Config) -> impl Iterator<Item = Row> + '_ {
    (0..config.rows).map(|id| Row {
        id,
        value: id,
        payload: payload(id ^ config.seed, config.payload_bytes),
    })
}

fn keys(count: u64, upper: u64, seed: u64) -> Vec<u64> {
    let mut rng = Rng::new(seed);
    (0..count).map(|_| rng.below(upper)).collect()
}

fn payload(seed: u64, length: usize) -> String {
    let mut rng = Rng::new(seed);
    let bytes: Vec<_> = (0..length)
        .map(|_| b'!' + rng.below((b'~' - b'!') as u64 + 1) as u8)
        .collect();
    String::from_utf8(bytes).expect("ASCII payload")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_schema_records_scan_length() {
        let row = ResultRow {
            schema_version: 1,
            suite: "micro-layers",
            engine: "test",
            layer: "L0",
            operation: "range_scan",
            repetition: 1,
            rows: 100,
            operations: 10,
            payload_bytes: 32,
            scan_length: 17,
            elapsed_ns: 1,
            ops_per_second: 1.0,
            checksum: 0,
            feature_versioned_row_publication: false,
            target_arch: "test",
            target_os: "test",
        };
        let encoded = serde_json::to_value(row).expect("result serializes");
        assert_eq!(encoded["scan_length"], 17);
    }
}
