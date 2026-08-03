use std::hint::black_box;
use std::str::FromStr;
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};
use serde::Serialize;
use wt_benchmarks::kv::{text_checksum, text_value};
use wt_benchmarks::rng::Rng;

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
                    "speedtest1-sqlite options:\n\
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
        if config.rows > i64::MAX as u64 || config.groups > i64::MAX as u64 {
            return Err("--rows and --groups must fit SQLite INTEGER".into());
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
struct ResultRow {
    schema_version: u32,
    suite: &'static str,
    engine: &'static str,
    phase: &'static str,
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
    sqlite_version: &'static str,
    target_arch: &'static str,
    target_os: &'static str,
}

fn result(
    config: &Config,
    phase: &'static str,
    repetition: usize,
    operations: u64,
    elapsed: Duration,
    checksum: u64,
) -> ResultRow {
    let elapsed_ns = elapsed.as_nanos();
    ResultRow {
        schema_version: 1,
        suite: "sqlite-speedtest1-core-shape",
        engine: "sqlite-memory",
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
        transaction_semantics: "SQLite :memory:; one autocommit statement per operation",
        read_ownership: "SQLite values decoded into owned Rust rows",
        sqlite_version: rusqlite::version(),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    }
}

fn main() {
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
        for row in run_repetition(&config, repetition, &keys, &scan_starts) {
            println!(
                "{}",
                serde_json::to_string(&row).expect("result must serialize")
            );
        }
    }
}

