use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, OpenFlags, params};
use wt_benchmarks::config::Config;
use wt_benchmarks::result::{LatencySummary, RunResult};
use wt_benchmarks::ycsb::{
    AcknowledgedKeyspace, FIELD_COUNT, Operation, OperationKind, generate_streams, make_fields,
};

static NEXT_DATABASE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct WorkerResult {
    completed: u64,
    errors: u64,
    retryable_errors: u64,
    counts: [u64; 5],
    error_counts: [u64; 5],
    latency: [Vec<u64>; 5],
}

fn main() {
    let config = Config::from_args().unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    if config.records > i64::MAX as u64 {
        eprintln!("error: --records must fit SQLite INTEGER");
        std::process::exit(2);
    }

    for repetition in 1..=config.repetitions {
        let result = run_repetition(&config, repetition);
        println!(
            "{}",
            serde_json::to_string(&result).expect("result must serialize")
        );
    }
}

fn run_repetition(config: &Config, repetition: usize) -> RunResult {
    let uri = format!(
        "file:wt-ycsb-{}-{repetition}-{}?mode=memory&cache=shared",
        std::process::id(),
        NEXT_DATABASE_ID.fetch_add(1, Ordering::Relaxed)
    );
    let master = open(&uri).expect("open SQLite shared in-memory database");
    master
        .execute_batch(
            "PRAGMA journal_mode=MEMORY;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=MEMORY;
             CREATE TABLE ycsb (
                 id INTEGER PRIMARY KEY,
                 field0 TEXT NOT NULL,
                 field1 TEXT NOT NULL,
                 field2 TEXT NOT NULL,
                 field3 TEXT NOT NULL,
                 field4 TEXT NOT NULL,
                 field5 TEXT NOT NULL,
                 field6 TEXT NOT NULL,
                 field7 TEXT NOT NULL,
                 field8 TEXT NOT NULL,
                 field9 TEXT NOT NULL
             );",
        )
        .expect("create SQLite YCSB schema");

    let load_started = Instant::now();
    {
        let mut insert = master
            .prepare_cached(
                "INSERT INTO ycsb VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )
            .expect("prepare YCSB load");
        for key in 0..config.records {
            let fields = make_fields(key ^ config.seed, config.field_bytes);
            insert_fields(&mut insert, key, &fields).expect("initial YCSB key must insert");
        }
    }
    let load_elapsed_ns = load_started.elapsed().as_nanos();

    let distribution = config
        .distribution_override
        .unwrap_or_else(|| config.workload.default_distribution());
    let generated = generate_streams(config);
    let acknowledged = generated.acknowledged;
    let ready = Arc::new(Barrier::new(config.threads + 1));
    let start = Arc::new(Barrier::new(config.threads + 1));
    let mut handles = Vec::with_capacity(config.threads);
    for stream in generated.streams {
        let uri = uri.clone();
        let acknowledged = acknowledged.as_ref().map(Arc::clone);
        let ready = Arc::clone(&ready);
        let start = Arc::clone(&start);
        let sample_every = config.sample_every;
        handles.push(std::thread::spawn(move || {
            let connection = open(&uri).expect("open SQLite YCSB worker connection");
            ready.wait();
            start.wait();
            run_worker(&connection, stream, sample_every, acknowledged)
        }));
    }

    ready.wait();
    let measured_started = Instant::now();
    start.wait();
    let mut combined = WorkerResult::default();
    for handle in handles {
        let mut worker = handle.join().expect("SQLite YCSB worker panicked");
        combined.completed += worker.completed;
        combined.errors += worker.errors;
        combined.retryable_errors += worker.retryable_errors;
        for kind in OperationKind::ALL {
            combined.counts[kind as usize] += worker.counts[kind as usize];
            combined.error_counts[kind as usize] += worker.error_counts[kind as usize];
            combined.latency[kind as usize].append(&mut worker.latency[kind as usize]);
        }
    }
    let elapsed_ns = measured_started.elapsed().as_nanos();

    let mut operation_counts = BTreeMap::new();
    let mut operation_errors = BTreeMap::new();
    let mut latency = BTreeMap::new();
    for kind in OperationKind::ALL {
        operation_counts.insert(kind.as_str().to_owned(), combined.counts[kind as usize]);
        operation_errors.insert(
            kind.as_str().to_owned(),
            combined.error_counts[kind as usize],
        );
        latency.insert(
            kind.as_str().to_owned(),
            LatencySummary::from_samples(std::mem::take(&mut combined.latency[kind as usize])),
        );
    }
    RunResult::for_engine(
        config,
        repetition,
        &distribution.to_string(),
        "sqlite-memory",
        combined.completed,
        combined.errors,
        combined.retryable_errors,
        load_elapsed_ns,
        elapsed_ns,
        operation_counts,
        operation_errors,
        latency,
        "SQLite :memory: shared-cache; one autocommit statement per operation; read-modify-write uses two statements",
        "SQLite values decoded into owned Rust rows",
        Some(rusqlite::version()),
    )
}

fn open(uri: &str) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}

