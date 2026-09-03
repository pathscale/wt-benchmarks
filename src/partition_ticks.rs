//! Routing to a partition and reading from it, under concurrent readers and
//! live partition churn.
//!
//! # Why this workload exists
//!
//! WorkTable's partitioned tables exist for one shape: a stream of events that
//! each name a partition, arriving faster than anything else in the system.
//! Market ticks are the case the design was measured against, and the routing
//! call sits on the hot path of every one of them.
//!
//! The repository's own `partition_routing` benchmark measures that call in
//! isolation, and isolation hides the two things that decide what it costs.
//!
//! **It does no table work.** A routing call that returns a borrow is 3 ns and
//! a `select` on the table it found is an order of magnitude more, so a
//! difference that looks like 15% against the routing call alone is under 2%
//! against the tick that contains it. A number without its denominator is not
//! a number a release decision can rest on.
//!
//! **Nothing is ever retired.** Reclamation only costs anything when there is
//! something to reclaim. A benchmark whose partition set never changes measures
//! the pin and never the grace period, so every reclamation scheme looks alike
//! and the one property they differ on — whether a retirement can complete
//! while readers keep arriving — is invisible. That property is the whole
//! reason this crate does not use `seize`, and no benchmark here exercised it.
//!
//! So this workload reads through the routing call, does real work with what it
//! finds, and runs a writer that removes and recreates partitions while the
//! readers are going.
//!
//! # The three routing strategies
//!
//! [`Routing::Arc`] hands back a counted handle: two atomic read-modify-writes
//! per lookup, one in and one out, on a line shared with every other reader of
//! that partition.
//!
//! [`Routing::Ref`] hands back a borrow guarded by a pin. No shared atomic, but
//! the pin ends in a `SeqCst` fence and the slot load right after it has to
//! wait on that fence.
//!
//! [`Routing::Pinned`] pins once and looks up many times, so a batch of ticks
//! pays one fence between them. It is the reason the batch size is a parameter
//! here: the question worth answering is not whether pinning once is cheaper
//! than pinning every time, which is arithmetic, but how long a batch has to be
//! before it matters, and what holding the pin for that long costs the writer
//! trying to reclaim.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use worktable::prelude::*;
use worktable::worktable;

use crate::rng::Rng;

worktable! {
    name: Tick,
    persist: false,
    partition_by: symbol_id: u16,
    columns: {
        id: u64 primary_key,
        price: f64,
        qty: u64,
    },
}

/// How a reader reaches the table it is going to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Routing {
    /// `partition`: a counted handle, two atomic RMWs per lookup.
    Arc,
    /// `partition_ref`: a pinned borrow, one pin per lookup.
    Ref,
    /// `pinned` + `get`: one pin per batch.
    ///
    /// Only available from worktable 1.0.0-beta.16. Behind the
    /// `partition-pinned` feature so this crate still builds against the
    /// published release, which has no such method.
    Pinned,
}

impl Routing {
    /// The strategies this build can measure.
    pub fn available() -> &'static [Routing] {
        #[cfg(feature = "partition-pinned")]
        {
            &[Routing::Arc, Routing::Ref, Routing::Pinned]
        }
        #[cfg(not(feature = "partition-pinned"))]
        {
            &[Routing::Arc, Routing::Ref]
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Routing::Arc => "partition_arc",
            Routing::Ref => "partition_ref",
            Routing::Pinned => "pinned_get",
        }
    }
}

/// Which symbols the readers ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spread {
    /// Every reader routes to one symbol, so `Arc`'s strong count is a single
    /// contended line. This is not artificial: one instrument carrying most of
    /// the volume is the normal case, not the exception.
    Hot,
    /// Readers spread across symbols, so the counts do not collide and the
    /// difference between the strategies should mostly disappear. It is the
    /// control for `Hot`.
    Spread,
}

impl Spread {
    pub fn label(self) -> &'static str {
        match self {
            Spread::Hot => "hot_symbol",
            Spread::Spread => "spread",
        }
    }
}

/// One run's shape.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub partitions: u16,
    pub rows_per_partition: u64,
    pub readers: usize,
    pub routing: Routing,
    pub spread: Spread,
    /// Ticks a reader handles between pins under [`Routing::Pinned`]. Ignored
    /// by the other two, which pin per tick by construction.
    pub batch: usize,
    /// Whether a writer removes and recreates partitions while readers run.
    ///
    /// With this off, nothing is ever retired and the grace period never has
    /// work, which is the blind spot this workload was written to close.
    pub churn: bool,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            partitions: 64,
            rows_per_partition: 256,
            readers: 4,
            routing: Routing::Ref,
            spread: Spread::Hot,
            batch: 32,
            churn: true,
        }
    }
}

