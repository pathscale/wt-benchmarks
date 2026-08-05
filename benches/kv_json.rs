//! KV + embedded-JSON bench (Criterion, Throughput::Elements): the durable-jank
//! tier. Same record across engines — WorkTable stores typed columns; KV engines
//! (redb, lmdb) store one serde_json blob per key (the omicron / pict-rs
//! pattern). The story op is `update_field`: WorkTable writes ONE column; the KV
//! engines parse -> mutate -> reserialize the WHOLE document.
//!
//!   cargo bench --bench kv_json --features external-adapters

use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main};

const ROWS: u64 = 10_000;
const OPS: u64 = 10_000;
const MIN_AGE: u32 = 40;

fn keys(n: u64, bound: u64, seed: u64) -> Vec<u64> {
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

fn grp<'a>(c: &'a mut Criterion, name: &str, elems: u64) -> BenchmarkGroup<'a, WallTime> {
    let mut g = c.benchmark_group(name);
    g.throughput(Throughput::Elements(elems));
    g.measurement_time(Duration::from_secs(5));
    g
}

#[cfg(feature = "worktable-adapter")]
mod wt {
    use super::*;
    use wt_benchmarks::kv_json::worktable_engine::WtDoc;

    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        g.bench_function("worktable", |b| {
            b.iter_batched(
                WtDoc::new,
                |e| {
                    for k in 0..ROWS {
                        e.insert(k);
                    }
                    e
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_get(g: &mut BenchmarkGroup<'_, WallTime>, ks: &[u64]) {
        let e = WtDoc::load(ROWS);
        g.bench_function("worktable", |b| b.iter(|| e.point_get_checksum(ks)));
    }
    pub fn update_field(g: &mut BenchmarkGroup<'_, WallTime>, ks: &[u64]) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let e = WtDoc::load(ROWS);
        g.bench_function("worktable", |b| {
            b.iter(|| rt.block_on(e.update_balance(ks)))
        });
    }
    pub fn query_field(g: &mut BenchmarkGroup<'_, WallTime>) {
        let e = WtDoc::load(ROWS);
        g.bench_function("worktable", |b| {
            b.iter(|| e.query_active_over_age_checksum(MIN_AGE))
        });
    }
}

#[cfg(feature = "redb-adapter")]
mod redb_j {
    use super::*;
    use wt_benchmarks::kv_json::redb_engine::RedbJson;

    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        g.bench_function("redb-json", |b| {
            b.iter_batched(
                RedbJson::new,
                |e| {
                    for k in 0..ROWS {
                        e.insert(k);
                    }
                    e
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_get(g: &mut BenchmarkGroup<'_, WallTime>, ks: &[u64]) {
        let e = RedbJson::load(ROWS);
        g.bench_function("redb-json", |b| b.iter(|| e.point_get_checksum(ks)));
    }
    pub fn update_field(g: &mut BenchmarkGroup<'_, WallTime>, ks: &[u64]) {
        let e = RedbJson::load(ROWS);
        g.bench_function("redb-json", |b| b.iter(|| e.update_balance(ks)));
    }
    pub fn query_field(g: &mut BenchmarkGroup<'_, WallTime>) {
        let e = RedbJson::load(ROWS);
        g.bench_function("redb-json", |b| {
            b.iter(|| e.query_active_over_age_checksum(MIN_AGE))
        });
    }
}

#[cfg(feature = "lmdb-adapter")]
mod lmdb_j {
    use super::*;
    use wt_benchmarks::kv_json::lmdb_engine::LmdbJson;

    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        g.bench_function("lmdb-json", |b| {
            b.iter_batched(
                LmdbJson::new,
                |e| {
                    for k in 0..ROWS {
                        e.insert(k);
                    }
                    e
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_get(g: &mut BenchmarkGroup<'_, WallTime>, ks: &[u64]) {
        let e = LmdbJson::load(ROWS);
        g.bench_function("lmdb-json", |b| b.iter(|| e.point_get_checksum(ks)));
    }
    pub fn update_field(g: &mut BenchmarkGroup<'_, WallTime>, ks: &[u64]) {
        let e = LmdbJson::load(ROWS);
        g.bench_function("lmdb-json", |b| b.iter(|| e.update_balance(ks)));
    }
    pub fn query_field(g: &mut BenchmarkGroup<'_, WallTime>) {
        let e = LmdbJson::load(ROWS);
        g.bench_function("lmdb-json", |b| {
            b.iter(|| e.query_active_over_age_checksum(MIN_AGE))
        });
    }
}

fn bench_insert(c: &mut Criterion) {
    let mut g = grp(c, "kv_json/insert", ROWS);
    #[cfg(feature = "worktable-adapter")]
    wt::insert(&mut g);
    #[cfg(feature = "redb-adapter")]
    redb_j::insert(&mut g);
    #[cfg(feature = "lmdb-adapter")]
    lmdb_j::insert(&mut g);
}

fn bench_point_get(c: &mut Criterion) {
    let ks = keys(OPS, ROWS, 42);
    let mut g = grp(c, "kv_json/point_get", OPS);
    #[cfg(feature = "worktable-adapter")]
    wt::point_get(&mut g, &ks);
    #[cfg(feature = "redb-adapter")]
    redb_j::point_get(&mut g, &ks);
    #[cfg(feature = "lmdb-adapter")]
    lmdb_j::point_get(&mut g, &ks);
}

fn bench_update_field(c: &mut Criterion) {
    let ks = keys(OPS, ROWS, 7);
    let mut g = grp(c, "kv_json/update_field", OPS);
    #[cfg(feature = "worktable-adapter")]
    wt::update_field(&mut g, &ks);
    #[cfg(feature = "redb-adapter")]
    redb_j::update_field(&mut g, &ks);
    #[cfg(feature = "lmdb-adapter")]
    lmdb_j::update_field(&mut g, &ks);
}

fn bench_query_field(c: &mut Criterion) {
    let mut g = grp(c, "kv_json/query_field", ROWS);
    #[cfg(feature = "worktable-adapter")]
    wt::query_field(&mut g);
    #[cfg(feature = "redb-adapter")]
    redb_j::query_field(&mut g);
    #[cfg(feature = "lmdb-adapter")]
    lmdb_j::query_field(&mut g);
}

criterion_group!(
    benches,
    bench_insert,
    bench_point_get,
    bench_update_field,
    bench_query_field
);
criterion_main!(benches);
