//! slotmap_baseline — the OSS baseline the ecosystem ACTUALLY uses.
//!
//! `slotmap` has ~90M downloads (cf. `slab` at ~805M): it is the arena crate
//! Rust reaches for to store records with stable handles. Like every arena in
//! that family (slab, generational-arena, id-vec), it gives O(1) access BY
//! HANDLE and maintains NO secondary index. A user who needs to find a record
//! by a non-handle field must hand-roll a linear scan — exactly the code
//! WorkTable's generated index replaces.
//!
//! We benchmark against `slotmap` specifically because it is un-cherry-pickable:
//! it is not a toy we wrote or a niche crate nobody uses; it is a load-bearing
//! dependency across the Rust ecosystem. The reverse-dependency search that led
//! here also found that NO widely-used crate exists whose purpose is
//! "scan-by-field" — because that pattern lives inline in applications, not in
//! libraries (which is itself evidence for the paper's premise).
//!
//! Operations mirror the WorkTable ablation and the other baselines:
//!   point_read_by_handle   — O(1), slotmap's home turf (owned via clone)
//!   select_by_secondary    — find by a non-handle field: HAND-ROLLED SCAN, O(n)
//!   insert                 — insert into the slotmap (+ we keep a pk->key map
//!                            so "by pk" access is possible at all, since a
//!                            slotmap key is not the application's pk)
//!   update_field           — update one field by handle
//!
//! CSV to stdout: bench,engine,op,ops_per_sec

use slotmap::{DefaultKey, SlotMap};
use std::collections::HashMap;
use wt_contention_campaign::util::*;

#[derive(Clone, Debug)]
struct Row {
    id: u64,
    a: u64, // the field looked up by scan (WorkTable indexes this as a_idx)
    b: u64, // a non-indexed field, updated in place
    c: f64,
    d: String,
}

fn mk(id: u64) -> Row {
    Row { id, a: id, b: id, c: id as f64, d: "payloadpayload".into() }
}

fn main() {
    let rows = env_u64("ROWS", 1_000_000);
    let reps = env_u64("REPS", 5) as usize;
    // secondary scans are O(n); cap the count so the run finishes and report
    // the honest per-lookup rate (each still scans the whole slotmap).
    let scan_lookups = env_u64("SCAN_LOOKUPS", 2_000);
    println!("bench,engine,op,ops_per_sec");

    // ---- insert (+ pk->key side map, since slotmap keys are not the app pk) ----
    let ins = median_ops_per_sec(reps, || {
        let mut sm: SlotMap<DefaultKey, Row> = SlotMap::with_capacity(rows as usize);
        let mut by_pk: HashMap<u64, DefaultKey> = HashMap::with_capacity(rows as usize);
        for i in 0..rows {
            let k = sm.insert(mk(i));
            by_pk.insert(i, k);
        }
        std::hint::black_box((&sm, &by_pk));
        rows
    });
    println!("slotmap_baseline,slotmap,insert,{ins:.0}");

    // build once for reads
    let mut sm: SlotMap<DefaultKey, Row> = SlotMap::with_capacity(rows as usize);
    let mut by_pk: HashMap<u64, DefaultKey> = HashMap::with_capacity(rows as usize);
    for i in 0..rows {
        by_pk.insert(i, sm.insert(mk(i)));
    }

    // ---- point read by pk: pk->key map, then O(1) slotmap access, owned ----
    let rd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(42);
        for _ in 0..rows {
            let pk = rng.below(rows);
            let row = by_pk.get(&pk).and_then(|k| sm.get(*k)).cloned();
            std::hint::black_box(row);
        }
        rows
    });
    println!("slotmap_baseline,slotmap,point_read_materialized,{rd:.0}");

    // ---- secondary lookup by field `a`: HAND-ROLLED LINEAR SCAN (O(n)) ----
    // slotmap has no secondary index; this is what its users must write.
    let sec = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(43);
        for _ in 0..scan_lookups {
            let key = rng.below(rows);
            let hit = sm.values().find(|r| r.a == key).cloned();
            std::hint::black_box(hit);
        }
        scan_lookups
    });
    println!("slotmap_baseline,slotmap,select_by_secondary_scan,{sec:.0}");

    // ---- field update by pk (O(1) by handle) ----
    let upd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(7);
        let n = rows / 10;
        for _ in 0..n {
            let pk = rng.below(rows);
            if let Some(r) = by_pk.get(&pk).and_then(|k| sm.get_mut(*k)) {
                r.b = pk;
            }
        }
        n
    });
    println!("slotmap_baseline,slotmap,update_field,{upd:.0}");
}
