//! Single-operation latency: insert, upsert, delete, select.
//!
//!   cargo bench --bench op_latency
//!
//! Not a Criterion bench. Criterion reports a distribution over batches of
//! repeated calls, which is the right shape for throughput and the wrong one
//! for "what does one call cost": the batching is exactly what a latency
//! question is trying to see through. Each operation here is timed
//! individually and reported as a median and a p99.
//!
//! Both storage modes, because they are different engines: an in-memory write
//! stops at the row and its indexes, a persisted write also queues an
//! operation. The persisted figures are caller-visible, not to-durability.

use wt_benchmarks::op_latency::{self, OPS, disk, memory, stats, time_each};

fn line(mode: &str, op: &str, samples: Vec<u128>) {
    let (median, p99) = stats(samples);
    println!("{mode:<8} {op:<8} {median:>10.0} ns {p99:>12.0} ns");
}

fn main() {
    println!("single-operation latency, {OPS} operations per arm");
    println!("{:<8} {:<8} {:>13} {:>15}", "mode", "op", "median", "p99");
    println!("{}", "-".repeat(48));

    // ---- in memory ----
    let table = memory::table();
    line("memory", "insert", time_each(|i| {
        table.insert(memory::row(i, 1_000_000)).expect("insert");
    }));
    line("memory", "select", time_each(|i| {
        std::hint::black_box(table.select(i));
    }));
    // A fresh payload each time: writing back the value already stored lets the
    // unique index short circuit, and the arm then measures a rejected write.
    line("memory", "upsert", time_each(|i| {
        futures::executor::block_on(table.upsert(memory::row(i, 9_000_000))).expect("upsert");
    }));
    line("memory", "delete", time_each(|i| {
        futures::executor::block_on(table.delete(i)).expect("delete");
    }));

    // ---- persisted ----
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime");
    let dir = "/tmp/wt-bench-op-latency";
    let table = runtime.block_on(disk::table(dir));

    line("disk", "insert", time_each(|i| {
        table.insert(disk::row(i, 1_000_000)).expect("insert");
    }));
    line("disk", "select", time_each(|i| {
        std::hint::black_box(table.select(i));
    }));
    line("disk", "upsert", time_each(|i| {
        runtime.block_on(table.upsert(disk::row(i, 9_000_000))).expect("upsert");
    }));
    line("disk", "delete", time_each(|i| {
        runtime.block_on(table.delete(i)).expect("delete");
    }));

    drop(table);
    let _ = std::fs::remove_dir_all(dir);
    let _ = op_latency::OPS;
}