/// What a run produced.
#[derive(Clone, Copy, Debug)]
pub struct Outcome {
    /// Wall clock of the slowest reader: the time the caller actually waits.
    pub elapsed: Duration,
    /// Ticks handled across all readers.
    pub ticks: u64,
    /// Ticks that routed to a partition the writer had removed. Not an error:
    /// it is the workload meeting churn, and a run reporting zero of them with
    /// `churn` on is a run whose writer never got scheduled.
    pub missed: u64,
    /// Partitions the writer removed and recreated.
    pub churned: u64,
    /// Retired partitions still queued when the run ended.
    ///
    /// The number that separates a scheme which reclaims under continuous read
    /// traffic from one that waits for a quiet instant that never comes. It is
    /// a backlog rather than a count of frees because `remove` already drives
    /// `collect` itself, so an external caller sees an empty queue and learns
    /// nothing. What is worth knowing is whether the queue stays short while
    /// readers keep arriving: a backlog tracking `churned` means retirements
    /// are piling up and nothing is being freed.
    pub retired_backlog: usize,
}

/// Build the table set and fill it.
pub fn populate(shape: Shape) -> TickPartitions {
    let table = TickPartitions::new();
    for symbol in 0..shape.partitions {
        let partition = table
            .partition_or_create(symbol)
            .expect("under the partition limit");
        for id in 0..shape.rows_per_partition {
            futures::executor::block_on(partition
                .insert(TickRow {
                    id,
                    price: id as f64,
                    qty: id,
                }))
                .expect("fresh partition, unique ids");
        }
    }
    table
}

/// Run one repetition and report what it cost.
///
/// The measured window starts after every reader has been spawned and released
/// by the barrier, so thread creation is outside it.
pub fn run(shape: Shape, ticks_per_reader: u64) -> Outcome {
    let table = Arc::new(populate(shape));
    let go = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let missed = Arc::new(AtomicU64::new(0));
    let churned = Arc::new(AtomicU64::new(0));

    let writer = shape.churn.then(|| {
        let table = Arc::clone(&table);
        let go = Arc::clone(&go);
        let stop = Arc::clone(&stop);
        let churned = Arc::clone(&churned);
        std::thread::spawn(move || {
            while !go.load(Ordering::Relaxed) {
                std::hint::spin_loop();
            }
            // The last symbol only. Readers under `Hot` all target symbol 0, so
            // churning a different one keeps the writer off the readers' line
            // and measures reclamation rather than a lock convoy.
            let symbol = shape.partitions - 1;
            let mut rng = Rng::new(0xC0FFEE);
            while !stop.load(Ordering::Relaxed) {
                table.remove(symbol);
                let fresh = table
                    .partition_or_create(symbol)
                    .expect("under the partition limit");
                let _ = futures::executor::block_on(fresh.insert(TickRow {
                    id: rng.below(shape.rows_per_partition),
                    price: 1.0,
                    qty: 1,
                }));
                // `remove` retires, advances and collects on its own, so
                // there is deliberately no `collect` call here: adding one
                // measures an already-drained queue and always reports zero.
                churned.fetch_add(1, Ordering::Relaxed);
            }
        })
    });

    let readers: Vec<_> = (0..shape.readers)
        .map(|reader| {
            let table = Arc::clone(&table);
            let go = Arc::clone(&go);
            let missed = Arc::clone(&missed);
            std::thread::spawn(move || {
                let mut rng = Rng::new(reader as u64 + 1);
                let mut local_missed = 0u64;
                while !go.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }

                let start = Instant::now();
                match shape.routing {
                    Routing::Arc => {
                        for _ in 0..ticks_per_reader {
                            let (symbol, id) = next_tick(&mut rng, shape, reader);
                            match table.partition(symbol) {
                                Some(p) => drop(std::hint::black_box(p.select(id))),
                                None => local_missed += 1,
                            }
                        }
                    }
                    Routing::Ref => {
                        for _ in 0..ticks_per_reader {
                            let (symbol, id) = next_tick(&mut rng, shape, reader);
                            match table.partition_ref(symbol) {
                                Some(p) => drop(std::hint::black_box(p.select(id))),
                                None => local_missed += 1,
                            }
                        }
                    }
                    Routing::Pinned => {
                        local_missed +=
                            pinned_ticks(&table, &mut rng, shape, reader, ticks_per_reader);
                    }
                }
                let elapsed = start.elapsed();
                missed.fetch_add(local_missed, Ordering::Relaxed);
                elapsed
            })
        })
        .collect();

    go.store(true, Ordering::Relaxed);
    let elapsed = readers
        .into_iter()
        .map(|r| r.join().expect("reader panicked"))
        .max()
        .unwrap_or_default();
    stop.store(true, Ordering::Relaxed);
    if let Some(writer) = writer {
        writer.join().expect("writer panicked");
    }

    Outcome {
        elapsed,
        ticks: ticks_per_reader * shape.readers as u64,
        missed: missed.load(Ordering::Relaxed),
        churned: churned.load(Ordering::Relaxed),
        retired_backlog: table.retired_len(),
    }
}

