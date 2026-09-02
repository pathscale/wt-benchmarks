//! A mixed read/write workload across threads, per index backend.
//!
//! Adapted from `WorkTablesIndex/benches/concurrent.rs`, which compared the raw
//! concurrent set against `scc::TreeIndex` and `crossbeam_skiplist::SkipSet`.
//! That comparison answers "is our data structure competitive"; it cannot
//! answer "which index backend should this table use", because arctic and
//! congee are not standalone sets with the same API. They only exist as
//! WorkTable index backends, so the question has to be asked at table level,
//! which is also where a consumer chooses.
//!
//! What is kept from the original is the method, which is the part that was
//! worth keeping:
//!
//! - **Fixed write ratios** rather than one blended workload. A backend can win
//!   read-heavy and lose write-heavy, and a single ratio hides that.
//! - **Disjoint key ranges per thread.** Threads writing the same keys measure
//!   the lock map rather than the index, and the result then depends mostly on
//!   how many threads collide.
//! - **Operations generated up front.** RNG in the timed section measures the
//!   RNG, and `rand` is not free.
//!
//! Sized to a few minutes per axis. The original ran 40 threads times 100,000
//! operations, which is four million operations per sample and far past any
//! budget worth having on a shared machine.

/// Rows present before the mixed phase starts.
pub const TABLE_ROWS: u64 = 20_000;

/// Operations each thread performs.
pub const OPS_PER_THREAD: u64 = 4_000;

/// Reader and writer threads. Deliberately more readers than writers: that is
/// the shape the reclamation work was aimed at, and the shape where a scheme
/// that stalls under continuous readers shows up.
///
/// Oversubscribed relative to the machine on purpose. Threads that never
/// contend measure a single-threaded path several times over; the point here is
/// the contention, so the thread count is set above the core count to force
/// readers and writers to interleave rather than to fit.
pub const READERS: usize = 24;
pub const WRITERS: usize = 8;

/// Share of a reader thread's operations that are writes. Zero is a pure
/// reader; the writer threads always write.
pub const WRITE_RATIOS: [u32; 3] = [0, 10, 50];

/// One thread's script, generated before timing starts.
#[derive(Clone, Copy)]
pub enum Op {
    Read(u64),
    Write(u64),
}

/// Deterministic per-thread operation scripts over disjoint key ranges.
///
/// Deterministic on purpose: every backend runs the identical script, so a
/// difference between them is the backend rather than the draw. The generator
/// is a plain LCG rather than `rand`, to keep the fixture reproducible without
/// pinning a dependency's RNG behaviour across versions.
pub fn scripts(write_ratio: u32) -> Vec<Vec<Op>> {
    let threads = READERS + WRITERS;
    let span = TABLE_ROWS / threads as u64;
    (0..threads)
        .map(|thread| {
            let start = thread as u64 * span;
            let mut state = 0x2545_F491_4F6C_DD1D ^ (thread as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            (0..OPS_PER_THREAD)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let key = start + (state >> 33) % span.max(1);
                    let writes = thread < WRITERS || (state >> 11) % 100 < write_ratio as u64;
                    if writes { Op::Write(key) } else { Op::Read(key) }
                })
                .collect()
        })
        .collect()
}

macro_rules! concurrent_backend {
    ($module:ident, $backend:ident) => {
        pub mod $module {
            use std::sync::Arc;
            use std::sync::atomic::{AtomicU64, Ordering};

            use worktable::prelude::*;
            use worktable::worktable;

            use super::{Op, TABLE_ROWS};

            worktable!(
                name: Mix,
                persist: false,
                columns: {
                    id: u64 primary_key,
                    payload: u64,
                    bucket: u32,
                },
                indexes: {
                    payload_idx: payload unique using $backend,
                    bucket_idx: bucket using worktables_index,
                },
            );

            pub fn populated() -> Arc<MixWorkTable> {
                let table = MixWorkTable::default();
                let rows: Vec<_> = (0..TABLE_ROWS)
                    .map(|id| MixRow { id, payload: 1_000_000 + id, bucket: (id % 16) as u32 })
                    .collect();
                table.insert_many(rows).expect("fixture inserts");
                Arc::new(table)
            }

            /// Run every thread's script at once and return when all are done.
            ///
            /// A write is an upsert of an existing key rather than an insert of
            /// a new one: that keeps the table size fixed across the run, so a
            /// long measurement does not quietly turn into a benchmark of table
            /// growth, and it keeps every backend holding the same number of
            /// entries.
            pub fn run(table: &Arc<MixWorkTable>, scripts: &[Vec<Op>]) -> (u64, u64) {
                let reads_hit = Arc::new(AtomicU64::new(0));
                let writes_ok = Arc::new(AtomicU64::new(0));
                std::thread::scope(|scope| {
                    for script in scripts {
                        let table = Arc::clone(table);
                        let reads_hit = Arc::clone(&reads_hit);
                        let writes_ok = Arc::clone(&writes_ok);
                        scope.spawn(move || {
                            let (mut hit, mut wrote) = (0u64, 0u64);
                            for op in script {
                                match op {
                                    Op::Read(key) => {
                                        if table.select(*key).is_some() {
                                            hit += 1;
                                        }
                                    }
                                    Op::Write(key) => {
                                        // The payload carries the operation
                                        // counter, so a write is a real index
                                        // mutation: writing the value already
                                        // stored lets a unique index short
                                        // circuit, and the workload then
                                        // measures a rejected write. That is
                                        // how a first version reported 50%
                                        // writes as *faster* than 0%.
                                        let row = MixRow {
                                            id: *key,
                                            payload: 1_000_000 + *key + wrote * TABLE_ROWS,
                                            bucket: (*key % 16) as u32,
                                        };
                                        // Blocked on, not dropped. `upsert` is
                                        // async, so a bare `let _ = ..` builds a
                                        // future and discards it: the write
                                        // never happens and the arm measures
                                        // nothing. That is exactly what a first
                                        // version did, and it showed up as 50%
                                        // writes running *faster* than 0%.
                                        if futures::executor::block_on(table.upsert(row)).is_ok() {
                                            wrote += 1;
                                        }
                                    }
                                }
                            }
                            reads_hit.fetch_add(hit, Ordering::Relaxed);
                            writes_ok.fetch_add(wrote, Ordering::Relaxed);
                        });
                    }
                });
                (reads_hit.load(Ordering::Relaxed), writes_ok.load(Ordering::Relaxed))
            }
        }
    };
}

concurrent_backend!(wti, worktables_index);
concurrent_backend!(arctic, arctic);
concurrent_backend!(congee, congee);
