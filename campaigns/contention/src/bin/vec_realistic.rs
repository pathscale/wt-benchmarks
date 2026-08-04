//! vec_realistic — the *honest* Vec baseline: `Vec<T>` plus the boilerplate a
//! competent Rust engineer writes to get WorkTable's actual functionality,
//! WITHOUT an engine. This is the apples-to-apples floor: raw `Vec` is not a
//! peer (it is positional, borrowed, single-threaded, no secondary lookup);
//! this is.
//!
//! Functional parity with the WorkTable `Bench` table (see lib.rs):
//!   * primary storage keyed by pk               -> Vec<Option<Row>>, pk == slot
//!   * a SECONDARY index on field `a` (a_idx)     -> HashMap<u64, Vec<usize>>,
//!                                                   maintained by hand
//!   * concurrent access                          -> RwLock over the whole store
//!   * reads return an OWNED row                  -> .clone() on read (WorkTable
//!                                                   deserializes an owned row;
//!                                                   returning a borrow into the
//!                                                   container is NOT the same
//!                                                   capability)
//!
//! Operations mirror the WorkTable ablation exactly:
//!   point_read_by_pk       — resolve pk, return owned Row
//!   select_by_secondary    — resolve secondary key `a`, return owned Row(s)
//!   update_field           — update one field of a row by pk (+ index upkeep
//!                            if the indexed field changes; here we update a
//!                            NON-indexed field `b`, matching UpdF/inc paths)
//!   insert                 — push row + maintain the secondary index
//!
//! CSV to stdout: bench,engine,op,ops_per_sec
//!
//! This measures what WorkTable REPLACES. If WorkTable is Nx off THIS, that is
//! the real, defensible number — not the raw-`Vec` fantasy.

use parking_lot::RwLock;
use std::collections::HashMap;
use wt_contention_campaign::util::*;

#[derive(Clone, Debug)]
struct Row {
    id: u64,
    a: u64, // the secondary-indexed field (mirrors a_idx)
    b: u64, // a non-indexed field (mirrors f0/UpdF — updated without touching the index)
    c: f64,
    d: String,
}

fn mk(id: u64) -> Row {
    Row { id, a: id, b: id, c: id as f64, d: "payloadpayload".into() }
}

/// `Vec` + hand-maintained secondary index + lock. The boilerplate WorkTable
/// generates, written by hand.
struct VecStore {
    // pk == slot index; Option so deletes are possible (parity with logical delete)
    rows: Vec<Option<Row>>,
    // secondary index on `a`: value -> list of pks. Non-unique, like a_idx.
    by_a: HashMap<u64, Vec<u64>>,
}

impl VecStore {
    fn with_capacity(n: usize) -> Self {
        VecStore { rows: Vec::with_capacity(n), by_a: HashMap::with_capacity(n) }
    }

    fn insert(&mut self, row: Row) {
        let pk = row.id;
        // maintain the secondary index by hand
        self.by_a.entry(row.a).or_default().push(pk);
        let slot = pk as usize;
        if slot >= self.rows.len() {
            self.rows.resize(slot + 1, None);
        }
        self.rows[slot] = Some(row);
    }

    /// point read by pk -> OWNED row (the .clone() is the parity cost)
    fn select_by_pk(&self, pk: u64) -> Option<Row> {
        self.rows.get(pk as usize).and_then(|o| o.clone())
    }

