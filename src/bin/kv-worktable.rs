use std::hint::black_box;
use std::time::Instant;

use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::kv::{
    DurabilityMode, KvConfig, TransactionScope, emit, text_checksum, text_value,
};

worktable!(
    name: KvBench,
    columns: {
        id: u64 primary_key,
        payload: String,
    },
    queries: {
        update: {
            Payload(payload) by id,
        }
    }
);

#[tokio::main]
async fn main() {
    let config = KvConfig::from_args("worktable").unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    if config.durability != DurabilityMode::Memory {
        eprintln!(
            "error: this adapter measures the in-memory WorkTable path; use --durability memory"
        );
        std::process::exit(2);
    }
    if config.transaction_scope != TransactionScope::PerOperation {
        eprintln!(
            "error: WorkTable has no batch transaction; use --transaction-scope per-operation"
        );
        std::process::exit(2);
    }

    let point_keys = config.point_keys();
    let scan_starts = config.scan_starts();
    for repetition in 1..=config.repetitions {
        let table = KvBenchWorkTable::default();

        let started = Instant::now();
        for key in 0..config.rows {
            table
                .insert(KvBenchRow {
                    id: key,
                    payload: text_value(key, config.payload_bytes),
                })
                .expect("fresh keys must insert");
        }
        emit(
            &config,
            "worktable",
            "insert",
            repetition,
            config.rows,
            "not-applicable",
            started.elapsed().as_nanos(),
            table.count() as u64,
        );

        let started = Instant::now();
        let checksum = point_keys.iter().fold(0_u64, |sum, key| {
            let row = black_box(table.select(*key)).expect("loaded key");
            sum.wrapping_add(text_checksum(row.id, &row.payload))
        });
        emit(
            &config,
            "worktable",
            "point_read",
            repetition,
            config.operations,
            "materialized-owned-row",
            started.elapsed().as_nanos(),
            checksum,
        );

        let started = Instant::now();
        for key in &point_keys {
            table
                .update_payload(
                    PayloadQuery {
                        payload: text_value(key.wrapping_mul(17), config.payload_bytes),
                    },
                    *key,
                )
                .await
                .expect("loaded key must update");
        }
        emit(
            &config,
            "worktable",
            "overwrite",
            repetition,
            config.operations,
            "not-applicable",
            started.elapsed().as_nanos(),
            table.count() as u64,
        );

        let started = Instant::now();
        let mut checksum = 0_u64;
        for start in &scan_starts {
            let end = start + config.scan_length;
            let rows = table
                .select_by_pk_range(*start..end)
                .execute()
                .expect("range scan");
            for row in rows {
                checksum = checksum.wrapping_add(text_checksum(row.id, &row.payload));
            }
        }
        emit(
            &config,
            "worktable",
            "range_scan",
            repetition,
            config.scan_operations,
            "materialized-owned-row",
            started.elapsed().as_nanos(),
            checksum,
        );

        let started = Instant::now();
        let mut deleted = 0_u64;
        for key in &point_keys {
            if table.delete(*key).await.is_ok() {
                deleted += 1;
            }
        }
        emit(
            &config,
            "worktable",
            "delete_random",
            repetition,
            config.operations,
            "not-applicable",
            started.elapsed().as_nanos(),
            deleted,
        );
    }
}