/// The pinned path, kept apart so the pin's scope is visible: one pin, then
/// `batch` lookups under it, and only then does the guard drop and let the
/// writer's retirements complete.
#[cfg(feature = "partition-pinned")]
fn pinned_ticks(
    table: &TickPartitions,
    rng: &mut Rng,
    shape: Shape,
    reader: usize,
    ticks_per_reader: u64,
) -> u64 {
    let mut missed = 0;
    let mut done = 0u64;
    while done < ticks_per_reader {
        let this_batch = shape.batch.min((ticks_per_reader - done) as usize);
        let pinned = table.pinned();
        for _ in 0..this_batch {
            let (symbol, id) = next_tick(rng, shape, reader);
            match pinned.get(symbol) {
                Some(p) => drop(std::hint::black_box(p.select(id))),
                None => missed += 1,
            }
        }
        done += this_batch as u64;
    }
    missed
}

#[cfg(not(feature = "partition-pinned"))]
fn pinned_ticks(_: &TickPartitions, _: &mut Rng, _: Shape, _: usize, _: u64) -> u64 {
    unreachable!("Routing::Pinned is not offered without the `partition-pinned` feature")
}

/// The next (symbol, row) a reader handles.
#[inline]
fn next_tick(rng: &mut Rng, shape: Shape, reader: usize) -> (u16, u64) {
    let symbol = match shape.spread {
        Spread::Hot => 0,
        // Distinct starting points, so readers do not march in step.
        Spread::Spread => {
            ((reader as u64 + rng.below(shape.partitions as u64)) % shape.partitions as u64) as u16
        }
    };
    (symbol, rng.below(shape.rows_per_partition))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(routing: Routing, churn: bool) -> Shape {
        Shape {
            partitions: 8,
            rows_per_partition: 32,
            readers: 2,
            routing,
            spread: Spread::Hot,
            batch: 8,
            churn,
        }
    }

    #[test]
    fn every_tick_is_accounted_for() {
        let outcome = run(shape(Routing::Ref, false), 500);
        assert_eq!(outcome.ticks, 1_000);
        assert!(outcome.elapsed > Duration::ZERO);
    }

    #[test]
    fn a_run_without_churn_retires_nothing() {
        // The control. If this ever reports reclamation, the workload is
        // retiring something the writer did not cause and the churn numbers
        // below mean nothing.
        let outcome = run(shape(Routing::Ref, false), 500);
        assert_eq!(outcome.churned, 0);
        assert_eq!(outcome.retired_backlog, 0);
    }

    #[test]
    fn reclamation_progresses_while_readers_keep_arriving() {
        // The property the whole reclamation choice rests on, and the reason
        // this crate does not use `seize`: readers arrive continuously, so
        // there is never an instant with no reader live. A scheme that waits
        // for quiescence reclaims nothing here and the retired partitions
        // queue forever. Reported as a number rather than asserted tightly,
        // because how *much* is scheduling; that it is not zero is the claim.
        let outcome = run(shape(Routing::Ref, true), 20_000);
        assert!(outcome.churned > 0, "the writer never ran");
        // Bounded, not zero: a retirement raced by a reader that pinned just
        // before it legitimately waits, so a short queue is healthy. A queue
        // the size of the churn count is the failure.
        assert!(
            (outcome.retired_backlog as u64) < outcome.churned / 2,
            "retired partitions are piling up: backlog {} against {} churns, so reclamation \
             is not keeping pace with readers that keep arriving",
            outcome.retired_backlog,
            outcome.churned
        );
    }

    #[test]
    fn every_routing_strategy_this_build_offers_runs() {
        for &routing in Routing::available() {
            let outcome = run(shape(routing, true), 2_000);
            assert_eq!(outcome.ticks, 4_000, "{} lost ticks", routing.label());
        }
    }
}
