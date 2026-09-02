//! The three ways to delete, against batch size and key distribution.
//!
//! A consumer evicting state has three APIs and no obvious rule for choosing
//! between them, because the answer depends on two things at once: how many
//! rows go at a time, and whether they form a span.
//!
//! - `delete(pk)` in a loop. Always applicable. Pays the full per-row cost
//!   every time, including the reclamation bookkeeping each retirement takes.
//! - `delete_many(keys)`. Any set of keys. One lock acquisition, one grace
//!   marker, one reclaim pass for the whole batch.
//! - `delete_range(a..b)`. Only a contiguous span, and it walks the primary
//!   index once instead of making the caller enumerate the keys.
//!
//! **The arms are deliberately not all comparable.** `delete_range` cannot
//! express a scattered set, so the scattered sweep runs two arms and the
//! contiguous sweep runs three. Reporting a range arm on scattered keys would
//! mean quietly changing the workload to suit the API, which is how a benchmark
//! ends up proving whatever it was pointed at.
//!
//! The expectation this exists to check is not "batching always wins". At one
//! row a batch is one row plus the cost of setting a batch up, and a loop
//! should be at least as good; the batch APIs should pull ahead as the batch
//! grows, and the crossover is the number worth knowing. An arm that loses
//! where it should lose is the bench working.
//!
//! Feature-gated on `worktable-adapter`.

/// Rows in the table before each measured delete.
///
/// Sized by the time budget, not by what would flatter an arm. Every measured
/// delete needs a table nobody has deleted from yet, so the fixture is rebuilt
/// constantly and its size sets the wall clock: at 250,000 rows one backend
/// took eight minutes, which is a benchmark nobody runs. 50,000 keeps the
/// per-key lookup cost (`O(log n)`) clearly above noise while each axis
/// finishes in a few minutes.
///
/// If an arm needs a bigger table to look good, that is a fact about the arm.
pub const TABLE_ROWS: u64 = 50_000;

/// Rows deleted per iteration. One row to a hundred, the range a consumer
/// evicting incrementally actually sits in, plus the small end where a plain
/// loop is expected to hold its own.
pub const BATCH: [u64; 5] = [1, 4, 16, 64, 100];

macro_rules! delete_backend {
    ($module:ident, $backend:ident) => {
        pub mod $module {
            use worktable::prelude::*;
            use worktable::worktable;

            use super::TABLE_ROWS;

            // The backend varies on BOTH the primary index and the unique
            // secondary one. A first version varied only the secondary, which
            // left every arm carrying two `worktables_index` indexes out of
            // three and diluted the axis to the point where the question
            // "is one backend faster to delete from" could not be answered
            // from it: the arms differed in a third of their index work.
            //
            // `generation_idx` stays on `worktables_index` in every arm because
            // it is non-unique and congee has no non-unique backend, so it
            // cannot vary. It is held constant rather than dropped: deleting
            // from a table with a non-unique index is the consumer's shape, and
            // a constant term is honest where a missing one is not.
            worktable!(
                name: Evict,
                persist: false,
                columns: {
                    id: u64 primary_key using $backend,
                    payload: u64,
                    generation: u32,
                },
                indexes: {
                    payload_idx: payload unique using $backend,
                    generation_idx: generation using worktables_index,
                },
            );

            /// A table with `TABLE_ROWS` rows, keys `0..TABLE_ROWS`.
            ///
            /// Rebuilt for every measured iteration. Reusing one would measure
            /// each batch against a table the previous batch already emptied,
            /// and the free list would diverge between arms, which quietly
            /// turns this into a benchmark of allocation reuse.
            pub fn populated() -> EvictWorkTable {
                let table = EvictWorkTable::default();
                let rows: Vec<_> = (0..TABLE_ROWS)
                    .map(|id| EvictRow {
                        id,
                        payload: 1_000_000 + id,
                        generation: (id % 8) as u32,
                    })
                    .collect();
                table.insert_many(rows).expect("fixture inserts");
                table
            }

            /// A loop of single-row deletes: the baseline, and the only arm
            /// that is always available.
            pub async fn delete_loop(table: &EvictWorkTable, keys: &[u64]) {
                for key in keys {
                    table.delete(*key).await.expect("delete");
                }
            }

            /// One batched delete over an explicit key list.
            pub fn delete_many(table: &EvictWorkTable, keys: &[u64]) {
                futures::executor::block_on(table.delete_many(keys.to_vec())).expect("delete_many");
            }

            /// One batched delete over a span, which resolves its links by
            /// walking the primary index rather than by key lookups.
            pub fn delete_range(table: &EvictWorkTable, count: u64) {
                futures::executor::block_on(
                    table.delete_range(EvictPrimaryKey::from(0u64)..EvictPrimaryKey::from(count)),
                )
                .expect("delete_range");
            }
        }
    };
}

// The secondary index backend is what varies. The primary index is the same
// ordered map in every arm, because that is what `delete_range` walks and the
// point is to compare the delete APIs rather than the primary index.
delete_backend!(wti, worktables_index);
delete_backend!(arctic, arctic);
delete_backend!(congee, congee);

/// Keys spread across the table rather than adjacent.
///
/// The realistic eviction shape: one generation's rows are interleaved with
/// every other generation's, so the set to remove is scattered. Deterministic,
/// so every arm deletes exactly the same keys.
pub fn scattered_keys(count: u64) -> Vec<u64> {
    let stride = TABLE_ROWS / count.max(1);
    (0..count).map(|i| (i * stride) % TABLE_ROWS).collect()
}

/// Keys forming a span, the only shape `delete_range` can express.
pub fn contiguous_keys(count: u64) -> Vec<u64> {
    (0..count).collect()
}
