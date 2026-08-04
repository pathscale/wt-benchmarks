//! DuckDB adopted-twin of the KV workload. Same operations, same shared harness
//! (`src/kv.rs`), same checksums as kv-worktable / kv-lmdb / kv-redb — only the
//! engine differs. DuckDB is a high-performance embeddable SQL engine with a
//! runtime schema; it supports both in-memory and on-disk databases, so it fills
//! both matrix cells:
//!   --durability memory  -> DuckDB :memory:
//!   --durability relaxed/durable -> DuckDB on-disk file
//!
//! DuckDB is bundled (compiled from source into the binary), so this is portable
//! to the Linux paper box with no system-library dependency.

use std::hint::black_box;
use std::time::Instant;

use duckdb::{params, Connection};
use wt_benchmarks::kv::{
    DurabilityMode, KvConfig, TransactionScope, emit, text_checksum, text_value,
};

type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> BenchResult<()> {
    let config = KvConfig::from_args("duckdb").unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    let point_keys = config.point_keys();
    let scan_starts = config.scan_starts();

    for repetition in 1..=config.repetitions {
        // In-memory for `memory`; a fresh temp file for relaxed/durable.
        let directory = tempfile::tempdir()?;
        let connection = match config.durability {
            DurabilityMode::Memory => Connection::open_in_memory()?,
            DurabilityMode::Relaxed | DurabilityMode::Durable => {
                Connection::open(directory.path().join("bench.duckdb"))?
            }
        };
        connection.execute_batch(
            "CREATE TABLE kv (id UBIGINT PRIMARY KEY, payload VARCHAR NOT NULL);",
        )?;

        let started = Instant::now();
        insert_rows(&connection, &config)?;
        emit(&config, "duckdb", "insert", repetition, config.rows,
            "not-applicable", started.elapsed().as_nanos(), config.rows);

        let started = Instant::now();
        let checksum = read_points(&connection, &config, &point_keys)?;
        emit(&config, "duckdb", "point_read", repetition, config.operations,
            "materialized-owned-row", started.elapsed().as_nanos(), checksum);

        let started = Instant::now();
        update_rows(&connection, &config, &point_keys)?;
        emit(&config, "duckdb", "overwrite", repetition, config.operations,
            "not-applicable", started.elapsed().as_nanos(), config.operations);

        let started = Instant::now();
        let checksum = scan_rows(&connection, &config, &scan_starts)?;
        emit(&config, "duckdb", "range_scan", repetition, config.scan_operations,
            "materialized-owned-row", started.elapsed().as_nanos(), checksum);

        let started = Instant::now();
        let deleted = delete_rows(&connection, &config, &point_keys)?;
        emit(&config, "duckdb", "delete_random", repetition, config.operations,
            "not-applicable", started.elapsed().as_nanos(), deleted);
    }
    Ok(())
}

fn insert_rows(connection: &Connection, config: &KvConfig) -> BenchResult<()> {
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            let mut stmt = connection.prepare("INSERT INTO kv (id, payload) VALUES (?, ?)")?;
            for key in 0..config.rows {
                stmt.execute(params![key, text_value(key, config.payload_bytes)])?;
            }
        }
        TransactionScope::Batch => {
            connection.execute_batch("BEGIN TRANSACTION;")?;
            {
                let mut stmt = connection.prepare("INSERT INTO kv (id, payload) VALUES (?, ?)")?;
                for key in 0..config.rows {
                    stmt.execute(params![key, text_value(key, config.payload_bytes)])?;
                }
            }
            connection.execute_batch("COMMIT;")?;
        }
    }
    Ok(())
}

fn read_points(connection: &Connection, _config: &KvConfig, keys: &[u64]) -> BenchResult<u64> {
    let mut checksum = 0_u64;
    let mut stmt = connection.prepare("SELECT payload FROM kv WHERE id = ?")?;
    for key in keys {
        let payload: String = stmt.query_row(params![*key], |row| row.get(0))?;
        checksum = checksum.wrapping_add(text_checksum(*key, black_box(&payload)));
    }
    Ok(checksum)
}

fn update_rows(connection: &Connection, config: &KvConfig, keys: &[u64]) -> BenchResult<()> {
    let mut stmt = connection.prepare("UPDATE kv SET payload = ? WHERE id = ?")?;
    for key in keys {
        stmt.execute(params![text_value(key.wrapping_mul(17), config.payload_bytes), *key])?;
    }
    Ok(())
}

fn scan_rows(connection: &Connection, config: &KvConfig, starts: &[u64]) -> BenchResult<u64> {
    let mut checksum = 0_u64;
    let mut stmt = connection
        .prepare("SELECT id, payload FROM kv WHERE id >= ? ORDER BY id LIMIT ?")?;
    for start in starts {
        let mut rows = stmt.query(params![*start, config.scan_length])?;
        while let Some(row) = rows.next()? {
            let id: u64 = row.get(0)?;
            let payload: String = row.get(1)?;
            checksum = checksum.wrapping_add(text_checksum(id, &payload));
        }
    }
    Ok(checksum)
}

fn delete_rows(connection: &Connection, _config: &KvConfig, keys: &[u64]) -> BenchResult<u64> {
    let mut deleted = 0_u64;
    let mut stmt = connection.prepare("DELETE FROM kv WHERE id = ?")?;
    for key in keys {
        deleted += stmt.execute(params![*key])? as u64;
    }
    Ok(deleted)
}
