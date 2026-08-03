use std::hint::black_box;
use std::time::Instant;

use rusqlite::{Connection, params};
use wt_benchmarks::kv::{
    DurabilityMode, KvConfig, TransactionScope, emit, text_checksum, text_value,
};

type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> BenchResult<()> {
    let config = KvConfig::from_args("sqlite").unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    if config.durability != DurabilityMode::Memory {
        eprintln!("error: this adapter measures SQLite :memory:; use --durability memory");
        std::process::exit(2);
    }
    if config.rows > i64::MAX as u64 {
        eprintln!("error: --rows must fit SQLite INTEGER");
        std::process::exit(2);
    }
    let point_keys = config.point_keys();
    let scan_starts = config.scan_starts();

    for repetition in 1..=config.repetitions {
        let mut connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "PRAGMA journal_mode=MEMORY;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=MEMORY;
             CREATE TABLE kv (
                 id INTEGER PRIMARY KEY,
                 payload TEXT NOT NULL
             );",
        )?;

        let started = Instant::now();
        insert_rows(&mut connection, &config)?;
        emit(
            &config,
            "sqlite-memory",
            "insert",
            repetition,
            config.rows,
            "not-applicable",
            started.elapsed().as_nanos(),
            config.rows,
        );

        let started = Instant::now();
        let checksum = read_points(&mut connection, &config, &point_keys)?;
        emit(
            &config,
            "sqlite-memory",
            "point_read",
            repetition,
            config.operations,
            "decoded-owned-row",
            started.elapsed().as_nanos(),
            checksum,
        );

        let started = Instant::now();
        update_rows(&mut connection, &config, &point_keys)?;
        emit(
            &config,
            "sqlite-memory",
            "overwrite",
            repetition,
            config.operations,
            "not-applicable",
            started.elapsed().as_nanos(),
            config.operations,
        );

        let started = Instant::now();
        let checksum = scan_rows(&mut connection, &config, &scan_starts)?;
        emit(
            &config,
            "sqlite-memory",
            "range_scan",
            repetition,
            config.scan_operations,
            "decoded-owned-row",
            started.elapsed().as_nanos(),
            checksum,
        );

        let started = Instant::now();
        let deleted = delete_rows(&mut connection, &config, &point_keys)?;
        emit(
            &config,
            "sqlite-memory",
            "delete_random",
            repetition,
            config.operations,
            "not-applicable",
            started.elapsed().as_nanos(),
            deleted,
        );
    }
    Ok(())
}

fn insert_rows(connection: &mut Connection, config: &KvConfig) -> BenchResult<()> {
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            let mut insert = connection.prepare_cached("INSERT INTO kv VALUES (?1, ?2)")?;
            for key in 0..config.rows {
                insert.execute(params![key as i64, text_value(key, config.payload_bytes)])?;
            }
        }
        TransactionScope::Batch => {
            let transaction = connection.transaction()?;
            {
                let mut insert = transaction.prepare_cached("INSERT INTO kv VALUES (?1, ?2)")?;
                for key in 0..config.rows {
                    insert.execute(params![key as i64, text_value(key, config.payload_bytes)])?;
                }
            }
            transaction.commit()?;
        }
    }
    Ok(())
}

fn read_points(connection: &mut Connection, config: &KvConfig, keys: &[u64]) -> BenchResult<u64> {
    let mut checksum = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            let mut select =
                connection.prepare_cached("SELECT id, payload FROM kv WHERE id = ?1")?;
            for key in keys {
                let (id, payload) = select.query_row(params![*key as i64], decode_row)?;
                checksum = checksum.wrapping_add(text_checksum(id, black_box(&payload)));
            }
        }
        TransactionScope::Batch => {
            let transaction = connection.transaction()?;
            {
                let mut select =
                    transaction.prepare_cached("SELECT id, payload FROM kv WHERE id = ?1")?;
                for key in keys {
                    let (id, payload) = select.query_row(params![*key as i64], decode_row)?;
                    checksum = checksum.wrapping_add(text_checksum(id, black_box(&payload)));
                }
            }
            transaction.commit()?;
        }
    }
    Ok(checksum)
}

fn update_rows(connection: &mut Connection, config: &KvConfig, keys: &[u64]) -> BenchResult<()> {
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            let mut update =
                connection.prepare_cached("UPDATE kv SET payload = ?1 WHERE id = ?2")?;
            for key in keys {
                update.execute(params![
                    text_value(key.wrapping_mul(17), config.payload_bytes),
                    *key as i64
                ])?;
            }
        }
        TransactionScope::Batch => {
            let transaction = connection.transaction()?;
            {
                let mut update =
                    transaction.prepare_cached("UPDATE kv SET payload = ?1 WHERE id = ?2")?;
                for key in keys {
                    update.execute(params![
                        text_value(key.wrapping_mul(17), config.payload_bytes),
                        *key as i64
                    ])?;
                }
            }
            transaction.commit()?;
        }
    }
    Ok(())
}

fn scan_rows(connection: &mut Connection, config: &KvConfig, starts: &[u64]) -> BenchResult<u64> {
    let mut checksum = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            let mut select = connection
                .prepare_cached("SELECT id, payload FROM kv WHERE id >= ?1 ORDER BY id LIMIT ?2")?;
            for start in starts {
                checksum =
                    checksum.wrapping_add(scan_once(&mut select, *start, config.scan_length)?);
            }
        }
        TransactionScope::Batch => {
            let transaction = connection.transaction()?;
            {
                let mut select = transaction.prepare_cached(
                    "SELECT id, payload FROM kv WHERE id >= ?1 ORDER BY id LIMIT ?2",
                )?;
                for start in starts {
                    checksum =
                        checksum.wrapping_add(scan_once(&mut select, *start, config.scan_length)?);
                }
            }
            transaction.commit()?;
        }
    }
    Ok(checksum)
}

fn scan_once(
    statement: &mut rusqlite::CachedStatement<'_>,
    start: u64,
    length: u64,
) -> rusqlite::Result<u64> {
    let mapped = statement.query_map(params![start as i64, length as i64], decode_row)?;
    let rows = mapped.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().fold(0_u64, |checksum, (id, payload)| {
        checksum.wrapping_add(text_checksum(id, black_box(&payload)))
    }))
}

fn delete_rows(connection: &mut Connection, config: &KvConfig, keys: &[u64]) -> BenchResult<u64> {
    let mut deleted = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            let mut delete = connection.prepare_cached("DELETE FROM kv WHERE id = ?1")?;
            for key in keys {
                deleted += delete.execute(params![*key as i64])? as u64;
            }
        }
        TransactionScope::Batch => {
            let transaction = connection.transaction()?;
            {
                let mut delete = transaction.prepare_cached("DELETE FROM kv WHERE id = ?1")?;
                for key in keys {
                    deleted += delete.execute(params![*key as i64])? as u64;
                }
            }
            transaction.commit()?;
        }
    }
    Ok(deleted)
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, String)> {
    Ok((row.get::<_, i64>(0)? as u64, row.get(1)?))
}
