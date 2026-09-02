//! Non-unique secondary index, across the backends that support one.
//!
//! WorkTable's own `nonunique_arctic_vs_wti` bench gives its two arms different
//! **key types** - a 32-character hex `String` for WorkTablesIndex against the
//! `u128` that string was rendered from for Arctic - so it measures String
//! allocation and comparison as much as it measures the index. This runs three
//! arms over one key derivation so the two effects separate:
//!
//!   wti_u128 vs arctic_u128   the backend, on equal footing
//!   wti_string vs wti_u128    the key type, on one backend
//!
//! Congee is absent because `worktable_codegen` rejects it: "non-unique indexes
//! currently require `worktables_index` or `arctic`". A capability gap, not an
//! omission.

use criterion::measurement::WallTime;
use criterion::{
    BatchSize, BenchmarkGroup, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main,
};
use std::hint::black_box;
use wt_benchmarks::nonunique::SHAPES;
use wt_benchmarks::rng::Rng;

/// One arm's insert and lookup, so adding a backend is one macro call.
macro_rules! arm {
    ($module:ident, $driver:ident, $label:literal) => {
        mod $module {
            use super::*;
            use wt_benchmarks::nonunique::$module::$driver;

            pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>, fan_out: u64, keys: u64) {
                let table = $driver::populated(fan_out, keys);
                let mut rng = Rng::new(0x5eed);
                g.bench_with_input(BenchmarkId::new($label, fan_out), &fan_out, |b, _| {
                    b.iter_batched(
                        || table.next_row(rng.below(keys)),
                        |row| table.insert_row(black_box(row)),
                        BatchSize::SmallInput,
                    )
                });
            }

            pub fn select(g: &mut BenchmarkGroup<'_, WallTime>, fan_out: u64, keys: u64) {
                let table = $driver::populated(fan_out, keys);
                let mut rng = Rng::new(0x5eed);
                g.bench_with_input(BenchmarkId::new($label, fan_out), &fan_out, |b, _| {
                    b.iter(|| black_box(table.select_by_key(rng.below(keys))))
                });
            }
        }
    };
}

arm!(wti_u128, WtiU128Adjacency, "wti_u128");
arm!(arctic_u128, ArcticU128Adjacency, "arctic_u128");
arm!(wti_string, WtiStringAdjacency, "wti_string");

fn bench_insert(c: &mut Criterion) {
    let mut g = c.benchmark_group("nonunique/insert");
    for (fan_out, keys) in SHAPES {
        // One insert per iteration; the shape is what varies, not the count.
        g.throughput(Throughput::Elements(1));
        wti_u128::insert(&mut g, fan_out, keys);
        arctic_u128::insert(&mut g, fan_out, keys);
        wti_string::insert(&mut g, fan_out, keys);
    }
    g.finish();
}

fn bench_select_by_key(c: &mut Criterion) {
    let mut g = c.benchmark_group("nonunique/select_by_key");
    for (fan_out, keys) in SHAPES {
        // A lookup returns `fan_out` rows, which is what the index walks.
        g.throughput(Throughput::Elements(fan_out));
        wti_u128::select(&mut g, fan_out, keys);
        arctic_u128::select(&mut g, fan_out, keys);
        wti_string::select(&mut g, fan_out, keys);
    }
    g.finish();
}

criterion_group!(benches, bench_insert, bench_select_by_key);
criterion_main!(benches);
