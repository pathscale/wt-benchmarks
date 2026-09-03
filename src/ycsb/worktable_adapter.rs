use std::collections::BTreeMap;
use std::future::Future;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Barrier;
use worktable::prelude::*;
use worktable::worktable;

use crate::config::Config;
use crate::result::{LatencySummary, RunResult};
use crate::ycsb::generator::{
    AcknowledgedKeyspace, FIELD_COUNT, Operation, OperationKind, generate_streams, make_fields,
};

pub use crate::kv_table::IndexBackend;

trait YcsbTable: Default + Send + Sync + 'static {
    fn insert(&self, key: u64, fields: [String; FIELD_COUNT]) -> bool;
    fn select(&self, key: u64) -> bool;
    fn scan(&self, start: u64, length: u64) -> bool;
    fn update(&self, key: u64, field: u8, value: String) -> impl Future<Output = bool> + Send;
}

macro_rules! ycsb_backend_table {
    ($module:ident, $using:ident) => {
        mod $module {
            use super::*;

            worktable!(
                name: Ycsb,
                persist: false,
                columns: {
                    id: u64 primary_key using $using,
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
                },
                queries: {
                    update: {
                        Field0(field0) by id,
                        Field1(field1) by id,
                        Field2(field2) by id,
                        Field3(field3) by id,
                        Field4(field4) by id,
                        Field5(field5) by id,
                        Field6(field6) by id,
                        Field7(field7) by id,
                        Field8(field8) by id,
                        Field9(field9) by id,
                    }
                }
            );

            pub(super) struct Driver(YcsbWorkTable);

            impl Default for Driver {
                fn default() -> Self {
                    Self(YcsbWorkTable::default())
                }
            }

            impl YcsbTable for Driver {
                fn insert(&self, key: u64, fields: [String; FIELD_COUNT]) -> bool {
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
                    ] = fields;
                    futures::executor::block_on(self.0
                        .insert(YcsbRow {
                            id: key,
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
                        }))
                        .is_ok()
                }

                fn select(&self, key: u64) -> bool {
                    black_box(self.0.select(key)).is_some()
                }

                fn scan(&self, start: u64, length: u64) -> bool {
                    let end = start.saturating_add(length.saturating_sub(1));
                    match self.0.select_by_pk_range(start..=end).execute() {
                        Ok(rows) => {
                            black_box(rows);
                            true
                        }
                        Err(_) => false,
                    }
                }

                async fn update(&self, key: u64, field: u8, value: String) -> bool {
                    match field {
                        0 => self.0.update_field_0(Field0Query { field0: value }, key).await.is_ok(),
                        1 => self.0.update_field_1(Field1Query { field1: value }, key).await.is_ok(),
                        2 => self.0.update_field_2(Field2Query { field2: value }, key).await.is_ok(),
                        3 => self.0.update_field_3(Field3Query { field3: value }, key).await.is_ok(),
                        4 => self.0.update_field_4(Field4Query { field4: value }, key).await.is_ok(),
                        5 => self.0.update_field_5(Field5Query { field5: value }, key).await.is_ok(),
                        6 => self.0.update_field_6(Field6Query { field6: value }, key).await.is_ok(),
                        7 => self.0.update_field_7(Field7Query { field7: value }, key).await.is_ok(),
                        8 => self.0.update_field_8(Field8Query { field8: value }, key).await.is_ok(),
                        9 => self.0.update_field_9(Field9Query { field9: value }, key).await.is_ok(),
                        _ => false,
                    }
                }
            }
        }
    };
}

ycsb_backend_table!(wti_backend, worktables_index);
ycsb_backend_table!(congee_backend, congee);
ycsb_backend_table!(arctic_backend, arctic);

#[derive(Default)]
struct WorkerResult {
    completed: u64,
    errors: u64,
    counts: [u64; 5],
    error_counts: [u64; 5],
    latency: [Vec<u64>; 5],
}

pub async fn run_repetition(config: &Config, repetition: usize) -> RunResult {
    run_repetition_with_backend(config, repetition, IndexBackend::WorktablesIndex).await
}

pub async fn run_repetition_with_backend(
    config: &Config,
    repetition: usize,
    backend: IndexBackend,
) -> RunResult {
    match backend {
        IndexBackend::WorktablesIndex => {
            run_backend::<wti_backend::Driver>(config, repetition).await
        }
        IndexBackend::Congee => run_backend::<congee_backend::Driver>(config, repetition).await,
        IndexBackend::Arctic => run_backend::<arctic_backend::Driver>(config, repetition).await,
    }
}

