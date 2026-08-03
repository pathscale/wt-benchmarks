use std::hint::black_box;
use std::time::Instant;

use redb::{Database, Durability, ReadableDatabase, TableDefinition};
use wt_benchmarks::kv::{
    DurabilityMode, KvConfig, TransactionScope, emit, text_checksum, text_value,
};

const TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("kv");
type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> BenchResult<()> {
    let config = KvConfig::from_args("redb").unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    if config.durability == DurabilityMode::Memory {
        eprintln!("error: redb has no in-memory mode; use relaxed or durable");
        std::process::exit(2);
    }
    let point_keys = config.point_keys();
    let scan_starts = config.scan_starts();

    for repetition in 1..=config.repetitions {
        let directory = tempfile::tempdir()?;
        let database = Database::create(directory.path().join("bench.redb"))?;
        initialize(&database, &config)?;

        let started = Instant::now();
        insert_rows(&database, &config)?;
        emit(
            &config,
            "redb",
            "insert",
            repetition,
            config.rows,
            "not-applicable",
            started.elapsed().as_nanos(),
            config.rows,
        );

        let started = Instant::now();
        let checksum = read_points(&database, &config, &point_keys)?;
        emit(
            &config,
            "redb",
            "point_read",
            repetition,
            config.operations,
            "borrowed-guard",
            started.elapsed().as_nanos(),
            checksum,
        );

        let started = Instant::now();
        update_rows(&database, &config, &point_keys)?;
        emit(
            &config,
            "redb",
            "overwrite",
            repetition,
            config.operations,
            "not-applicable",
            started.elapsed().as_nanos(),
            config.operations,
        );

        let started = Instant::now();
        let checksum = scan_rows(&database, &config, &scan_starts)?;
        emit(
            &config,
            "redb",
            "range_scan",
            repetition,
            config.scan_operations,
            "borrowed-guard",
            started.elapsed().as_nanos(),
            checksum,
        );

        let started = Instant::now();
        let deleted = delete_rows(&database, &config, &point_keys)?;
        emit(
            &config,
            "redb",
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

fn initialize(database: &Database, config: &KvConfig) -> BenchResult<()> {
    let mut transaction = database.begin_write()?;
    set_durability(&mut transaction, config)?;
    transaction.open_table(TABLE)?;
    transaction.commit()?;
    Ok(())
}

fn set_durability(transaction: &mut redb::WriteTransaction, config: &KvConfig) -> BenchResult<()> {
    let durability = match config.durability {
        DurabilityMode::Relaxed => Durability::None,
        DurabilityMode::Durable => Durability::Immediate,
        DurabilityMode::Memory => unreachable!("validated by caller"),
    };
    transaction.set_durability(durability)?;
    Ok(())
}

fn insert_rows(database: &Database, config: &KvConfig) -> BenchResult<()> {
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for key in 0..config.rows {
                let encoded = text_value(key, config.payload_bytes);
                let mut transaction = database.begin_write()?;
                set_durability(&mut transaction, config)?;
                {
                    let mut table = transaction.open_table(TABLE)?;
                    table.insert(key, encoded.as_bytes())?;
                }
                transaction.commit()?;
            }
        }
        TransactionScope::Batch => {
            let mut transaction = database.begin_write()?;
            set_durability(&mut transaction, config)?;
            {
                let mut table = transaction.open_table(TABLE)?;
                for key in 0..config.rows {
                    let encoded = text_value(key, config.payload_bytes);
                    table.insert(key, encoded.as_bytes())?;
                }
            }
            transaction.commit()?;
        }
    }
    Ok(())
}

fn read_points(database: &Database, config: &KvConfig, keys: &[u64]) -> BenchResult<u64> {
    let mut checksum = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for key in keys {
                let transaction = database.begin_read()?;
                let table = transaction.open_table(TABLE)?;
                let row = table.get(*key)?.expect("loaded key");
                checksum = checksum.wrapping_add(text_checksum(
                    *key,
                    std::str::from_utf8(black_box(row.value()))?,
                ));
            }
        }
        TransactionScope::Batch => {
            let transaction = database.begin_read()?;
            let table = transaction.open_table(TABLE)?;
            for key in keys {
                let row = table.get(*key)?.expect("loaded key");
                checksum = checksum.wrapping_add(text_checksum(
                    *key,
                    std::str::from_utf8(black_box(row.value()))?,
                ));
            }
        }
    }
    Ok(checksum)
}

fn update_rows(database: &Database, config: &KvConfig, keys: &[u64]) -> BenchResult<()> {
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for key in keys {
                let encoded = text_value(key.wrapping_mul(17), config.payload_bytes);
                let mut transaction = database.begin_write()?;
                set_durability(&mut transaction, config)?;
                {
                    let mut table = transaction.open_table(TABLE)?;
                    table.insert(*key, encoded.as_bytes())?;
                }
                transaction.commit()?;
            }
        }
        TransactionScope::Batch => {
            let mut transaction = database.begin_write()?;
            set_durability(&mut transaction, config)?;
            {
                let mut table = transaction.open_table(TABLE)?;
                for key in keys {
                    let encoded = text_value(key.wrapping_mul(17), config.payload_bytes);
                    table.insert(*key, encoded.as_bytes())?;
                }
            }
            transaction.commit()?;
        }
    }
    Ok(())
}

fn scan_rows(database: &Database, config: &KvConfig, starts: &[u64]) -> BenchResult<u64> {
    let mut checksum = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for start in starts {
                let transaction = database.begin_read()?;
                let table = transaction.open_table(TABLE)?;
                for row in table.range(*start..)?.take(config.scan_length as usize) {
                    let (key, value) = row?;
                    checksum = checksum.wrapping_add(text_checksum(
                        key.value(),
                        std::str::from_utf8(value.value())?,
                    ));
                }
            }
        }
        TransactionScope::Batch => {
            let transaction = database.begin_read()?;
            let table = transaction.open_table(TABLE)?;
            for start in starts {
                for row in table.range(*start..)?.take(config.scan_length as usize) {
                    let (key, value) = row?;
                    checksum = checksum.wrapping_add(text_checksum(
                        key.value(),
                        std::str::from_utf8(value.value())?,
                    ));
                }
            }
        }
    }
    Ok(checksum)
}

fn delete_rows(database: &Database, config: &KvConfig, keys: &[u64]) -> BenchResult<u64> {
    let mut deleted = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for key in keys {
                let mut transaction = database.begin_write()?;
                set_durability(&mut transaction, config)?;
                {
                    let mut table = transaction.open_table(TABLE)?;
                    deleted += u64::from(table.remove(*key)?.is_some());
                }
                transaction.commit()?;
            }
        }
        TransactionScope::Batch => {
            let mut transaction = database.begin_write()?;
            set_durability(&mut transaction, config)?;
            {
                let mut table = transaction.open_table(TABLE)?;
                for key in keys {
                    deleted += u64::from(table.remove(*key)?.is_some());
                }
            }
            transaction.commit()?;
        }
    }
    Ok(deleted)
}
