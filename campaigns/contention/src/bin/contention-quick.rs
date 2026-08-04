//! contention-quick — casual regression runner for lock-granularity contention.
//!
//! Minimal by design: CSV to stdout, a handful of thread counts, a few seconds
//! total. Use it to eyeball whether a change moved the field-granular vs
//! coarse-lock story. It is NOT the paper artifact — for versioned JSONL,
//! correctness counters, the publication axis, and per-cell fresh processes,
//! use `contention-paper` driven by scripts/run-contention-matrix.sh.
//!
//! Modes (one hot row, N tasks incrementing it for DURATION_SECS):
//!   disjoint  — worker i updates field (i % CONTENTION_FIELDS); distinct
//!               workers never collide on a field lock. Fixes the original
//!               even/odd split that piled workers onto one of two fields.
//!   overlap   — every worker updates the SAME field (f0); write sets are
//!               identical, so the field locks fully serialize.
//!   wholerow  — every worker takes the all-columns lock (UpdAll) on the same
//!               row: WorkTable's own machinery serializing the whole field
//!               set (the honest coarse baseline).
//!   mutex     — like disjoint, but the identical call is wrapped in one
//!               external tokio::Mutex (reference line for "naive coarse lock").
//!   inplace   — every worker runs the in_place closure increment on f0.
//!
//! CSV to stdout: bench,mode,tasks,ops_per_sec

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use wt_contention_campaign::util::*;
use wt_contention_campaign::*;

async fn update_disjoint_field(table: &BenchWorkTable, worker: usize, pk: u64, n: u64) {
    // Dispatch to the field this worker owns. All arms are the same shape of
    // work (one u64 field update); only the target field (hence lock) differs.
    match disjoint_field(worker) {
        0 => table.update_upd_f_0(UpdF0Query { f0: n }, pk).await.unwrap(),
        1 => table.update_upd_f_1(UpdF1Query { f1: n }, pk).await.unwrap(),
        2 => table.update_upd_f_2(UpdF2Query { f2: n }, pk).await.unwrap(),
        3 => table.update_upd_f_3(UpdF3Query { f3: n }, pk).await.unwrap(),
        4 => table.update_upd_f_4(UpdF4Query { f4: n }, pk).await.unwrap(),
        5 => table.update_upd_f_5(UpdF5Query { f5: n }, pk).await.unwrap(),
        6 => table.update_upd_f_6(UpdF6Query { f6: n }, pk).await.unwrap(),
        _ => table.update_upd_f_7(UpdF7Query { f7: n }, pk).await.unwrap(),
    }
}

async fn run(mode: &'static str, tasks: usize) -> f64 {
    let table = Arc::new(BenchWorkTable::default());
    let pk = table.insert(mk_row(&table, 1)).unwrap();
    let pk_val: u64 = pk.into();

    let stop = Arc::new(AtomicBool::new(false));
    let total = Arc::new(AtomicU64::new(0));
    let big_lock = Arc::new(tokio::sync::Mutex::new(()));

    let mut handles = Vec::new();
    for i in 0..tasks {
        let table = table.clone();
        let stop = stop.clone();
        let total = total.clone();
        let big_lock = big_lock.clone();
        handles.push(tokio::spawn(async move {
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                match mode {
                    "disjoint" => update_disjoint_field(&table, i, pk_val, n).await,
                    "overlap" => {
                        table.update_upd_f_0(UpdF0Query { f0: n }, pk_val).await.unwrap()
                    }
                    "wholerow" => {
                        // all-columns lock via the multi-field in_place closure
                        table
                            .update_inc_all_in_place(
                                |(f0, f1, f2, f3, f4, f5, f6, f7)| {
                                    *f0 += 1; *f1 += 1; *f2 += 1; *f3 += 1;
                                    *f4 += 1; *f5 += 1; *f6 += 1; *f7 += 1;
                                },
                                pk_val,
                            )
                            .await
                            .unwrap()
                    }
                    "mutex" => {
                        let _g = big_lock.lock().await;
                        update_disjoint_field(&table, i, pk_val, n).await;
                    }
                    "inplace" => {
                        table.update_inc_f_0_in_place(|f0| *f0 += 1, pk_val).await.unwrap()
                    }
                    _ => unreachable!(),
                }
                n += 1;
            }
            total.fetch_add(n, Ordering::Relaxed);
        }));
    }

    let dur = env_secs("DURATION_SECS", 3);
    let t0 = Instant::now();
    tokio::time::sleep(dur).await;
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        h.await.unwrap();
    }
    total.load(Ordering::Relaxed) as f64 / t0.elapsed().as_secs_f64()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    println!("bench,mode,tasks,ops_per_sec");
    let task_counts = [1usize, 2, 4, 8, 16];
    for mode in ["disjoint", "overlap", "wholerow", "mutex", "inplace"] {
        for &t in &task_counts {
            let _ = run(mode, t).await; // warmup
            let ops = run(mode, t).await;
            println!("contention,{mode},{t},{ops:.0}");
        }
    }
}
