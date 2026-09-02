//! Single-operation latency, in memory and on disk.
//!
//! The axis a consumer feels as latency rather than throughput: one operation
//! at a time, no batching, no concurrency. `insert_many` and the concurrency
//! sweeps answer different questions; this one answers "what does a single
//! write cost me".
//!
//! Reported as median and p99 over individually timed operations, not as a
//! mean, because the mean hides the thing worth knowing. An insert that
//! occasionally pays a page switch is not the same as one that never does, and
//! on the persisted table the queue push has a tail of its own.
//!
//! **Caller-visible, not to-durability.** A persisted write returns once the
//! row is visible and the operation is queued; the cost of that operation
//! reaching disk is a separate measurement and `insert_many_bench` in the
//! WorkTable repository is where the two are compared.
//!
//! Feature-gated on `worktable-adapter`.

use std::time::Instant;

/// Operations timed per arm.
pub const OPS: usize = 50_000;

/// Median and p99 of a nanosecond sample.
pub fn stats(mut samples: Vec<u128>) -> (f64, f64) {
    samples.sort_unstable();
    let median = samples[samples.len() / 2] as f64;
    let p99 = samples[(samples.len() as f64 * 0.99) as usize] as f64;
    (median, p99)
}

/// Times `op` once per `i` in `0..OPS`, returning the samples.
pub fn time_each<F: FnMut(u64)>(mut op: F) -> Vec<u128> {
    let mut samples = Vec::with_capacity(OPS);
    for i in 0..OPS as u64 {
        let start = Instant::now();
        op(i);
        samples.push(start.elapsed().as_nanos());
    }
    samples
}

pub mod memory {
    use worktable::prelude::*;
    use worktable::worktable;

    worktable!(
        name: Lat,
        persist: false,
        columns: { id: u64 primary_key, payload: u64, bucket: u32 },
        indexes: { payload_idx: payload unique, bucket_idx: bucket },
    );

    pub fn table() -> LatWorkTable {
        LatWorkTable::default()
    }

    pub fn row(id: u64, payload_base: u64) -> LatRow {
        LatRow { id, payload: payload_base + id, bucket: (id % 16) as u32 }
    }
}

pub mod disk {
    use worktable::prelude::*;
    use worktable::worktable;

    worktable!(
        name: OnDisk,
        persist: true,
        columns: { id: u64 primary_key, payload: u64, bucket: u32 },
        indexes: { payload_idx: payload unique, bucket_idx: bucket },
    );

    pub async fn table(dir: &str) -> OnDiskWorkTable {
        let _ = std::fs::remove_dir_all(dir);
        let config = DiskConfig::new_with_table_name(dir, OnDiskWorkTable::name_snake_case(), OnDiskWorkTable::version());
        let engine = OnDiskPersistenceEngine::new(config).await.expect("engine");
        OnDiskWorkTable::load(engine).await.expect("load")
    }

    pub fn row(id: u64, payload_base: u64) -> OnDiskRow {
        OnDiskRow { id, payload: payload_base + id, bucket: (id % 16) as u32 }
    }
}

/// Insert throughput against concurrent writer count.
///
/// Lives here rather than in WorkTable's test suite because it is a benchmark:
/// it reports a curve, and the shape of that curve is the finding. It was an
/// `#[ignore]`d test for a while, which put a timing measurement inside a
/// correctness suite where it could only ever be run by hand anyway.
///
/// The result it exists to keep visible: throughput is flat to four writers and
/// collapses at eight. Before the `EmptyLinkRegistry::pop_max` fast path that
/// was 0.25x of single-writer throughput at eight writers; after it, 0.66x. The
/// remaining ceiling is the exclusive lock `DataPages::insert` takes on the one
/// page named by `current_page_id`, which serialises appends by construction.
pub mod scaling {
    use std::sync::Arc;
    use std::time::Instant;

    use super::memory;

    /// Rows inserted per arm, split across the writers.
    pub const ROWS: u64 = 200_000;

    /// Writer counts visited.
    pub const SWEEP: [u64; 5] = [1, 2, 4, 8, 16];

    /// Best-of-three aggregate throughput, in inserts per second.
    ///
    /// Best rather than mean: the machine is shared, so the fastest run is the
    /// one least polluted by everything else, and a mean here measures the
    /// neighbours.
    pub fn throughput(writers: u64) -> f64 {
        let mut best = f64::MAX;
        for _ in 0..3 {
            let table = Arc::new(memory::table());
            let per = ROWS / writers;
            let start = Instant::now();
            std::thread::scope(|scope| {
                for w in 0..writers {
                    let table = Arc::clone(&table);
                    scope.spawn(move || {
                        for i in (w * per)..((w + 1) * per) {
                            let _ = table.insert(memory::row(i, 1_000_000));
                        }
                    });
                }
            });
            let ns = start.elapsed().as_nanos() as f64 / ROWS as f64;
            if ns < best {
                best = ns;
            }
        }
        1e9 / best
    }
}
