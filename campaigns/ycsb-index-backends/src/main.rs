use std::hint::black_box;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tokio::sync::Barrier;
use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::config::Config;
use wt_benchmarks::result::LatencySummary;
use wt_benchmarks::ycsb::{FIELD_COUNT, Operation, Workload, generate_streams, make_fields};

worktable!(
    name: YcsbWti,
    persist: false,
    columns: {
        id: u64 primary_key using worktables_index,
        field0: String,
        field1: String,
        field2: String,
        field3: String,
        field4: String,
        field5: String,
        field6: String,
        field7: String,
        field8: String,
        field9: String,
    }
);

worktable!(
    name: YcsbIndexset,
    persist: false,
    columns: {
        id: u64 primary_key using indexset,
        field0: String,
        field1: String,
        field2: String,
        field3: String,
        field4: String,
        field5: String,
        field6: String,
        field7: String,
        field8: String,
        field9: String,
    }
);

worktable!(
    name: YcsbCongee,
    persist: false,
    columns: {
        id: u64 primary_key using congee,
        field0: String,
        field1: String,
        field2: String,
        field3: String,
        field4: String,
        field5: String,
        field6: String,
        field7: String,
        field8: String,
        field9: String,
    }
);

worktable!(
    name: YcsbArctic,
    persist: false,
    columns: {
        id: u64 primary_key using arctic,
        field0: String,
        field1: String,
        field2: String,
        field3: String,
        field4: String,
        field5: String,
        field6: String,
        field7: String,
        field8: String,
        field9: String,
    }
);

#[derive(Clone, Copy)]
enum Backend {
    WorkTablesIndex,
    Indexset,
    Congee,
    Arctic,
}

impl Backend {
    fn as_str(self) -> &'static str {
        match self {
            Self::WorkTablesIndex => "worktables_index",
            Self::Indexset => "indexset",
            Self::Congee => "congee",
            Self::Arctic => "arctic",
        }
    }
}

impl FromStr for Backend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "worktables_index" | "wti" | "default" => Ok(Self::WorkTablesIndex),
            "indexset" => Ok(Self::Indexset),
            "congee" => Ok(Self::Congee),
            "arctic" => Ok(Self::Arctic),
            _ => Err(format!("unknown WT_INDEX_BACKEND: {value}")),
        }
    }
}

#[derive(Default)]
struct WorkerResult {
    operations: u64,
    inserts: u64,
    first_read_misses: u64,
    retry_recovered: u64,
    final_read_errors: u64,
    insert_errors: u64,
    read_latency: Vec<u64>,
    insert_latency: Vec<u64>,
}

#[derive(Serialize)]
struct ResultRecord {
    schema_version: u32,
    suite: &'static str,
    backend: &'static str,
    repetition: usize,
    campaign_pair: Option<usize>,
    records_initial: u64,
    operations: u64,
    threads: usize,
    inserts: u64,
    first_read_misses: u64,
    retry_recovered: u64,
    final_read_errors: u64,
    insert_errors: u64,
    elapsed_ns: u128,
    ops_per_second: f64,
    read_latency: LatencySummary,
    insert_latency: LatencySummary,
    immediate_retry_on_miss: bool,
    acknowledged_insert_frontier: bool,
    stable_index_read_retry: bool,
    persistence: bool,
    target_arch: &'static str,
    target_os: &'static str,
}

fn row_fields(key: u64, config: &Config) -> [String; FIELD_COUNT] {
    make_fields(key ^ config.seed, config.field_bytes)
}

macro_rules! row {
    ($row:ident, $key:expr, $fields:expr) => {{
        let [
            field0,
            field1,
            field2,
            field3,
            field4,
            field5,
            field6,
            field7,
            field8,
            field9,
        ] = $fields;
        $row {
            id: $key,
            field0,
            field1,
            field2,
            field3,
            field4,
            field5,
            field6,
            field7,
            field8,
            field9,
        }
    }};
}

