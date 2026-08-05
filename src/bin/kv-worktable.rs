//! WorkTable KV adapter. Runs the fixed KV workload (insert / point_read /
//! overwrite / range_scan / delete) against WorkTable's in-memory path, and can
//! exercise any of the three primary-index backends selectable through the
//! `using` keyword — WorkTablesIndex (default), Congee, and Arctic — via
//! `--index-backend`. All three share one driver surface (see `kv_table`), so
//! the timed loop below is written once and dispatched by backend.

use std::hint::black_box;
use std::time::Instant;

use wt_benchmarks::kv::{DurabilityMode, KvConfig, TransactionScope, emit};
use wt_benchmarks::kv_table::{ArcticKv, CongeeKv, IndexBackend, WorktableKv};

/// Runs every repetition of the KV workload against one backend driver `$Kv`,
/// emitting a row per operation. The engine label carries the backend so
/// results from different `--index-backend` runs stay distinguishable.
macro_rules! run_backend {
    ($Kv:ty, $config:expr, $engine:expr, $point_keys:expr, $scan_starts:expr) => {{
        let config = $config;
        let engine = $engine;
        let point_keys = $point_keys;
        let scan_starts = $scan_starts;
        for repetition in 1..=config.repetitions {
            let kv = <$Kv>::new(config.payload_bytes);

            let started = Instant::now();
            for key in 0..config.rows {
                kv.insert(key);
            }
            emit(
                config,
                engine,
                "insert",
                repetition,
                config.rows,
                "not-applicable",
                started.elapsed().as_nanos(),
                kv.count(),
            );

            let started = Instant::now();
            let checksum = black_box(kv.point_read_checksum(point_keys));
            emit(
                config,
                engine,
                "point_read",
                repetition,
                config.operations,
                "materialized-owned-row",
                started.elapsed().as_nanos(),
                checksum,
            );

            let started = Instant::now();
            kv.overwrite(point_keys).await;
            emit(
                config,
                engine,
                "overwrite",
                repetition,
                config.operations,
                "not-applicable",
                started.elapsed().as_nanos(),
                kv.count(),
            );

            let started = Instant::now();
            let checksum = black_box(kv.range_scan_checksum(scan_starts, config.scan_length));
            emit(
                config,
                engine,
                "range_scan",
                repetition,
                config.scan_operations,
                "materialized-owned-row",
                started.elapsed().as_nanos(),
                checksum,
            );

            let started = Instant::now();
            let deleted = kv.delete(point_keys).await;
            emit(
                config,
                engine,
                "delete_random",
                repetition,
                config.operations,
                "not-applicable",
                started.elapsed().as_nanos(),
                deleted,
            );
        }
    }};
}

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

    let backend = IndexBackend::parse(&config.index_backend).expect("validated index backend");
    // The engine label folds in the backend so ladder rows from separate
    // --index-backend runs never collide; the default backend keeps the bare
    // "worktable" label for continuity with existing baselines.
    let engine = match backend {
        IndexBackend::WorktablesIndex => "worktable".to_string(),
        other => format!("worktable-{}", other.label()),
    };

    let point_keys = config.point_keys();
    let scan_starts = config.scan_starts();

    match backend {
        IndexBackend::WorktablesIndex => {
            run_backend!(WorktableKv, &config, &engine, &point_keys, &scan_starts)
        }
        IndexBackend::Congee => {
            run_backend!(CongeeKv, &config, &engine, &point_keys, &scan_starts)
        }
        IndexBackend::Arctic => {
            run_backend!(ArcticKv, &config, &engine, &point_keys, &scan_starts)
        }
    }
}
