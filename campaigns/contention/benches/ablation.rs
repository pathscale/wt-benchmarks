//! Specialization ablation via Divan (paper Table 1 / C3): the specialized
//! `worktable!` build vs the dynamic twin, per operation, with statistical
//! rigor (Divan reports median + confidence intervals, handles warmup and
//! outliers). The custom-harness `ablation` binary remains for CSV ladder
//! output; this is the credible statistical view of the same four ops.
//!
//! Each op benchmarks specialized and dynamic side by side. Setup (building and
//! populating the table) is done in `with_inputs` so only the measured op is
//! timed.

use divan::{black_box, Bencher};
use wt_contention_campaign::dynamic::{mk_dyn_row, DynTable, Value};
use wt_contention_campaign::{mk_row, BenchWorkTable, UpdAQuery};

/// Rows preloaded before the measured read/update ops.
const ROWS: u64 = 10_000;

fn main() {
    divan::main();
}

// ---------------------------------------------------------------- insert

#[divan::bench(name = "insert/specialized")]
fn insert_specialized(bencher: Bencher) {
    bencher
        .with_inputs(BenchWorkTable::default)
        .bench_values(|table| {
            for v in 0..ROWS {
                table.insert(mk_row(&table, v)).unwrap();
            }
            black_box(&table);
        });
}

#[divan::bench(name = "insert/dynamic")]
fn insert_dynamic(bencher: Bencher) {
    bencher.with_inputs(DynTable::new).bench_values(|table| {
        for v in 0..ROWS {
            let pk = table.get_next_pk();
            table.insert(mk_dyn_row(pk, v));
        }
        black_box(&table);
    });
}

// ------------------------------------------------------------- point_read

fn seeded_keys(n: u64) -> Vec<u64> {
    // Deterministic pseudo-random key order over [0, ROWS).
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

#[divan::bench(name = "point_read/specialized")]
fn point_read_specialized(bencher: Bencher) {
    let table = BenchWorkTable::default();
    for v in 0..ROWS {
        table.insert(mk_row(&table, v)).unwrap();
    }
    let keys = seeded_keys(ROWS);
    bencher.bench_local(|| {
        let mut sum = 0u64;
        for k in &keys {
            if let Some(row) = table.select(*k) {
                sum = sum.wrapping_add(row.a);
            }
        }
        black_box(sum)
    });
}

#[divan::bench(name = "point_read/dynamic")]
fn point_read_dynamic(bencher: Bencher) {
    let table = DynTable::new();
    for v in 0..ROWS {
        let pk = table.get_next_pk();
        table.insert(mk_dyn_row(pk, v));
    }
    let keys = seeded_keys(ROWS);
    bencher.bench_local(|| {
        let mut sum = 0u64;
        for k in &keys {
            if let Some(row) = table.select(*k) {
                if let Value::U64(a) = &row[1] {
                    sum = sum.wrapping_add(*a);
                }
            }
        }
        black_box(sum)
    });
}

// ------------------------------------------------------------ update_field

#[divan::bench(name = "update_field/specialized")]
fn update_field_specialized(bencher: Bencher) {
    let table = BenchWorkTable::default();
    for v in 0..ROWS {
        table.insert(mk_row(&table, v)).unwrap();
    }
    let keys = seeded_keys(ROWS);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    bencher.bench_local(|| {
        runtime.block_on(async {
            for k in &keys {
                let _ = table
                    .update_upd_a(UpdAQuery { a: black_box(*k) }, *k)
                    .await;
            }
        });
    });
}

#[divan::bench(name = "update_field/dynamic")]
fn update_field_dynamic(bencher: Bencher) {
    let table = DynTable::new();
    for v in 0..ROWS {
        let pk = table.get_next_pk();
        table.insert(mk_dyn_row(pk, v));
    }
    let keys = seeded_keys(ROWS);
    bencher.bench_local(|| {
        for k in &keys {
            table.update_field(*k, "a", Value::U64(black_box(*k)));
        }
    });
}
