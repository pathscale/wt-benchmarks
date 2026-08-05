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

/// Number of disjoint contention columns. Workers in the `disjoint` write-set
/// map to `field i % CONTENTION_FIELDS`, so up to this many workers proceed
/// without any field-lock collision.
pub const CONTENTION_FIELDS: usize = 8;

// The paper's benchmark table. `a` is indexed (non-unique) so indexed vs
// non-indexed update cost can be measured separately. `f0..f7` are the
// disjoint contention columns (all non-indexed). `d` keeps the table "unsized"
// (String) at a fixed length so updates stay in place.
pub trait AblationTable: Default {
    fn insert_value(&self, value: u64);
    fn point_read(&self, key: u64) -> Option<u64>;
    fn update_a(&self, key: u64, value: u64) -> impl std::future::Future<Output = ()> + Send;
}

macro_rules! bench_backend {
    ($module:ident, $driver:ident, $using:ident) => {
        mod $module {
            use worktable::prelude::*;
            use worktable::worktable;

            worktable!(
                name: Bench,
                persist: false,
                columns: {
                    id: u64 primary_key autoincrement using $using,
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
                        IncF0(f0) by id,
                        IncF1(f1) by id,
                        IncF2(f2) by id,
                        IncF3(f3) by id,
                        IncF4(f4) by id,
                        IncF5(f5) by id,
                        IncF6(f6) by id,
                        IncF7(f7) by id,
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
                    d: "payloadpayload".to_string(),
                }
            }

            pub struct $driver(BenchWorkTable);

            impl Default for $driver {
                fn default() -> Self {
                    Self(BenchWorkTable::default())
                }
            }

            impl crate::AblationTable for $driver {
                fn insert_value(&self, value: u64) {
                    self.0.insert(mk_row(&self.0, value)).unwrap();
                }

                fn point_read(&self, key: u64) -> Option<u64> {
                    self.0.select(key).map(|row| row.a)
                }

                async fn update_a(&self, key: u64, value: u64) {
                    let _ = self.0.update_upd_a(UpdAQuery { a: value }, key).await;
                }
            }
        }
    };
}

bench_backend!(wti_backend, WtiBench, worktables_index);
bench_backend!(congee_backend, CongeeBench, congee);
bench_backend!(arctic_backend, ArcticBench, arctic);

pub use arctic_backend::ArcticBench;
pub use congee_backend::CongeeBench;
pub use wti_backend::*;

/// Which field a worker touches under the `disjoint` write-set, given its
/// index. Distinct workers (mod `CONTENTION_FIELDS`) never share a field lock.
#[inline]
pub const fn disjoint_field(worker: usize) -> usize {
    worker % CONTENTION_FIELDS
}
