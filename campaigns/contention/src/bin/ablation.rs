//! Specialization ablation (paper Table 1): WorkTable vs the dynamic twin.
//!
//! Single-threaded per-op cost; ROWS rows; REPS repetitions, median reported.
//! CSV to stdout: bench,engine,op,ops_per_sec

use wt_contention_campaign::dynamic::*;
use wt_contention_campaign::util::*;
use wt_contention_campaign::*;
// SelectQueryExecutor provides `.execute()` on the select builder.
use worktable::prelude::SelectQueryExecutor;

fn main() {
    let rows = env_u64("ROWS", 1_000_000);
    let reps = env_u64("REPS", 5) as usize;
    println!("bench,engine,op,ops_per_sec");

    // ---------------- specialized ----------------
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    // insert
    let table = BenchWorkTable::default();
    let ins = median_ops_per_sec(reps, || {
        // fresh table per rep to avoid unbounded growth distorting later reps
        let t = BenchWorkTable::default();
        for v in 0..rows {
            t.insert(mk_row(&t, v)).unwrap();
        }
        rows
    });
    println!("ablation,specialized,insert,{ins:.0}");

    // populate once for read/update reps
    for v in 0..rows {
        table.insert(mk_row(&table, v)).unwrap();
    }

    // random point read
    let rd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(42);
        let n = rows;
        for _ in 0..n {
            let pk = rng.below(rows);
            std::hint::black_box(table.select(pk));
        }
        n
    });
    println!("ablation,specialized,point_read,{rd:.0}");

    // secondary lookup by the indexed field `a` (a_idx). This is WorkTable's
    // generated B-tree index path — the direct counterpart to the janky
    // `Vec::iter().find(|s| s.field == key)` O(n) scan in vec_janky. The most
    // honest apples-to-apples cell: index vs no-index for "find by non-pk field".
    let sec = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(43);
        let n = rows;
        for _ in 0..n {
            let key = rng.below(rows);
            let _ = std::hint::black_box(table.select_by_a(key).execute());
        }
        n
    });
    println!("ablation,specialized,select_by_secondary,{sec:.0}");

    // RANGE query over the indexed field `a` (a_idx). WorkTable's ordered
    // B-tree walks only the matching range; the counterpart HashMap index in
    // vec_realistic must scan its whole keyspace. Same fixed range width.
    let range_width = env_u64("RANGE_WIDTH", 100);
    let range_lookups = env_u64("RANGE_LOOKUPS", 2_000);
    let rng_q = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(44);
        for _ in 0..range_lookups {
            let lo = rng.below(rows.saturating_sub(range_width));
            let _ = std::hint::black_box(table.select_by_a_range(lo..(lo + range_width)).execute());
        }
        range_lookups
    });
    println!("ablation,specialized,range_by_secondary,{rng_q:.0}");

    // non-indexed field update (UpdB)
    let upd = {
        let table = &table;
        median_ops_per_sec(reps, || {
            let mut rng = Rng::new(7);
            let n = rows / 10;
            rt.block_on(async {
                for _ in 0..n {
                    let pk = rng.below(rows);
                    table.update_upd_f_0(UpdF0Query { f0: pk }, pk).await.unwrap();
                }
            });
            n
        })
    };
    println!("ablation,specialized,update_field,{upd:.0}");

    // indexed field update (UpdA) — includes index maintenance
    let updi = {
        let table = &table;
        median_ops_per_sec(reps, || {
            let mut rng = Rng::new(9);
            let n = rows / 10;
            rt.block_on(async {
                for _ in 0..n {
                    let pk = rng.below(rows);
                    table.update_upd_a(UpdAQuery { a: pk }, pk).await.unwrap();
                }
            });
            n
        })
    };
    println!("ablation,specialized,update_indexed,{updi:.0}");

    // in-place RMW
    let inp = {
        let table = &table;
        median_ops_per_sec(reps, || {
            let mut rng = Rng::new(11);
            let n = rows / 10;
            rt.block_on(async {
                for _ in 0..n {
                    let pk = rng.below(rows);
                    table.update_inc_f_0_in_place(|b| *b += 1, pk).await.unwrap();
                }
            });
            n
        })
    };
    println!("ablation,specialized,inplace_rmw,{inp:.0}");

    // ---------------- dynamic twin ----------------
    let ins = median_ops_per_sec(reps, || {
        let t = DynTable::new();
        for _ in 0..rows {
            let pk = t.get_next_pk();
            t.insert(mk_dyn_row(pk, pk));
        }
        rows
    });
    println!("ablation,dynamic,insert,{ins:.0}");

    let dt = DynTable::new();
    for _ in 0..rows {
        let pk = dt.get_next_pk();
        dt.insert(mk_dyn_row(pk, pk));
    }

    let rd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(42);
        for _ in 0..rows {
            let pk = rng.below(rows);
            std::hint::black_box(dt.select(pk));
        }
        rows
    });
    println!("ablation,dynamic,point_read,{rd:.0}");

    let upd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(7);
        let n = rows / 10;
        for _ in 0..n {
            let pk = rng.below(rows);
            dt.update_field(pk, "b", Value::U64(pk)).unwrap();
        }
        n
    });
    println!("ablation,dynamic,update_field,{upd:.0}");
}
