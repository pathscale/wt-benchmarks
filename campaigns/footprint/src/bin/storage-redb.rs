use redb::{
    Database, MultimapTableDefinition, ReadableDatabase, ReadableTableMetadata, TableDefinition,
};
use wt_footprint_campaign::{
    StorageConfig, emit_storage, encoded_checksum, encoded_row, encoded_row_with_payload_seed,
    live_rows_after_churn,
};

const ROWS: TableDefinition<u64, &[u8]> = TableDefinition::new("footprint");
const ACCOUNT_INDEX: MultimapTableDefinition<u64, u64> =
    MultimapTableDefinition::new("footprint_account_idx");

type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> BenchResult<()> {
    let config = StorageConfig::parse().unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(2);
    });
    std::fs::create_dir_all(&config.path)?;
    let database_path = config.path.join("database.redb");
    let mut database = Database::create(&database_path)?;

    {
        let write = database.begin_write()?;
        {
            let mut rows = write.open_table(ROWS)?;
            let mut index = write.open_multimap_table(ACCOUNT_INDEX)?;
            for id in 0..config.rows {
                let encoded = encoded_row(id, config.payload_bytes);
                rows.insert(id, encoded.as_slice())?;
                index.insert(id % 10_000, id)?;
            }
        }
        write.commit()?;
    }
    emit_storage("redb", "loaded", &config, config.rows)?;

    {
        let write = database.begin_write()?;
        {
            let mut rows = write.open_table(ROWS)?;
            let mut index = write.open_multimap_table(ACCOUNT_INDEX)?;
            for id in 0..config.rows {
                match id % 4 {
                    0 => {
                        rows.remove(id)?;
                        index.remove(id % 10_000, id)?;
                    }
                    1 => {
                        let encoded = encoded_row_with_payload_seed(
                            id,
                            id.wrapping_add(1_000_000),
                            config.payload_bytes,
                        );
                        rows.insert(id, encoded.as_slice())?;
                    }
                    _ => {}
                }
            }
        }
        write.commit()?;
    }
    let expected = live_rows_after_churn(config.rows);
    verify_state(&database, &config, expected)?;
    emit_storage("redb", "churned", &config, expected)?;

    while database.compact()? {}
    verify_state(&database, &config, expected)?;
    emit_storage("redb", "compacted", &config, expected)?;
    drop(database);

    let database = Database::open(&database_path)?;
    verify_state(&database, &config, expected)?;
    emit_storage("redb", "reloaded", &config, expected)?;
    drop(database);
    emit_storage("redb", "closed-after-reload", &config, expected)?;
    Ok(())
}

fn verify_state(database: &Database, config: &StorageConfig, expected: u64) -> BenchResult<()> {
    let read = database.begin_read()?;
    let rows = read.open_table(ROWS)?;
    let count = rows.len()?;
    if count != expected {
        return Err(format!("redb row count mismatch: expected {expected}, got {count}").into());
    }
    if rows.get(0)?.is_some() {
        return Err("redb resurrected a deleted row".into());
    }
    if config.rows > 1 {
        let row = rows.get(1)?.ok_or("redb lost an expected row")?;
        let actual = encoded_checksum(row.value()).ok_or("redb stored an invalid encoded row")?;
        let expected_row = encoded_row_with_payload_seed(1, 1_000_001, config.payload_bytes);
        let expected_checksum =
            encoded_checksum(&expected_row).expect("benchmark encoding must be valid");
        if actual != expected_checksum {
            return Err("redb returned the wrong updated row".into());
        }
        let index = read.open_multimap_table(ACCOUNT_INDEX)?;
        if !index
            .get(1)?
            .any(|candidate| candidate.map(|value| value.value() == 1).unwrap_or(false))
        {
            return Err("redb lost a secondary-index entry".into());
        }
    }
    Ok(())
}
