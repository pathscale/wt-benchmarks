//! Contention on Criterion, two ports over the same concurrent cell run.
//!
//! The measured run lives in the proven `contention-paper` binary (one cell per
//! process: lock mode x threads x hot_rows). Rather than duplicate ~130 lines of
//! worker/lock logic, each Criterion iteration invokes that binary and reads its
//! JSONL, so the bench measures exactly what the paper campaign measures.
//!
//!   1. THROUGHPUT port — `iter_custom` returns the cell's own measured elapsed
//!      window; with `Throughput::Elements(ops)` Criterion reports ops/sec + CIs.
//!   2. CORRECTNESS port — asserts `lost_updates == 0` per cell (a lock-protocol
//!      regression shows up as a hard failure, not a silent number).
//!
//! Run: `cargo bench --bench contention` (build the release bin first).

use std::process::Command;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use serde_json::Value;

const THREADS: &[usize] = &[1, 4, 8];
const OPS_PER_THREAD: u64 = 100_000;
const MODES: &[&str] = &["field_granular", "single_mutex"];

fn bin_path() -> String {
    // Same-profile sibling of this bench binary.
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // deps/
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("contention-paper");
    p.to_string_lossy().into_owned()
}

/// Run one cell, returning (elapsed_ns, ops_completed, lost_updates).
fn run_cell(bin: &str, mode: &str, threads: usize) -> (u64, u64, u64) {
    let out = Command::new(bin)
        .args([
            "--mode", mode,
            "--threads", &threads.to_string(),
            "--ops-per-thread", &OPS_PER_THREAD.to_string(),
            "--warmup-ops", "20000",
            "--repetition", "1",
        ])
        .output()
        .expect("run contention-paper");
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .expect("cell JSONL")
        .to_string();
    let v: Value = serde_json::from_str(&line).unwrap();
    (
        v["elapsed_ns"].as_u64().unwrap(),
        v["operations_completed"].as_u64().unwrap(),
        v["lost_updates"].as_u64().unwrap(),
    )
}

fn throughput(c: &mut Criterion) {
    let bin = bin_path();
    let mut group = c.benchmark_group("contention/throughput");
    group.sample_size(10);
    for mode in MODES {
        for &t in THREADS {
            group.throughput(Throughput::Elements(OPS_PER_THREAD * t as u64));
            group.bench_function(format!("{mode}/t{t}"), |b| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let (elapsed_ns, _ops, lost) = run_cell(&bin, mode, t);
                        assert_eq!(lost, 0, "correctness: {mode} t{t} lost updates");
                        total += Duration::from_nanos(elapsed_ns);
                    }
                    total
                })
            });
        }
    }
    group.finish();
}

criterion_group!(benches, throughput);
criterion_main!(benches);
