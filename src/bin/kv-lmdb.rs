//! LMDB (via heed) adopted-twin of the KV workload. Same operations, same
//! shared harness (`src/kv.rs`), same checksums as kv-worktable / kv-redb —
//! only the storage engine differs. LMDB is a memory-mapped B+tree that
//! advertises very high read throughput; it is durable/persistent by design and
//! has no in-memory mode, so --durability memory is rejected (mirrors redb).
//!
//! Keys are stored big-endian so LMDB's native byte order matches u64 order,
//! keeping range_scan semantically identical to the other adapters.

use std::hint::black_box;
use std::time::Instant;

use heed::types::{Bytes, U64};
use heed::{byteorder::BigEndian, Database, EnvOpenOptions};
use wt_benchmarks::kv::{
    DurabilityMode, KvConfig, TransactionScope, emit, text_checksum, text_value,
};

type LmdbKey = U64<BigEndian>;
type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

// LMDB needs a map size ceiling up front; size generously for the bench.
const MAP_SIZE: usize = 8 * 1024 * 1024 * 1024;

fn main() -> BenchResult<()> {
    let config = KvConfig::from_args("lmdb").unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    if config.durability == DurabilityMode::Memory {
        eprintln!("error: LMDB has no in-memory mode; use relaxed or durable");
        std::process::exit(2);
    }
    let point_keys = config.point_keys();
    let scan_starts = config.scan_starts();

    for repetition in 1..=config.repetitions {
        let directory = tempfile::tempdir()?;
        // Relaxed => no fsync on commit (LMDB MDB_NOSYNC); durable => default sync.
        let unsafe_no_sync = config.durability == DurabilityMode::Relaxed;
        let env = unsafe {
            let mut opts = EnvOpenOptions::new();
            opts.map_size(MAP_SIZE).max_dbs(1);
            if unsafe_no_sync {
                opts.flags(heed::EnvFlags::NO_SYNC | heed::EnvFlags::NO_META_SYNC);
            }
            opts.open(directory.path())?
        };
        let db: Database<LmdbKey, Bytes> = {
            let mut wtxn = env.write_txn()?;
            let db = env.create_database(&mut wtxn, None)?;
            wtxn.commit()?;
            db
        };

        let started = Instant::now();
        insert_rows(&env, &db, &config)?;
        emit(&config, "lmdb", "insert", repetition, config.rows,
            "not-applicable", started.elapsed().as_nanos(), config.rows);

        let started = Instant::now();
        let checksum = read_points(&env, &db, &config, &point_keys)?;
        emit(&config, "lmdb", "point_read", repetition, config.operations,
            "borrowed-mmap", started.elapsed().as_nanos(), checksum);

        let started = Instant::now();
        update_rows(&env, &db, &config, &point_keys)?;
        emit(&config, "lmdb", "overwrite", repetition, config.operations,
            "not-applicable", started.elapsed().as_nanos(), config.operations);

        let started = Instant::now();
        let checksum = scan_rows(&env, &db, &config, &scan_starts)?;
        emit(&config, "lmdb", "range_scan", repetition, config.scan_operations,
            "borrowed-mmap", started.elapsed().as_nanos(), checksum);

        let started = Instant::now();
        let deleted = delete_rows(&env, &db, &config, &point_keys)?;
        emit(&config, "lmdb", "delete_random", repetition, config.operations,
            "not-applicable", started.elapsed().as_nanos(), deleted);
    }
    Ok(())
}

fn insert_rows(env: &heed::Env, db: &Database<LmdbKey, Bytes>, config: &KvConfig) -> BenchResult<()> {
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for key in 0..config.rows {
                let encoded = text_value(key, config.payload_bytes);
                let mut wtxn = env.write_txn()?;
                db.put(&mut wtxn, &key, encoded.as_bytes())?;
                wtxn.commit()?;
            }
        }
        TransactionScope::Batch => {
            let mut wtxn = env.write_txn()?;
            for key in 0..config.rows {
                let encoded = text_value(key, config.payload_bytes);
                db.put(&mut wtxn, &key, encoded.as_bytes())?;
            }
            wtxn.commit()?;
        }
    }
    Ok(())
}

fn read_points(env: &heed::Env, db: &Database<LmdbKey, Bytes>, config: &KvConfig, keys: &[u64]) -> BenchResult<u64> {
    let mut checksum = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for key in keys {
                let rtxn = env.read_txn()?;
                let value = db.get(&rtxn, key)?.expect("loaded key");
                checksum = checksum.wrapping_add(text_checksum(*key, std::str::from_utf8(black_box(value))?));
            }
        }
        TransactionScope::Batch => {
            let rtxn = env.read_txn()?;
            for key in keys {
                let value = db.get(&rtxn, key)?.expect("loaded key");
                checksum = checksum.wrapping_add(text_checksum(*key, std::str::from_utf8(black_box(value))?));
            }
        }
    }
    Ok(checksum)
}

fn update_rows(env: &heed::Env, db: &Database<LmdbKey, Bytes>, config: &KvConfig, keys: &[u64]) -> BenchResult<()> {
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for key in keys {
                let encoded = text_value(key.wrapping_mul(17), config.payload_bytes);
                let mut wtxn = env.write_txn()?;
                db.put(&mut wtxn, key, encoded.as_bytes())?;
                wtxn.commit()?;
            }
        }
        TransactionScope::Batch => {
            let mut wtxn = env.write_txn()?;
            for key in keys {
                let encoded = text_value(key.wrapping_mul(17), config.payload_bytes);
                db.put(&mut wtxn, key, encoded.as_bytes())?;
            }
            wtxn.commit()?;
        }
    }
    Ok(())
}

fn scan_rows(env: &heed::Env, db: &Database<LmdbKey, Bytes>, config: &KvConfig, starts: &[u64]) -> BenchResult<u64> {
    let mut checksum = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for start in starts {
                let rtxn = env.read_txn()?;
                for row in db.range(&rtxn, &(*start..))?.take(config.scan_length as usize) {
                    let (key, value) = row?;
                    checksum = checksum.wrapping_add(text_checksum(key, std::str::from_utf8(value)?));
                }
            }
        }
        TransactionScope::Batch => {
            let rtxn = env.read_txn()?;
            for start in starts {
                for row in db.range(&rtxn, &(*start..))?.take(config.scan_length as usize) {
                    let (key, value) = row?;
                    checksum = checksum.wrapping_add(text_checksum(key, std::str::from_utf8(value)?));
                }
            }
        }
    }
    Ok(checksum)
}

fn delete_rows(env: &heed::Env, db: &Database<LmdbKey, Bytes>, config: &KvConfig, keys: &[u64]) -> BenchResult<u64> {
    let mut deleted = 0_u64;
    match config.transaction_scope {
        TransactionScope::PerOperation => {
            for key in keys {
                let mut wtxn = env.write_txn()?;
                deleted += u64::from(db.delete(&mut wtxn, key)?);
                wtxn.commit()?;
            }
        }
        TransactionScope::Batch => {
            let mut wtxn = env.write_txn()?;
            for key in keys {
                deleted += u64::from(db.delete(&mut wtxn, key)?);
            }
            wtxn.commit()?;
        }
    }
    Ok(deleted)
}
