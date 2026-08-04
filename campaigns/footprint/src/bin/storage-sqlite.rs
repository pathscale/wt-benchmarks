use rusqlite::{Connection, params};
use wt_footprint_campaign::{StorageConfig, emit_storage, live_rows_after_churn, payload};

type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> BenchResult<()> {
    let config = StorageConfig::parse().unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(2);
    });
    std::fs::create_dir_all(&config.path)?;
    let database_path = config.path.join("database.sqlite");
    let mut connection = Connection::open(&database_path)?;
    connection.execute_batch(
        "PRAGMA journal_mode=DELETE;
         PRAGMA synchronous=FULL;
         CREATE TABLE footprint (
             id INTEGER PRIMARY KEY,
             account_id INTEGER NOT NULL,
             sequence INTEGER NOT NULL,
             score REAL NOT NULL,
             payload TEXT NOT NULL
         );
         CREATE INDEX footprint_account_idx ON footprint(account_id);",
    )?;

    {
        let transaction = connection.transaction()?;
        {
            let mut insert = transaction.prepare(
                "INSERT INTO footprint(id, account_id, sequence, score, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for id in 0..config.rows {
                insert.execute(params![
                    id as i64,
                    (id % 10_000) as i64,
                    id.wrapping_mul(17) as i64,
                    id as f64 / 100.0,
                    payload(id, config.payload_bytes),
                ])?;
            }
        }
        transaction.commit()?;
    }
    emit_storage("sqlite-bundled", "loaded", &config, config.rows)?;
    emit_pages(&connection, "loaded")?;

    {
        let transaction = connection.transaction()?;
        {
            let mut delete = transaction.prepare("DELETE FROM footprint WHERE id = ?1")?;
            let mut update =
                transaction.prepare("UPDATE footprint SET payload = ?1 WHERE id = ?2")?;
            for id in 0..config.rows {
                match id % 4 {
                    0 => {
                        delete.execute([id as i64])?;
                    }
                    1 => {
                        update.execute(params![
                            payload(id.wrapping_add(1_000_000), config.payload_bytes),
                            id as i64
                        ])?;
                    }
                    _ => {}
                }
            }
        }
        transaction.commit()?;
    }
    let expected = live_rows_after_churn(config.rows);
    verify_state(&connection, &config, expected)?;
    emit_storage("sqlite-bundled", "churned", &config, expected)?;
    emit_pages(&connection, "churned")?;

    connection.execute_batch("VACUUM;")?;
    verify_state(&connection, &config, expected)?;
    emit_storage("sqlite-bundled", "vacuumed", &config, expected)?;
    emit_pages(&connection, "vacuumed")?;
    drop(connection);

    let connection = Connection::open(&database_path)?;
    verify_state(&connection, &config, expected)?;
    emit_storage("sqlite-bundled", "reloaded", &config, expected)?;
    drop(connection);
    emit_storage("sqlite-bundled", "closed-after-reload", &config, expected)?;
    Ok(())
}

fn verify_state(connection: &Connection, config: &StorageConfig, expected: u64) -> BenchResult<()> {
    let count: i64 =
        connection.query_row("SELECT count(*) FROM footprint", [], |row| row.get(0))?;
    if count as u64 != expected {
        return Err(format!("SQLite row count mismatch: expected {expected}, got {count}").into());
    }
    let deleted_exists: i64 = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM footprint WHERE id = 0)",
        [],
        |row| row.get(0),
    )?;
    if deleted_exists != 0 {
        return Err("SQLite resurrected a deleted row".into());
    }
    if config.rows > 1 {
        let updated: String = connection.query_row(
            "SELECT payload FROM footprint WHERE id = 1 AND account_id = 1",
            [],
            |row| row.get(0),
        )?;
        if updated != payload(1_000_001, config.payload_bytes) {
            return Err("SQLite returned the wrong updated payload".into());
        }
    }
    Ok(())
}

fn emit_pages(connection: &Connection, phase: &str) -> BenchResult<()> {
    let page_count: i64 = connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let freelist_count: i64 =
        connection.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
    let page_size: i64 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    println!(
        "{{\"benchmark\":\"storage-pages\",\"engine\":\"sqlite-bundled\",\"phase\":\"{phase}\",\"page_count\":{page_count},\"freelist_count\":{freelist_count},\"page_size\":{page_size}}}"
    );
    Ok(())
}
