//! The MoE-PGO profile.
//!
//! # What this project actually asks of WorkTable
//!
//! Stated narrowly, because an earlier version of this module was much broader
//! and most of it was wrong.
//!
//! MoE-PGO re-draws mixture-of-experts boundaries from measured traffic. The
//! parameters live in memory-mapped safetensors and never move; an expert is a
//! *view* over that fixed block store, so re-partitioning writes a new map
//! rather than relocating anything. A map is one `u16` per neuron, roughly
//! 864 KB at donor scale.
//!
//! That leaves WorkTable holding two things, and only two:
//!
//! **The counters.** Profiling updates per-neuron statistics as traffic
//! arrives. Every token touches many neurons, so this is a read-modify-write
//! stream over a *dense key set with no locality* and no Zipf tail: the working
//! set is the whole table. Nothing else in this suite measures that shape.
//!
//! **The map, versioned.** Re-partitioning publishes a new version while
//! readers are still resolving against the current one, then retires the old.
//! Readers arrive continuously, so there is never a quiet instant. That is
//! precisely the property epoch reclamation has and quiescence-based schemes do
//! not, which makes this the one place the reclamation work is load-bearing for
//! this consumer.
//!
//! # What this deliberately does not measure, and why
//!
//! **Per-token neuron routing.** An earlier version modelled thousands of
//! lookups per token, on the theory that relevance is discovered during the
//! forward pass. It is not: the partition is drawn so that what a request needs
//! is known before compute starts. If per-token routing were ever needed that
//! would be evidence the partition had failed, not a serving requirement.
//!
//! **Weight paging.** WorkTable does not hold parameters. The mmap'd store is
//! the OS page cache's problem.
//!
//! **Generational churn at scale.** The map is 864 KB and versions are cheap.
//! There is no memory pressure to engineer around and no mass invalidation: a
//! resident block stays valid across a re-partition, because experts are views.
//!
//! # Backends
//!
//! The key is a dense `u32`, which is the shape ART indexes exist for, so every
//! phase runs on all three primary-index backends. Measuring only the default
//! answers the wrong question, and the gap is not small.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::rng::Rng;

/// One neuron's row: which expert currently claims it, and how often it fired.
///
/// Both live in one row because both are per-neuron and the pipeline writes
/// them in different phases: publishing sets `expert`, profiling accumulates
/// `fires`. `fires` is fixed width and unindexed, so its update lands in the
/// archived row in place.
macro_rules! moe_backend {
    ($module:ident, $name:ident, $row:ident, $parts:ident, $query:ident, $using:ident) => {
        pub mod $module {
            use std::sync::Arc;
            use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
            use std::time::{Duration, Instant};

            use worktable::prelude::*;
            use worktable::worktable;

            use crate::rng::Rng;

            worktable! {
                name: $name,
                persist: false,
                partition_by: version: u16,
                columns: {
                    neuron: u32 primary_key using $using,
                    expert: u16,
                    fires: u64,
                },
                queries: {
                    update: {
                        Fires(fires) by neuron,
                    }
                }
            }

            pub fn build(width: u32) -> $parts {
                let set = $parts::new();
                fill(&set, 0, width);
                set
            }

            pub fn fill(set: &$parts, version: u16, width: u32) {
                let partition = set
                    .partition_or_create(version)
                    .expect("under the partition limit");
                for neuron in 0..width {
                    partition
                        .insert($row {
                            neuron,
                            expert: (neuron % 64) as u16,
                            fires: 0,
                        })
                        .expect("unique neuron ids");
                }
            }

            /// Profiling: read-modify-write over the whole key set, no locality.
            pub async fn accumulate(set: &$parts, width: u32, updates: u64) -> Duration {
                let mut rng = Rng::new(7);
                let start = Instant::now();
                let Some(partition) = set.partition_ref(0) else {
                    return start.elapsed();
                };
                for _ in 0..updates {
                    let neuron = rng.below(width as u64) as u32;
                    if let Some(row) = partition.select(neuron) {
                        let _ = partition
                            .update_fires(
                                $query {
                                    fires: row.fires + 1,
                                },
                                neuron,
                            )
                            .await;
                    }
                }
                start.elapsed()
            }

            /// Publish `versions` maps while `readers` threads resolve against
            /// whichever is current, then retire the one they left behind.
            pub fn republish(width: u32, readers: usize, versions: u16) -> super::PublishOutcome {
                let set = Arc::new(build(width));
                let current = Arc::new(AtomicU16::new(0));
                let stop = Arc::new(AtomicBool::new(false));
                let resolved = Arc::new(AtomicU64::new(0));
                let missed = Arc::new(AtomicU64::new(0));

                let workers: Vec<_> = (0..readers)
                    .map(|r| {
                        let set = Arc::clone(&set);
                        let current = Arc::clone(&current);
                        let stop = Arc::clone(&stop);
                        let resolved = Arc::clone(&resolved);
                        let missed = Arc::clone(&missed);
                        std::thread::spawn(move || {
                            let mut rng = Rng::new(r as u64 + 1);
                            let (mut ok, mut no) = (0u64, 0u64);
                            while !stop.load(Ordering::Relaxed) {
                                let version = current.load(Ordering::Acquire);
                                match set.partition_ref(version) {
                                    Some(map) => {
                                        let n = rng.below(width as u64) as u32;
                                        std::hint::black_box(map.select(n));
                                        ok += 1;
                                    }
                                    None => no += 1,
                                }
                            }
                            resolved.fetch_add(ok, Ordering::Relaxed);
                            missed.fetch_add(no, Ordering::Relaxed);
                        })
                    })
                    .collect();

                let (mut publish, mut retire) = (Duration::ZERO, Duration::ZERO);
                for version in 1..=versions {
                    let t = Instant::now();
                    fill(&set, version, width);
                    publish += t.elapsed();

                    let previous = current.swap(version, Ordering::Release);
                    let t = Instant::now();
                    set.remove(previous);
                    retire += t.elapsed();
                }

                stop.store(true, Ordering::Relaxed);
                for w in workers {
                    w.join().expect("reader panicked");
                }

                super::PublishOutcome {
                    publish,
                    retire,
                    resolved: resolved.load(Ordering::Relaxed),
                    missed: missed.load(Ordering::Relaxed),
                }
            }
        }
    };
}

