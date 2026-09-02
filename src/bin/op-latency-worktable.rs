//! Single-operation latency and insert scaling for WorkTable.
//!
//! ```sh
//! cargo run --release --bin op-latency-worktable
//! cargo run --release --bin op-latency-worktable -- --sweep 1,2,4,8,16,32 --memory-only
//! ```
//!
//! Emits one JSON object per arm, like the other suites, so a campaign can
//! collect runs without parsing prose.
use std::time::Instant;

use wt_benchmarks::op_latency::{LatencyConfig, Mode, disk, emit_latency, emit_scaling, memory};

/// Times `op` once per index, returning per-call samples and total elapsed.
fn time_each(operations: u64, mut op: impl FnMut(u64)) -> (Vec<u64>, u128) {
    let mut samples = Vec::with_capacity(operations as usize);
    let overall = Instant::now();
    for i in 0..operations {
        let start = Instant::now();
        op(i);
        samples.push(start.elapsed().as_nanos() as u64);
    }
    (samples, overall.elapsed().as_nanos())
}

fn main() {
    let config = match LatencyConfig::from_args() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}\nrun with --help for usage");
            std::process::exit(2);
        }
    };

    if cfg!(debug_assertions) {
        eprintln!(
            "warning: debug build. Per-operation overhead swamps the contention, and the \
             scaling sweep inverts: eight writers report faster than one. Use --release."
        );
    }

    // ---- latency, in memory ----
    let table = memory::table();
    let (samples, elapsed) = time_each(config.operations, |i| {
        table.insert(memory::row(i, 1_000_000)).expect("insert");
    });
    emit_latency(Mode::Memory, "insert", samples, elapsed);

    let (samples, elapsed) = time_each(config.operations, |i| {
        std::hint::black_box(table.select(i));
    });
    emit_latency(Mode::Memory, "select", samples, elapsed);

    // A fresh payload each time. Writing back the value already stored lets the
    // unique index short circuit, and the arm then measures a rejected write.
    let (samples, elapsed) = time_each(config.operations, |i| {
        futures::executor::block_on(table.upsert(memory::row(i, 9_000_000))).expect("upsert");
    });
    emit_latency(Mode::Memory, "upsert", samples, elapsed);

    let (samples, elapsed) = time_each(config.operations, |i| {
        futures::executor::block_on(table.delete(i)).expect("delete");
    });
    emit_latency(Mode::Memory, "delete", samples, elapsed);
    drop(table);

    // ---- latency, persisted ----
    if !config.memory_only {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("runtime");
        let table = runtime.block_on(disk::table(&config.disk_dir));

        let (samples, elapsed) = time_each(config.operations, |i| {
            table.insert(disk::row(i, 1_000_000)).expect("insert");
        });
        emit_latency(Mode::Disk, "insert", samples, elapsed);

        let (samples, elapsed) = time_each(config.operations, |i| {
            std::hint::black_box(table.select(i));
        });
        emit_latency(Mode::Disk, "select", samples, elapsed);

        let (samples, elapsed) = time_each(config.operations, |i| {
            runtime
                .block_on(table.upsert(disk::row(i, 9_000_000)))
                .expect("upsert");
        });
        emit_latency(Mode::Disk, "upsert", samples, elapsed);

        let (samples, elapsed) = time_each(config.operations, |i| {
            runtime.block_on(table.delete(i)).expect("delete");
        });
        emit_latency(Mode::Disk, "delete", samples, elapsed);

        drop(table);
        let _ = std::fs::remove_dir_all(&config.disk_dir);
    }

    // ---- insert scaling ----
    //
    // Best of `repetitions` rather than a mean: this machine is shared, so the
    // fastest run is the one least polluted by everything else and a mean
    // measures the neighbours.
    let mut single_writer_rate = 0.0f64;
    for &writers in &config.sweep {
        let mut best_ns = u128::MAX;
        for _ in 0..config.repetitions {
            let table = std::sync::Arc::new(memory::table());
            let per = config.scaling_rows / writers;
            let start = Instant::now();
            std::thread::scope(|scope| {
                for w in 0..writers {
                    let table = std::sync::Arc::clone(&table);
                    scope.spawn(move || {
                        for i in (w * per)..((w + 1) * per) {
                            let _ = table.insert(memory::row(i, 1_000_000));
                        }
                    });
                }
            });
            best_ns = best_ns.min(start.elapsed().as_nanos());
        }
        let rate = config.scaling_rows as f64 / (best_ns as f64 / 1_000_000_000.0);
        if writers == *config.sweep.first().expect("sweep is not empty") {
            single_writer_rate = rate;
        }
        emit_scaling(
            writers,
            config.scaling_rows,
            config.repetitions,
            best_ns,
            single_writer_rate,
        );
    }
}