fn run_repetition(
    config: &Config,
    repetition: usize,
    keys: &[u64],
    scan_starts: &[u64],
) -> Vec<ResultRow> {
    let connection = Connection::open_in_memory().expect("open SQLite :memory:");
    connection
        .execute_batch(
            "PRAGMA journal_mode=MEMORY;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=MEMORY;
             CREATE TABLE speed_int (
                 id INTEGER PRIMARY KEY,
                 group_id INTEGER NOT NULL,
                 counter INTEGER NOT NULL,
                 payload TEXT NOT NULL
             );
             CREATE INDEX speed_int_group_idx ON speed_int(group_id);
             CREATE TABLE speed_text (
                 key TEXT PRIMARY KEY,
                 value INTEGER NOT NULL,
                 payload TEXT NOT NULL
             );",
        )
        .expect("create speedtest1 schema");
    let mut results = Vec::with_capacity(9);

    {
        let mut insert = connection
            .prepare_cached(
                "INSERT INTO speed_int(id, group_id, counter, payload) VALUES (?1, ?2, ?3, ?4)",
            )
            .expect("prepare integer insert");
        let started = Instant::now();
        for id in 0..config.rows {
            insert
                .execute(params![
                    id as i64,
                    (id % config.groups) as i64,
                    id as i64,
                    text_value(id, config.payload_bytes)
                ])
                .expect("sequential integer key must insert");
        }
        let count = connection
            .query_row("SELECT COUNT(*) FROM speed_int", [], |row| {
                sqlite_u64(row, 0)
            })
            .expect("count integer rows");
        results.push(result(
            config,
            "integer_insert_sequential",
            repetition,
            config.rows,
            started.elapsed(),
            count,
        ));
    }

    {
        let mut select = connection
            .prepare_cached("SELECT id, group_id, counter, payload FROM speed_int WHERE id = ?1")
            .expect("prepare integer point read");
        let started = Instant::now();
        let checksum = keys.iter().fold(0_u64, |sum, key| {
            let (id, group_id, counter, payload) = select
                .query_row(params![*key as i64], |row| {
                    Ok((
                        sqlite_u64(row, 0)?,
                        sqlite_u64(row, 1)?,
                        sqlite_u64(row, 2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .expect("loaded integer key");
            black_box(group_id);
            sum.wrapping_add(counter)
                .wrapping_add(text_checksum(id, &payload))
        });
        results.push(result(
            config,
            "integer_point_read_random",
            repetition,
            config.operations,
            started.elapsed(),
            checksum,
        ));
    }

    {
        let mut select = connection
            .prepare_cached(
                "SELECT id, group_id, counter, payload FROM speed_int
                 WHERE id >= ?1 AND id < ?2 ORDER BY id",
            )
            .expect("prepare integer range read");
        let started = Instant::now();
        let mut checksum = 0_u64;
        for start in scan_starts {
            let mapped = select
                .query_map(
                    params![*start as i64, (*start + config.scan_length) as i64],
                    |row| {
                        Ok((
                            sqlite_u64(row, 0)?,
                            sqlite_u64(row, 1)?,
                            sqlite_u64(row, 2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .expect("execute integer range");
            let rows = mapped
                .collect::<rusqlite::Result<Vec<_>>>()
                .expect("decode integer range");
            checksum = rows.into_iter().fold(checksum, |sum, row| {
                black_box((&row.0, &row.1, &row.3));
                sum.wrapping_add(row.2)
            });
        }
        results.push(result(
            config,
            "integer_range_read",
            repetition,
            config.operations,
            started.elapsed(),
            checksum,
        ));
    }

    {
        let mut select = connection
            .prepare_cached(
                "SELECT id, group_id, counter, payload FROM speed_int WHERE group_id = ?1",
            )
            .expect("prepare secondary fanout");
        let started = Instant::now();
        let mut checksum = 0_u64;
        for group in keys.iter().map(|key| key % config.groups) {
            let mapped = select
                .query_map(params![group as i64], |row| {
                    Ok((
                        sqlite_u64(row, 0)?,
                        sqlite_u64(row, 1)?,
                        sqlite_u64(row, 2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .expect("execute secondary fanout");
            let rows = mapped
                .collect::<rusqlite::Result<Vec<_>>>()
                .expect("decode secondary rows");
            black_box(&rows);
            checksum = checksum.wrapping_add(rows.len() as u64);
        }
        results.push(result(
            config,
            "integer_secondary_fanout",
            repetition,
            config.operations,
            started.elapsed(),
            checksum,
        ));
    }

    {
        let mut update = connection
            .prepare_cached("UPDATE speed_int SET counter = ?1 WHERE id = ?2")
            .expect("prepare integer update");
        let started = Instant::now();
        for key in keys {
            update
                .execute(params![key.wrapping_mul(17) as i64, *key as i64])
                .expect("loaded integer key must update");
        }
        let count = connection
            .query_row("SELECT COUNT(*) FROM speed_int", [], |row| {
                sqlite_u64(row, 0)
            })
            .expect("count integer rows");
        results.push(result(
            config,
            "integer_update_random",
            repetition,
            config.operations,
            started.elapsed(),
            count,
        ));
    }

    {
        let mut select = connection
            .prepare_cached("SELECT id, group_id, counter, payload FROM speed_int ORDER BY id")
            .expect("prepare ordered full scan");
        let started = Instant::now();
        let mapped = select
            .query_map([], |row| {
                Ok((
                    sqlite_u64(row, 0)?,
                    sqlite_u64(row, 1)?,
                    sqlite_u64(row, 2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .expect("execute ordered full scan");
        let rows = mapped
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("decode ordered full scan");
        let checksum = rows.into_iter().fold(0_u64, |sum, row| {
            black_box((&row.0, &row.1, &row.3));
            sum.wrapping_add(row.2)
        });
        results.push(result(
            config,
            "integer_ordered_full_scan",
            repetition,
            config.rows,
            started.elapsed(),
            checksum,
        ));
    }

    {
        let mut insert = connection
            .prepare_cached("INSERT INTO speed_text(key, value, payload) VALUES (?1, ?2, ?3)")
            .expect("prepare text insert");
        let started = Instant::now();
        for id in 0..config.rows {
            insert
                .execute(params![
                    text_key(id),
                    id as i64,
                    text_value(id, config.payload_bytes)
                ])
                .expect("sequential text key must insert");
        }
        let count = connection
            .query_row("SELECT COUNT(*) FROM speed_text", [], |row| {
                sqlite_u64(row, 0)
            })
            .expect("count text rows");
        results.push(result(
            config,
            "text_insert_sequential",
            repetition,
            config.rows,
            started.elapsed(),
            count,
        ));
    }

    {
        let mut select = connection
            .prepare_cached("SELECT key, value, payload FROM speed_text WHERE key = ?1")
            .expect("prepare text point read");
        let started = Instant::now();
        let checksum = keys.iter().fold(0_u64, |sum, key| {
            let (selected_key, value, payload) = select
                .query_row(params![text_key(*key)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        sqlite_u64(row, 1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .expect("loaded text key");
            black_box(selected_key);
            sum.wrapping_add(value).wrapping_add(payload.len() as u64)
        });
        results.push(result(
            config,
            "text_point_read_random",
            repetition,
            config.operations,
            started.elapsed(),
            checksum,
        ));
    }

    {
        let mut delete = connection
            .prepare_cached("DELETE FROM speed_int WHERE id = ?1")
            .expect("prepare integer delete");
        let started = Instant::now();
        let mut deleted = 0_u64;
        for key in keys {
            deleted += delete
                .execute(params![*key as i64])
                .expect("delete integer key") as u64;
        }
        results.push(result(
            config,
            "integer_delete_random",
            repetition,
            config.operations,
            started.elapsed(),
            deleted,
        ));
    }

    results
}

fn random_keys(count: u64, upper: u64, seed: u64) -> Vec<u64> {
    let mut rng = Rng::new(seed);
    (0..count).map(|_| rng.below(upper)).collect()
}

fn sqlite_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    row.get::<_, i64>(index).map(|value| value as u64)
}

fn text_key(value: u64) -> String {
    format!("key-{value:020}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_run_emits_all_matching_phases() {
        let config = Config {
            rows: 100,
            operations: 100,
            repetitions: 1,
            scan_length: 10,
            payload_bytes: 16,
            groups: 10,
            seed: 42,
        };
        let keys = random_keys(config.operations, config.rows, config.seed);
        let starts = random_keys(
            config.operations,
            config.rows - config.scan_length + 1,
            config.seed ^ 0x55aa,
        );
        let results = run_repetition(&config, 1, &keys, &starts);
        assert_eq!(results.len(), 9);
        assert!(results.iter().all(|result| result.operations > 0));
    }
}