macro_rules! run_repetition {
    ($config:expr, $backend:expr, $repetition:expr, $table:ty, $row:ident) => {{
        let config = $config;
        let table = Arc::new(<$table>::default());
        for key in 0..config.records {
            table
                .insert(row!($row, key, row_fields(key, config)))
                .expect("initial YCSB key must insert");
        }

        let generated = generate_streams(config);
        let acknowledged = generated
            .acknowledged
            .expect("Workload D must have acknowledged frontier");
        let ready = Arc::new(Barrier::new(config.threads + 1));
        let start = Arc::new(Barrier::new(config.threads + 1));
        let mut handles = Vec::with_capacity(config.threads);

        for stream in generated.streams {
            let table = Arc::clone(&table);
            let acknowledged = Arc::clone(&acknowledged);
            let ready = Arc::clone(&ready);
            let start = Arc::clone(&start);
            let sample_every = config.sample_every;
            handles.push(tokio::spawn(async move {
                let mut result = WorkerResult::default();
                let sample_capacity = stream.len() / sample_every as usize + 1;
                result.read_latency.reserve(sample_capacity);
                result.insert_latency.reserve(sample_capacity);

                ready.wait().await;
                start.wait().await;
                for (index, operation) in stream.into_iter().enumerate() {
                    let sampled = (index as u64).is_multiple_of(sample_every);
                    let started = sampled.then(Instant::now);
                    match operation {
                        Operation::ReadAcknowledged {
                            sample,
                            distribution,
                        } => {
                            let key = acknowledged.resolve(sample, distribution);
                            let first_found = black_box(table.select(key)).is_some();
                            if !first_found {
                                result.first_read_misses += 1;
                                if black_box(table.select(key)).is_some() {
                                    result.retry_recovered += 1;
                                } else {
                                    result.final_read_errors += 1;
                                }
                            }
                            if let Some(started) = started {
                                result.read_latency.push(
                                        started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                                    );
                            }
                        }
                        Operation::Insert { key, fields } => {
                            result.inserts += 1;
                            if table.insert(row!($row, key, *fields)).is_ok() {
                                acknowledged.acknowledge(key);
                            } else {
                                result.insert_errors += 1;
                            }
                            if let Some(started) = started {
                                result.insert_latency.push(
                                        started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                                    );
                            }
                        }
                        _ => panic!("D-only campaign generated a non-D operation"),
                    }
                    result.operations += 1;
                }
                result
            }));
        }

        ready.wait().await;
        let measured = Instant::now();
        start.wait().await;
        let mut combined = WorkerResult::default();
        for handle in handles {
            let mut worker = handle.await.expect("YCSB worker panicked");
            combined.operations += worker.operations;
            combined.inserts += worker.inserts;
            combined.first_read_misses += worker.first_read_misses;
            combined.retry_recovered += worker.retry_recovered;
            combined.final_read_errors += worker.final_read_errors;
            combined.insert_errors += worker.insert_errors;
            combined.read_latency.append(&mut worker.read_latency);
            combined.insert_latency.append(&mut worker.insert_latency);
        }
        let elapsed_ns = measured.elapsed().as_nanos();
        ResultRecord {
            schema_version: 1,
            suite: "ycsb-d-index-backends",
            backend: $backend.as_str(),
            repetition: $repetition,
            campaign_pair: std::env::var("WT_CAMPAIGN_PAIR")
                .ok()
                .and_then(|value| value.parse().ok()),
            records_initial: config.records,
            operations: combined.operations,
            threads: config.threads,
            inserts: combined.inserts,
            first_read_misses: combined.first_read_misses,
            retry_recovered: combined.retry_recovered,
            final_read_errors: combined.final_read_errors,
            insert_errors: combined.insert_errors,
            elapsed_ns,
            ops_per_second: combined.operations as f64 / (elapsed_ns as f64 / 1_000_000_000.0),
            read_latency: LatencySummary::from_samples(combined.read_latency),
            insert_latency: LatencySummary::from_samples(combined.insert_latency),
            immediate_retry_on_miss: true,
            acknowledged_insert_frontier: true,
            stable_index_read_retry: cfg!(feature = "stable-index-read-retry"),
            persistence: false,
            target_arch: std::env::consts::ARCH,
            target_os: std::env::consts::OS,
        }
    }};
}

#[tokio::main]
async fn main() {
    let backend = std::env::var("WT_INDEX_BACKEND")
        .unwrap_or_else(|_| "worktables_index".to_owned())
        .parse::<Backend>()
        .unwrap_or_else(|error| {
            eprintln!("error: {error}");
            std::process::exit(2);
        });
    let config = Config::from_args().unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    if config.workload != Workload::D {
        eprintln!("error: this diagnostic campaign only accepts --workload D");
        std::process::exit(2);
    }

    for repetition in 1..=config.repetitions {
        let result = match backend {
            Backend::WorkTablesIndex => {
                run_repetition!(&config, backend, repetition, YcsbWtiWorkTable, YcsbWtiRow)
            }
            Backend::Indexset => run_repetition!(
                &config,
                backend,
                repetition,
                YcsbIndexsetWorkTable,
                YcsbIndexsetRow
            ),
            Backend::Congee => run_repetition!(
                &config,
                backend,
                repetition,
                YcsbCongeeWorkTable,
                YcsbCongeeRow
            ),
            Backend::Arctic => run_repetition!(
                &config,
                backend,
                repetition,
                YcsbArcticWorkTable,
                YcsbArcticRow
            ),
        };
        println!(
            "{}",
            serde_json::to_string(&result).expect("result must serialize")
        );
    }
}