async fn run_backend<T: YcsbTable>(config: &Config, repetition: usize) -> RunResult {
    let table = Arc::new(T::default());
    let load_started = Instant::now();
    for key in 0..config.records {
        let fields = make_fields(key ^ config.seed, config.field_bytes);
        assert!(
            table.insert(key, fields),
            "initial YCSB keys must be unique"
        );
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
        let table = Arc::clone(&table);
        let acknowledged = acknowledged.as_ref().map(Arc::clone);
        let ready = Arc::clone(&ready);
        let start = Arc::clone(&start);
        let sample_every = config.sample_every;
        handles.push(tokio::spawn(async move {
            ready.wait().await;
            start.wait().await;
            run_worker(table, stream, sample_every, acknowledged).await
        }));
    }

    ready.wait().await;
    let measured_started = Instant::now();
    start.wait().await;

    let mut combined = WorkerResult::default();
    for handle in handles {
        let worker = handle.await.expect("YCSB worker panicked");
        combined.completed += worker.completed;
        combined.errors += worker.errors;
        for kind in OperationKind::ALL {
            combined.counts[kind as usize] += worker.counts[kind as usize];
            combined.error_counts[kind as usize] += worker.error_counts[kind as usize];
            combined.latency[kind as usize].extend(worker.latency[kind as usize].iter().copied());
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

    RunResult::new(
        config,
        repetition,
        &distribution.to_string(),
        combined.completed,
        combined.errors,
        load_elapsed_ns,
        elapsed_ns,
        operation_counts,
        operation_errors,
        latency,
    )
}

async fn run_worker<T: YcsbTable>(
    table: Arc<T>,
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
        let success = execute(&*table, operation, acknowledged.as_deref()).await;
        if let Some(started) = started {
            let elapsed = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            result.latency[kind as usize].push(elapsed);
        }
        result.counts[kind as usize] += 1;
        result.completed += u64::from(success);
        result.errors += u64::from(!success);
        result.error_counts[kind as usize] += u64::from(!success);
    }
    result
}

async fn execute<T: YcsbTable>(
    table: &T,
    operation: Operation,
    acknowledged: Option<&AcknowledgedKeyspace>,
) -> bool {
    match operation {
        Operation::Read { key } => table.select(key),
        Operation::ReadAcknowledged {
            sample,
            distribution,
        } => {
            let key = acknowledged
                .expect("acknowledged read requires Workload D state")
                .resolve(sample, distribution);
            table.select(key)
        }
        Operation::Update { key, field, value } => table.update(key, field, value).await,
        Operation::Insert { key, fields } => {
            let inserted = table.insert(key, *fields);
            if inserted && let Some(acknowledged) = acknowledged {
                acknowledged.acknowledge(key);
            }
            inserted
        }
        Operation::Scan { start, length } => table.scan(start, length),
        Operation::ReadModifyWrite { key, field, value } => {
            if !table.select(key) {
                false
            } else {
                table.update(key, field, value).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ycsb::Workload;

    #[tokio::test]
    async fn workload_d_completes_with_an_acknowledged_single_worker_stream() {
        let config = Config {
            workload: Workload::D,
            records: 1_000,
            operations: 20_000,
            threads: 1,
            repetitions: 1,
            sample_every: 256,
            seed: 42,
            field_bytes: 16,
            scan_max: 10,
            zipf_theta: 0.99,
            distribution_override: None,
        };
        let result = run_repetition(&config, 1).await;
        assert_eq!(result.errors, 0);
        assert_eq!(result.operations_completed, config.operations);
    }

    #[cfg(feature = "versioned-row-publication")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "root suite tracks released WorkTable; run concurrent D against PR 187 in the dedicated campaign"]
    async fn concurrent_workload_d_has_no_committed_key_misses() {
        let config = Config {
            workload: Workload::D,
            records: 10_000,
            operations: 100_000,
            threads: 8,
            repetitions: 1,
            sample_every: 256,
            seed: 42,
            field_bytes: 16,
            scan_max: 10,
            zipf_theta: 0.99,
            distribution_override: None,
        };
        let result = run_repetition(&config, 1).await;
        assert_eq!(result.errors, 0);
        assert_eq!(result.operations_completed, config.operations);
    }
}