    /// secondary lookup by `a` -> owned rows. This is the capability raw `Vec`
    /// simply does not have; without the index it is an O(n) scan.
    fn select_by_a(&self, a: u64) -> Vec<Row> {
        match self.by_a.get(&a) {
            Some(pks) => pks
                .iter()
                .filter_map(|&pk| self.rows.get(pk as usize).and_then(|o| o.clone()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// RANGE query over the secondary key `a`: all rows with a in [lo, hi).
    /// A HashMap secondary index CANNOT serve this without visiting every key
    /// and filtering — a HashMap is unordered. This is the capability a hash
    /// index structurally lacks and an ordered index (WorkTable's B-tree) has.
    /// Returned rows are sorted by `a` to match ordered-index semantics.
    fn range_by_a(&self, lo: u64, hi: u64) -> Vec<Row> {
        // full scan of the index keyspace — the honest cost of range-over-hash
        let mut hits: Vec<(u64, u64)> = self
            .by_a
            .iter()
            .filter(|(a, _)| **a >= lo && **a < hi)
            .flat_map(|(a, pks)| pks.iter().map(move |&pk| (*a, pk)))
            .collect();
        hits.sort_unstable_by_key(|&(a, _)| a); // ordered-index parity
        hits.into_iter()
            .filter_map(|(_, pk)| self.rows.get(pk as usize).and_then(|o| o.clone()))
            .collect()
    }

    /// update a NON-indexed field by pk (no index maintenance needed) — mirrors
    /// WorkTable's UpdF/in_place on a non-indexed column.
    fn update_b(&mut self, pk: u64, v: u64) -> bool {
        match self.rows.get_mut(pk as usize).and_then(|o| o.as_mut()) {
            Some(r) => {
                r.b = v;
                true
            }
            None => false,
        }
    }
}

fn main() {
    let rows = env_u64("ROWS", 1_000_000);
    let reps = env_u64("REPS", 5) as usize;
    println!("bench,engine,op,ops_per_sec");

    // ---- insert (build store + secondary index) ----
    let ins = median_ops_per_sec(reps, || {
        let mut store = VecStore::with_capacity(rows as usize);
        for i in 0..rows {
            store.insert(mk(i));
        }
        std::hint::black_box(&store);
        rows
    });
    println!("vec_realistic,vec_indexed,insert,{ins:.0}");

    // Build one store, wrap in RwLock, for the read/update measurements.
    let store = {
        let mut s = VecStore::with_capacity(rows as usize);
        for i in 0..rows {
            s.insert(mk(i));
        }
        RwLock::new(s)
    };

    // ---- point read by pk -> owned (parity with point_read_materialized) ----
    let rd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(42);
        let g = store.read();
        for _ in 0..rows {
            std::hint::black_box(g.select_by_pk(rng.below(rows)));
        }
        rows
    });
    println!("vec_realistic,vec_indexed,point_read_materialized,{rd:.0}");

    // ---- secondary lookup by `a` -> owned (parity with select_by_a) ----
    let sec = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(43);
        let g = store.read();
        for _ in 0..rows {
            std::hint::black_box(g.select_by_a(rng.below(rows)));
        }
        rows
    });
    println!("vec_realistic,vec_indexed,select_by_secondary,{sec:.0}");

    // ---- RANGE query over secondary key `a` (width fixed) ----
    // This is where a HashMap index structurally loses: it must scan the whole
    // keyspace. WorkTable's ordered B-tree walks only the range.
    let range_width = env_u64("RANGE_WIDTH", 100);
    let range_lookups = env_u64("RANGE_LOOKUPS", 2_000);
    let rng_q = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(44);
        let g = store.read();
        for _ in 0..range_lookups {
            let lo = rng.below(rows.saturating_sub(range_width));
            std::hint::black_box(g.range_by_a(lo, lo + range_width));
        }
        range_lookups
    });
    println!("vec_realistic,vec_indexed,range_by_secondary,{rng_q:.0}");

    // ---- field update by pk (non-indexed field) ----
    // Take the write lock PER OP (not once around the loop) so the baseline
    // pays the same acquire/release-per-operation the WorkTable field-locked
    // update pays. Hoisting the lock outside the loop would measure a raw
    // memory store, which is the apples-vs-oranges trap we are removing.
    //
    // NOTE ON FAIRNESS: even so, this remains a *sync* lock over a *native*
    // struct, whereas WorkTable's update is *async* (awaitable, cooperative)
    // over an *archived* (rkyv) row with a per-COLUMN lock. The large gap on
    // this op is the inherent cost of those capabilities (async concurrency +
    // zero-copy archived storage + field granularity), i.e. "the price of the
    // engine" — not an unfair baseline. The engine's win shows up on the
    // capability ops (secondary lookup, range), not on a single uncontended
    // field write, where a bare locked Vec is expected to lead.
    let upd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(7);
        let n = rows / 10;
        for _ in 0..n {
            let pk = rng.below(rows);
            std::hint::black_box(store.write().update_b(pk, pk));
        }
        n
    });
    println!("vec_realistic,vec_indexed,update_field,{upd:.0}");
}
