//! Contention campaign library: the shared `Bench` table and helpers used by
//! both the quick regression runner and the paper-grade matrix runner.
//!
//! The table carries a wide set of *disjoint, non-indexed* contention columns
//! (`f0..f7`) so the field-granular locking curve can scale past two workers
//! without forcing intra-group collisions — the flaw in the original
//! `paper-bench/contention.rs`, whose `disjoint` mode split workers into just
//! two groups (even/odd) that still piled up on one of two fields.
//!
//! Every contention mode drives the *same* logical operation ("increment one
//! u64 field of the one hot row"); the modes differ only in the lock
//! discipline used to serialize it. That "same work, only the lock changes"
//! invariant is what makes the resulting C2 comparison defensible.

pub mod dynamic;
pub mod util;

use worktable::prelude::*;
use worktable::worktable;

/// Number of disjoint contention columns. Workers in the `disjoint` write-set
/// map to `field i % CONTENTION_FIELDS`, so up to this many workers proceed
/// without any field-lock collision.
pub const CONTENTION_FIELDS: usize = 8;

// The paper's benchmark table. `a` is indexed (non-unique) so indexed vs
// non-indexed update cost can be measured separately. `f0..f7` are the
// disjoint contention columns (all non-indexed). `d` keeps the table "unsized"
// (String) at a fixed length so updates stay in place.
worktable!(
    name: Bench,
    columns: {
        id: u64 primary_key autoincrement,
        a: u64,
        f0: u64,
        f1: u64,
        f2: u64,
        f3: u64,
        f4: u64,
        f5: u64,
        f6: u64,
        f7: u64,
        c: f64,
        d: String,
    },
    indexes: {
        a_idx: a,
    },
    queries: {
        update: {
            UpdA(a) by id,
            UpdF0(f0) by id,
            UpdF1(f1) by id,
            UpdF2(f2) by id,
            UpdF3(f3) by id,
            UpdF4(f4) by id,
            UpdF5(f5) by id,
            UpdF6(f6) by id,
            UpdF7(f7) by id,
        },
        in_place: {
            // Atomic RMW increment for every contention field, so every lock
            // mode performs the *identical* atomic operation and differs only
            // in lock scope. Non-atomic select+update would both lose updates
            // under same-field contention and break the same-work invariant.
            IncF0(f0) by id,
            IncF1(f1) by id,
            IncF2(f2) by id,
            IncF3(f3) by id,
            IncF4(f4) by id,
            IncF5(f5) by id,
            IncF6(f6) by id,
            IncF7(f7) by id,
            // Whole-write-set atomic increment: one closure over all eight
            // contention columns under the all-columns lock. This is
            // WorkTable's own lock machinery serializing the entire contended
            // field set atomically — the honest "whole-row" baseline, distinct
            // from an external mutex.
            IncAll(f0, f1, f2, f3, f4, f5, f6, f7) by id,
        }
    }
);

pub fn mk_row(table: &BenchWorkTable, v: u64) -> BenchRow {
    BenchRow {
        id: table.get_next_pk().into(),
        a: v,
        f0: v,
        f1: v,
        f2: v,
        f3: v,
        f4: v,
        f5: v,
        f6: v,
        f7: v,
        c: v as f64,
        d: "payloadpayload".to_string(), // fixed-length: updates stay in place
    }
}

/// Which field a worker touches under the `disjoint` write-set, given its
/// index. Distinct workers (mod `CONTENTION_FIELDS`) never share a field lock.
#[inline]
pub const fn disjoint_field(worker: usize) -> usize {
    worker % CONTENTION_FIELDS
}