fn run_worker(
    connection: &Connection,
    stream: Vec<Operation>,
    sample_every: u64,
    acknowledged: Option<Arc<AcknowledgedKeyspace>>,
) -> WorkerResult {
    let mut result = WorkerResult::default();
    let sample_capacity = stream.len() / sample_every as usize + 1;
    for samples in &mut result.latency {
        samples.reserve(sample_capacity);
    }
    for (index, operation) in stream.into_iter().enumerate() {
        let kind = operation.kind();
        let sampled = (index as u64).is_multiple_of(sample_every);
        let started = sampled.then(Instant::now);
        let acknowledged_read = match &operation {
            Operation::ReadAcknowledged {
                sample,
                distribution,
            } => Some(
                acknowledged
                    .as_deref()
                    .expect("acknowledged read requires Workload D state")
                    .resolve(*sample, *distribution),
            ),
            _ => None,
        };
        let (success, retries) = execute_with_retry(connection, &operation, acknowledged_read);
        if success
            && let Operation::Insert { key, .. } = &operation
            && let Some(acknowledged) = acknowledged.as_deref()
        {
            acknowledged.acknowledge(*key);
        }
        if let Some(started) = started {
            result.latency[kind as usize]
                .push(started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
        }
        result.counts[kind as usize] += 1;
        result.completed += u64::from(success);
        result.errors += u64::from(!success);
        result.error_counts[kind as usize] += u64::from(!success);
        result.retryable_errors += retries;
    }
    result
}

fn execute_with_retry(
    connection: &Connection,
    operation: &Operation,
    acknowledged_read: Option<u64>,
) -> (bool, u64) {
    let mut retries = 0_u64;
    loop {
        match execute(connection, operation, acknowledged_read) {
            Ok(success) => return (success, retries),
            Err(error) if retryable(&error) && retries < 1_000 => {
                retries += 1;
                if retries < 10 {
                    std::thread::yield_now();
                } else {
                    std::thread::sleep(Duration::from_micros(50));
                }
            }
            Err(_) => return (false, retries),
        }
    }
}

fn retryable(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(code.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn execute(
    connection: &Connection,
    operation: &Operation,
    acknowledged_read: Option<u64>,
) -> rusqlite::Result<bool> {
    match operation {
        Operation::Read { key } => read(connection, *key),
        Operation::ReadAcknowledged { .. } => read(
            connection,
            acknowledged_read.expect("acknowledged read key must be resolved before execution"),
        ),
        Operation::Update { key, field, value } => update(connection, *key, *field, value),
        Operation::Insert { key, fields } => connection
            .prepare_cached(
                "INSERT INTO ycsb VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )
            .and_then(|mut statement| insert_fields(&mut statement, *key, fields))
            .map(|inserted| inserted == 1),
        Operation::Scan { start, length } => scan(connection, *start, *length),
        Operation::ReadModifyWrite { key, field, value } => {
            if read(connection, *key)? {
                update(connection, *key, *field, value)
            } else {
                Ok(false)
            }
        }
    }
}

fn read(connection: &Connection, key: u64) -> rusqlite::Result<bool> {
    match connection
        .prepare_cached(
            "SELECT field0, field1, field2, field3, field4,
                    field5, field6, field7, field8, field9
             FROM ycsb WHERE id = ?1",
        )
        .and_then(|mut statement| {
            statement.query_row(params![key as i64], |row| {
                let fields = decode_fields(row)?;
                black_box(fields);
                Ok(())
            })
        }) {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(error) => Err(error),
    }
}

fn update(connection: &Connection, key: u64, field: u8, value: &str) -> rusqlite::Result<bool> {
    let sql = match field {
        0 => "UPDATE ycsb SET field0 = ?1 WHERE id = ?2",
        1 => "UPDATE ycsb SET field1 = ?1 WHERE id = ?2",
        2 => "UPDATE ycsb SET field2 = ?1 WHERE id = ?2",
        3 => "UPDATE ycsb SET field3 = ?1 WHERE id = ?2",
        4 => "UPDATE ycsb SET field4 = ?1 WHERE id = ?2",
        5 => "UPDATE ycsb SET field5 = ?1 WHERE id = ?2",
        6 => "UPDATE ycsb SET field6 = ?1 WHERE id = ?2",
        7 => "UPDATE ycsb SET field7 = ?1 WHERE id = ?2",
        8 => "UPDATE ycsb SET field8 = ?1 WHERE id = ?2",
        9 => "UPDATE ycsb SET field9 = ?1 WHERE id = ?2",
        _ => return Ok(false),
    };
    connection
        .prepare_cached(sql)
        .and_then(|mut statement| statement.execute(params![value, key as i64]))
        .map(|updated| updated == 1)
}

fn scan(connection: &Connection, start: u64, length: u64) -> rusqlite::Result<bool> {
    connection
        .prepare_cached(
            "SELECT field0, field1, field2, field3, field4,
                    field5, field6, field7, field8, field9
             FROM ycsb WHERE id >= ?1 ORDER BY id LIMIT ?2",
        )
        .and_then(|mut statement| {
            let mapped =
                statement.query_map(params![start as i64, length as i64], decode_fields)?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map(|rows| {
            black_box(rows);
            true
        })
}

fn insert_fields(
    statement: &mut rusqlite::CachedStatement<'_>,
    key: u64,
    fields: &[String; FIELD_COUNT],
) -> rusqlite::Result<usize> {
    statement.execute(params![
        key as i64, &fields[0], &fields[1], &fields[2], &fields[3], &fields[4], &fields[5],
        &fields[6], &fields[7], &fields[8], &fields[9]
    ])
}

fn decode_fields(row: &rusqlite::Row<'_>) -> rusqlite::Result<[String; FIELD_COUNT]> {
    Ok([
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use wt_benchmarks::ycsb::Workload;

    #[test]
    fn all_six_workloads_complete_single_thread() {
        for workload in [
            Workload::A,
            Workload::B,
            Workload::C,
            Workload::D,
            Workload::E,
            Workload::F,
        ] {
            let config = Config {
                workload,
                records: 100,
                operations: 1_000,
                threads: 1,
                repetitions: 1,
                sample_every: 100,
                seed: 42,
                field_bytes: 16,
                scan_max: 10,
                zipf_theta: 0.99,
                distribution_override: None,
            };
            let result = run_repetition(&config, 1);
            assert_eq!(result.errors, 0, "workload {workload}");
            assert_eq!(result.operations_completed, config.operations);
        }
    }

    #[test]
    fn mixed_four_thread_workload_retries_locks_without_losing_operations() {
        let config = Config {
            workload: Workload::A,
            records: 100,
            operations: 1_000,
            threads: 4,
            repetitions: 1,
            sample_every: 100,
            seed: 42,
            field_bytes: 16,
            scan_max: 10,
            zipf_theta: 0.99,
            distribution_override: None,
        };
        let result = run_repetition(&config, 1);
        assert_eq!(result.errors, 0);
        assert_eq!(result.operations_completed, config.operations);
    }

    #[test]
    fn concurrent_workload_d_never_reads_unacknowledged_inserts() {
        let config = Config {
            workload: Workload::D,
            records: 1_000,
            operations: 20_000,
            threads: 8,
            repetitions: 1,
            sample_every: 256,
            seed: 42,
            field_bytes: 16,
            scan_max: 10,
            zipf_theta: 0.99,
            distribution_override: None,
        };
        let result = run_repetition(&config, 1);
        assert_eq!(result.errors, 0);
        assert_eq!(result.operations_completed, config.operations);
    }
}
