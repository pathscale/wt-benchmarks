//! KV throughput bench (Criterion, Throughput::Elements) — the adopted-twin KV
//! workload across engines, so Criterion reports elements/sec with confidence
//! intervals directly. This is the THROUGHPUT port; a separate latency port
//! records p50/p95/p99 for the concurrent workloads. Single-threaded point KV
//! only needs the throughput view.
//!
//! Engines beyond WorkTable are behind their adapter features:
//!   cargo bench --bench kv --features external-adapters

use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkGroup, Criterion, Throughput};
use criterion::measurement::WallTime;

use wt_benchmarks::kv::{text_checksum, text_value};

const ROWS: u64 = 10_000;
const OPS: u64 = 10_000;
const SCAN_OPS: u64 = 200;
const SCAN_LEN: u64 = 100;
const PAYLOAD: usize = 64;

fn seeded_keys(n: u64, bound: u64, seed: u64) -> Vec<u64> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % bound
        })
        .collect()
}

fn point_keys() -> Vec<u64> {
    seeded_keys(OPS, ROWS, 42)
}
fn scan_starts() -> Vec<u64> {
    seeded_keys(SCAN_OPS, ROWS - SCAN_LEN + 1, 0xa5a5)
}

fn grp<'a>(c: &'a mut Criterion, name: &str, elems: u64) -> BenchmarkGroup<'a, WallTime> {
    let mut g = c.benchmark_group(name);
    g.throughput(Throughput::Elements(elems));
    g.measurement_time(Duration::from_secs(5));
    g
}

// ------------------------------------------------------------- WorkTable
#[cfg(feature = "worktable-adapter")]
mod worktable_engine {
    use super::*;
    use wt_benchmarks::kv_table::WorktableKv;

    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        g.bench_function("worktable", |b| {
            b.iter_batched(
                || WorktableKv::new(PAYLOAD),
                |kv| {
                    for k in 0..ROWS {
                        kv.insert(k);
                    }
                    kv
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_read(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let kv = WorktableKv::load(PAYLOAD, ROWS);
        g.bench_function("worktable", |b| b.iter(|| kv.point_read_checksum(keys)));
    }
    pub fn overwrite(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let kv = WorktableKv::load(PAYLOAD, ROWS);
        g.bench_function("worktable", |b| b.iter(|| rt.block_on(kv.overwrite(keys))));
    }
    pub fn range_scan(g: &mut BenchmarkGroup<'_, WallTime>, starts: &[u64]) {
        let kv = WorktableKv::load(PAYLOAD, ROWS);
        g.bench_function("worktable", |b| b.iter(|| kv.range_scan_checksum(starts, SCAN_LEN)));
    }
}

// --------------------------------------------------------------- SQLite
#[cfg(feature = "sqlite-adapter")]
mod sqlite_engine {
    use super::*;
    use rusqlite::{params, Connection};

    fn load() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;
             CREATE TABLE kv (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);",
        )
        .unwrap();
        {
            let mut s = c.prepare("INSERT INTO kv VALUES (?,?)").unwrap();
            for k in 0..ROWS {
                s.execute(params![k as i64, text_value(k, PAYLOAD)]).unwrap();
            }
        }
        c
    }
    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        g.bench_function("sqlite", |b| {
            b.iter_batched(
                || {
                    let c = Connection::open_in_memory().unwrap();
                    c.execute_batch(
                        "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;
                         CREATE TABLE kv (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);",
                    )
                    .unwrap();
                    c
                },
                |c| {
                    let mut s = c.prepare("INSERT INTO kv VALUES (?,?)").unwrap();
                    for k in 0..ROWS {
                        s.execute(params![k as i64, text_value(k, PAYLOAD)]).unwrap();
                    }
                    drop(s);
                    c
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_read(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let c = load();
        g.bench_function("sqlite", |b| {
            b.iter(|| {
                let mut s = c.prepare("SELECT payload FROM kv WHERE id=?").unwrap();
                let mut sum = 0u64;
                for k in keys {
                    let p: String = s.query_row(params![*k as i64], |r| r.get(0)).unwrap();
                    sum = sum.wrapping_add(text_checksum(*k, &p));
                }
                sum
            })
        });
    }
}

fn bench_insert(c: &mut Criterion) {
    let mut g = grp(c, "kv/insert", ROWS);
    #[cfg(feature = "worktable-adapter")]
    worktable_engine::insert(&mut g);
    #[cfg(feature = "sqlite-adapter")]
    sqlite_engine::insert(&mut g);
    g.finish();
}

fn bench_point_read(c: &mut Criterion) {
    let keys = point_keys();
    let mut g = grp(c, "kv/point_read", OPS);
    #[cfg(feature = "worktable-adapter")]
    worktable_engine::point_read(&mut g, &keys);
    #[cfg(feature = "sqlite-adapter")]
    sqlite_engine::point_read(&mut g, &keys);
    g.finish();
}

fn bench_overwrite(c: &mut Criterion) {
    let keys = point_keys();
    let mut g = grp(c, "kv/overwrite", OPS);
    #[cfg(feature = "worktable-adapter")]
    worktable_engine::overwrite(&mut g, &keys);
    g.finish();
}

fn bench_range_scan(c: &mut Criterion) {
    let starts = scan_starts();
    let mut g = grp(c, "kv/range_scan", SCAN_OPS * SCAN_LEN);
    #[cfg(feature = "worktable-adapter")]
    worktable_engine::range_scan(&mut g, &starts);
    g.finish();
}

criterion_group!(benches, bench_insert, bench_point_read, bench_overwrite, bench_range_scan);
criterion_main!(benches);
