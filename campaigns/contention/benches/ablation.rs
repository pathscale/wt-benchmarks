//! Specialization ablation (paper Table 1 / C3): the specialized `worktable!`
//! build vs the dynamic twin, per operation, measured with Criterion.
//!
//! Each op is a `BenchmarkGroup` with `Throughput::Elements(ROWS)` so Criterion
//! reports elements/sec directly (no homebrew ops/sec math) alongside its
//! statistics — median, MAD, and confidence intervals with outlier handling.
//! Table setup is done outside the timed closure via `iter_batched`, so only
//! the measured operation is timed.

use std::time::Duration;

use criterion::{
    criterion_group, criterion_main, BatchSize, BenchmarkGroup, Criterion, Throughput,
};
use criterion::measurement::WallTime;

use wt_contention_campaign::dynamic::{mk_dyn_row, DynTable, Value};
use wt_contention_campaign::{mk_row, BenchWorkTable, UpdAQuery};

/// Rows preloaded before each measured read/update op, and the element count
/// used for throughput.
const ROWS: u64 = 10_000;

fn tokio_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Deterministic pseudo-random key order over [0, ROWS).
fn seeded_keys(n: u64) -> Vec<u64> {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % ROWS
        })
        .collect()
}

fn populated_specialized() -> BenchWorkTable {
    let table = BenchWorkTable::default();
    for v in 0..ROWS {
        table.insert(mk_row(&table, v)).unwrap();
    }
    table
}

fn populated_dynamic() -> DynTable {
    let table = DynTable::new();
    for v in 0..ROWS {
        let pk = table.get_next_pk();
        table.insert(mk_dyn_row(pk, v));
    }
    table
}

fn common(group: &mut BenchmarkGroup<'_, WallTime>) {
    // ROWS operations per iteration -> Criterion reports elements/sec.
    group.throughput(Throughput::Elements(ROWS));
    group.measurement_time(Duration::from_secs(5));
}

fn bench_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert");
    common(&mut group);

    group.bench_function("specialized", |b| {
        b.iter_batched(
            BenchWorkTable::default,
            |table| {
                for v in 0..ROWS {
                    table.insert(mk_row(&table, v)).unwrap();
                }
                table
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("dynamic", |b| {
        b.iter_batched(
            DynTable::new,
            |table| {
                for v in 0..ROWS {
                    let pk = table.get_next_pk();
                    table.insert(mk_dyn_row(pk, v));
                }
                table
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_point_read(c: &mut Criterion) {
    let keys = seeded_keys(ROWS);
    let mut group = c.benchmark_group("point_read");
    common(&mut group);

    let spec = populated_specialized();
    group.bench_function("specialized", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for k in &keys {
                if let Some(row) = spec.select(*k) {
                    sum = sum.wrapping_add(row.a);
                }
            }
            sum
        })
    });

    let dynamic = populated_dynamic();
    group.bench_function("dynamic", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for k in &keys {
                if let Some(row) = dynamic.select(*k) {
                    if let Value::U64(a) = &row[1] {
                        sum = sum.wrapping_add(*a);
                    }
                }
            }
            sum
        })
    });

    group.finish();
}

fn bench_update_field(c: &mut Criterion) {
    let keys = seeded_keys(ROWS);
    let mut group = c.benchmark_group("update_field");
    common(&mut group);

    let rt = tokio_rt();
    let spec = populated_specialized();
    group.bench_function("specialized", |b| {
        b.iter(|| {
            rt.block_on(async {
                for k in &keys {
                    let _ = spec.update_upd_a(UpdAQuery { a: *k }, *k).await;
                }
            })
        })
    });

    let dynamic = populated_dynamic();
    group.bench_function("dynamic", |b| {
        b.iter(|| {
            for k in &keys {
                dynamic.update_field(*k, "a", Value::U64(*k));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_insert, bench_point_read, bench_update_field);
criterion_main!(benches);