moe_backend!(
    wti,
    MoeWti,
    MoeWtiRow,
    MoeWtiPartitions,
    FiresQuery,
    worktables_index
);
moe_backend!(
    congee,
    MoeCongee,
    MoeCongeeRow,
    MoeCongeePartitions,
    FiresQuery,
    congee
);
moe_backend!(
    arctic,
    MoeArctic,
    MoeArcticRow,
    MoeArcticPartitions,
    FiresQuery,
    arctic
);

/// Which primary-index backend the map and counters use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    WorktablesIndex,
    Congee,
    Arctic,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::WorktablesIndex, Backend::Congee, Backend::Arctic];

    pub fn label(self) -> &'static str {
        match self {
            Backend::WorktablesIndex => "worktables_index",
            Backend::Congee => "congee",
            Backend::Arctic => "arctic",
        }
    }
}

/// What a publish cycle cost, and what it cost the readers.
#[derive(Clone, Copy, Debug, Default)]
pub struct PublishOutcome {
    /// Building and filling the new version.
    pub publish: Duration,
    /// Retiring the one the readers left behind.
    pub retire: Duration,
    /// Lookups the readers completed while that happened.
    pub resolved: u64,
    /// Lookups that found no map.
    ///
    /// A reader loads the current version, then looks it up. If a version can
    /// be retired between those two steps the reader is handed nothing. Zero
    /// is not proof the window is closed, only that it was not hit.
    pub missed: u64,
}

/// Profiling updates, on one backend.
pub async fn accumulate(backend: Backend, width: u32, updates: u64) -> Duration {
    match backend {
        Backend::WorktablesIndex => {
            let set = wti::build(width);
            wti::accumulate(&set, width, updates).await
        }
        Backend::Congee => {
            let set = congee::build(width);
            congee::accumulate(&set, width, updates).await
        }
        Backend::Arctic => {
            let set = arctic::build(width);
            arctic::accumulate(&set, width, updates).await
        }
    }
}

/// Map publication under live readers, on one backend.
pub fn republish(backend: Backend, width: u32, readers: usize, versions: u16) -> PublishOutcome {
    match backend {
        Backend::WorktablesIndex => wti::republish(width, readers, versions),
        Backend::Congee => congee::republish(width, readers, versions),
        Backend::Arctic => arctic::republish(width, readers, versions),
    }
}

