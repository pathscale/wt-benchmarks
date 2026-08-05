//! Specialization comparison (paper Table 1 / C3): the specialized `worktable!`
//! build vs the naive dynamic baseline (`DynTable`), per operation, via Criterion.
//!
//! DELIBERATELY UNEVEN: WorkTable is not expected to beat the naive dynamic
//! baseline on raw point ops — see the note in `dynamic.rs`. A bare Vec+BTreeMap
//! skips real storage/indexing, so it looks fast on trivial point work; the
//! comparison exposes what that shortcut costs. This is the "apple" of the
//! paper's fruit basket (naive dynamic); the "lemon" (SQLite) and "grape"
//! (DuckDB) live in the KV bench. WorkTable ("orange") is contrasted against all
//! three, and is meant to win some and, by design, not others.
//!
//! Each op is a `BenchmarkGroup` with `Throughput::Elements(ROWS)` so Criterion
//! reports elements/sec directly (no homebrew ops/sec math) alongside its
//! statistics — median, MAD, and confidence intervals with outlier handling.
//! Table setup is done outside the timed closure via `iter_batched`, so only
//! the measured operation is timed.

use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{
    BatchSize, BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main,
};

use wt_contention_campaign::dynamic::{DynTable, Value, mk_dyn_row};
use wt_contention_campaign::{AblationTable, ArcticBench, CongeeBench, WtiBench};

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

fn populated_specialized<T: AblationTable>() -> T {
    let table = T::default();
    for v in 0..ROWS {
        table.insert_value(v);
    }
    table
}

fn bench_specialized_insert<T: AblationTable>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    label: &str,
) {
    group.bench_function(label, |b| {
        b.iter_batched(
            T::default,
            |table| {
                for v in 0..ROWS {
                    table.insert_value(v);
                }
                table
            },
            BatchSize::SmallInput,
        )
    });
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

    bench_specialized_insert::<WtiBench>(&mut group, "specialized");
    bench_specialized_insert::<CongeeBench>(&mut group, "specialized-congee");
    bench_specialized_insert::<ArcticBench>(&mut group, "specialized-arctic");

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

    macro_rules! specialized_read {
        ($driver:ty, $label:literal) => {{
            let spec = populated_specialized::<$driver>();
            group.bench_function($label, |b| {
                b.iter(|| {
                    let mut sum = 0u64;
                    for k in &keys {
                        if let Some(value) = spec.point_read(*k) {
                            sum = sum.wrapping_add(value);
                        }
                    }
                    sum
                })
            });
        }};
    }
    specialized_read!(WtiBench, "specialized");
    specialized_read!(CongeeBench, "specialized-congee");
    specialized_read!(ArcticBench, "specialized-arctic");

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

    macro_rules! specialized_update {
        ($driver:ty, $label:literal) => {{
            let rt = tokio_rt();
            let spec = populated_specialized::<$driver>();
            group.bench_function($label, |b| {
                b.iter(|| {
                    rt.block_on(async {
                        for k in &keys {
                            spec.update_a(*k, *k).await;
                        }
                    })
                })
            });
        }};
    }
    specialized_update!(WtiBench, "specialized");
    specialized_update!(CongeeBench, "specialized-congee");
    specialized_update!(ArcticBench, "specialized-arctic");

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