/// A fixed amount of work that cannot vary with any parameter under test.
///
/// The validity gate. Every other measurement here will happily report a number
/// taken on a loaded machine; this one has no WorkTable in it at all, so if it
/// moves between arms the machine moved and the run is void. Nothing else in
/// this suite has one, which is how an earlier session came to report a 3.6x
/// spread on a pure dereference and treat the surrounding numbers as real.
pub fn control(iterations: u64) -> Duration {
    let mut rng = Rng::new(1);
    let start = Instant::now();
    let mut acc = 0u64;
    for _ in 0..iterations {
        acc = acc.wrapping_add(rng.next_u64());
    }
    std::hint::black_box(acc);
    start.elapsed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_backend_accumulates() {
        for backend in Backend::ALL {
            let took = accumulate(backend, 1024, 2_000).await;
            assert!(took > Duration::ZERO, "{}", backend.label());
        }
    }

    #[test]
    fn every_backend_publishes_and_retires_under_live_readers() {
        for backend in Backend::ALL {
            let out = republish(backend, 1024, 2, 4);
            assert!(
                out.resolved > 0,
                "{} readers resolved nothing",
                backend.label()
            );
        }
    }

    #[test]
    fn the_control_is_stable() {
        let a = control(1_000_000);
        let b = control(1_000_000);
        let ratio = a.as_secs_f64().max(b.as_secs_f64()) / a.as_secs_f64().min(b.as_secs_f64());
        assert!(
            ratio < 2.0,
            "control varied {ratio:.2}x between back-to-back runs"
        );
    }
}

/// The same profiling workload against a plain array.
///
/// Not a WorkTable backend, and not a fair fight: it has no index, no guard, no
/// reclamation, no persistence and no schema. That is the point. The counters
/// are 442,368 `u64` at donor scale, which is 3.5 MB, and the map is 442,368
/// `u16`, which is 864 KB. Both are small, dense, integer-keyed and fixed size,
/// which is exactly the shape where a direct index beats any database.
///
/// It is here as the denominator. A decision to keep these structures out of
/// WorkTable should rest on a measured gap rather than on the assumption that
/// an array is obviously faster, and if the gap is ever small enough that the
/// durability and versioning are worth paying for, this is what says so.
///
/// `AtomicU64` rather than plain `u64` because real profiling accumulates from
/// several threads, so the comparison should carry that cost.
pub fn accumulate_array(width: u32, updates: u64) -> Duration {
    use std::sync::atomic::AtomicU64;

    let counters: Vec<AtomicU64> = (0..width).map(|_| AtomicU64::new(0)).collect();
    let mut rng = Rng::new(7);
    let start = Instant::now();
    for _ in 0..updates {
        let neuron = rng.below(width as u64) as usize;
        counters[neuron].fetch_add(1, Ordering::Relaxed);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(&counters);
    elapsed
}

/// The array workload again, but paying the async cost the WorkTable path pays.
///
/// The database arm `.await`s a generated update per iteration; this one is a
/// tight synchronous loop. Comparing them directly attributes the whole gap to
/// storage when some of it may be the state machine, or worse, a yield to the
/// runtime. This awaits a ready future per iteration so the two differ in the
/// database and not in the shape of the loop.
pub async fn accumulate_array_async(width: u32, updates: u64) -> Duration {
    use std::sync::atomic::AtomicU64;

    let counters: Vec<AtomicU64> = (0..width).map(|_| AtomicU64::new(0)).collect();
    let mut rng = Rng::new(7);
    let start = Instant::now();
    for _ in 0..updates {
        let neuron = rng.below(width as u64) as usize;
        ready(counters[neuron].fetch_add(1, Ordering::Relaxed)).await;
    }
    let elapsed = start.elapsed();
    std::hint::black_box(&counters);
    elapsed
}

/// A future that is ready on first poll: the cheapest possible `.await`.
async fn ready<T>(value: T) -> T {
    value
}

/// Does the generated update yield to the runtime, or complete inline?
///
/// If it yields, every profiling update is a scheduler round trip and the
/// database arm is measuring tokio as much as WorkTable.
pub async fn accumulate_yield_probe(width: u32, updates: u64) -> Duration {
    let mut rng = Rng::new(7);
    let start = Instant::now();
    for _ in 0..updates {
        let neuron = rng.below(width as u64);
        std::hint::black_box(neuron);
        tokio::task::yield_now().await;
    }
    start.elapsed()
}

#[cfg(test)]
mod async_cost_tests {
    use super::*;

    /// How much of the database-versus-array gap is the `.await`?
    #[tokio::test]
    async fn attribute_the_gap() {
        const WIDTH: u32 = 12288;
        const UPDATES: u64 = 200_000;

        let sync = accumulate_array(WIDTH, UPDATES);
        let asy = accumulate_array_async(WIDTH, UPDATES).await;
        let yielded = accumulate_yield_probe(WIDTH, UPDATES).await;
        let wt = accumulate(Backend::Arctic, WIDTH, UPDATES).await;

        let rate = |d: Duration| UPDATES as f64 / d.as_secs_f64() / 1e6;
        println!("array, sync loop     {:>9.1} M/s", rate(sync));
        println!("array, ready .await  {:>9.1} M/s", rate(asy));
        println!("bare yield_now only  {:>9.1} M/s", rate(yielded));
        println!("worktable arctic     {:>9.1} M/s", rate(wt));
    }
}
