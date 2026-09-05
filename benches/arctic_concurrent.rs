//! Arctic's `ConcurrentMap` against the lock it replaces, read-mostly, 1 to 8 threads.
//!
//! **Consumer profile: EKOPathRS.** See `docs/BENCHMARK_CATALOG.md`. Same key shape and
//! same sizes as `benches/arctic_paths.rs` and `benches/probe_order.rs`, so the numbers sit
//! next to theirs: `fn:unit%06d/loop:%d` at 163 / 512 / 8,192 / 131,072, probed in the
//! shared seeded shuffle from `wt_benchmarks::rng`.
//!
//! **Why this exists.** Arctic's headline property is that `ConcurrentMap` is lock-free
//! with wait-free reads. Nothing else here measured it. `benches/arctic_paths.rs` and
//! `benches/probe_order.rs` both use `SequentialMap`, which gives that property up
//! deliberately, so every Arctic number this consumer had was from the arm with both
//! headline properties switched off. A resident compiler server is exactly the shape that
//! would use the other one: many reader threads resolving structural paths while a small
//! number of writers republish regions.
//!
//! **Not redundant with `benches/concurrent_mix.rs`.** That one asks "which index backend
//! should this table use" and asks it at WorkTable table level with disjoint key ranges per
//! thread, because congee and Arctic exist there only as backends. This one is at index
//! level, keys overlap between threads on purpose, and the contenders are not three index
//! backends but one structure against the two lock disciplines it is offered as a
//! replacement for.
//!
//! **The arms, and what each one isolates:**
//!
//!   arctic_lockfree        `ConcurrentMap`, default `smr-ps-reclaim`
//!   arctic_lockfree_noop   the same map with `NoOp` SMR: the reclamation-cost floor
//!   arctic_seq_rwlock      `SequentialMap` behind an `RwLock`, the thing given up
//!   std_rwlock             `std::collections::BTreeMap` behind an `RwLock`
//!   std_rwlock_null        `std_rwlock` again, unchanged: the floor
//!   wti_concurrent         WorkTablesIndex's own concurrent map, which it does have
//!
//! `NoOp` leaks every retired allocation by construction and is a benchmarking type, not a
//! usable one. It is here as a lower bound: the gap between it and `arctic_lockfree` is
//! what safe memory reclamation costs on the read path, and no configuration can be faster.
//!
//! **What this bench does NOT show, and should not be read as showing.** `ps-reclaim`'s
//! stated guarantee over `seize` and a plain reader counter is that a reader starting
//! *after* a retirement does not delay that retirement. That is a claim about reclamation
//! latency and retained memory under continuously arriving readers, not about read
//! throughput. A read-mostly throughput sweep cannot distinguish it: this workload upserts
//! `u64` values over a fixed key set, so it retires almost nothing, and the arm that would
//! settle the question has to measure retirement progress and bytes retained, not
//! nanoseconds per get. `src/moe_pgo.rs`'s Retire group is the shape that measurement
//! wants. Saying "ps-reclaim won the throughput bench" would be answering a question
//! nobody asked with a number that does not contain it.
//!
//! **The write mix is deliberately structure-preserving.** Every write is an upsert of an
//! existing key with a new `u64`, so the key set never changes and no arm pays for a split,
//! a rebalance, or node retirement that another arm avoids. What is being compared is
//! reader/writer *interference*: an `RwLock` writer excludes every reader for the duration,
//! a lock-free map does not. Introducing structural writes would fold index-maintenance
//! cost into that and neither effect could be read off the result.
//!
//! Run: `cargo bench --bench arctic_concurrent`. One axis: `-- 'arctic_concurrent/std'`.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use arctic::concurrent::smr::NoOp;
use arctic::key::{BoxedStr, NonNull, Str};
use arctic::{ConcurrentMap, SequentialMap};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use parking_lot::RwLock;
use worktables_index::concurrent::map::BTreeMap as WtiConcurrentMap;
use wt_benchmarks::rng::{PROBE_SHUFFLE_SEED, shuffle_seeded};

/// The sizes the sibling benches report, so the tables line up row for row.
const SIZES: &[usize] = &[163, 512, 8_192, 131_072];

/// The requested sweep. Sixteen physical cores here, so 8 is still inside the machine and
/// the numbers are scaling rather than oversubscription.
const THREADS: &[usize] = &[1, 2, 4, 8];

/// One operation in twenty is a write. Fixed stride rather than a sampled coin, so the mix
/// is identical in every arm and no RNG runs inside the timed region. Threads start at
/// different offsets in the probe vector, so their writes are staggered rather than in
/// lockstep.
const WRITE_EVERY: usize = 20;

/// Bounded on purpose: six arms times four thread counts times four sizes is 96 cells, each
/// spawning its thread set per sample. Shrink the fixture before raising any of these.
const SAMPLES: usize = 10;
const MEASURE: Duration = Duration::from_millis(1_000);
const WARM_UP: Duration = Duration::from_millis(300);

/// The key shape from the compiler this came from, identical to the sibling benches.
fn path_keys(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("fn:unit{:06}/loop:{}", i / 3, i % 3)).collect()
}

/// The shared seeded shuffle. Probe order is the largest single effect in this comparison
/// (see `benches/probe_order.rs`), so it is pinned here rather than left to each arm.
fn shuffled_probes(keys: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = keys.iter().map(String::as_str).collect();
    shuffle_seeded(&mut out, PROBE_SHUFFLE_SEED);
    out
}

/// Run `iters` operations per thread across `threads` threads and return the wall time.
///
/// The clock starts once every worker has reached the barrier, so thread creation is
/// outside the measurement. Criterion's linear sampling then fits a slope through the
/// remaining per-sample constant, which is what makes this readable at small `iters`.
///
/// `body(thread_index, operation_index) -> u64` performs one operation and returns
/// something the compiler cannot discard. Each thread's running total is folded into a
/// shared atomic after the timed region, never inside it.
fn scaled<F>(threads: usize, iters: u64, body: F) -> Duration
where
    F: Fn(usize, u64) -> u64 + Sync,
{
    let barrier = Barrier::new(threads + 1);
    let sink = AtomicU64::new(0);
    let body = &body;
    let barrier = &barrier;
    let sink = &sink;

    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|thread| {
                scope.spawn(move || {
                    barrier.wait();
                    let mut total = 0u64;
                    for operation in 0..iters {
                        total = total.wrapping_add(body(thread, operation));
                    }
                    sink.fetch_add(total, AtomicOrdering::Relaxed);
                })
            })
            .collect();

        barrier.wait();
        let start = Instant::now();
        // Joined explicitly rather than by leaving the scope, so the clock stops after the
        // last worker and not after the scope's own bookkeeping.
        for worker in workers {
            worker.join().expect("worker thread panicked");
        }
        let elapsed = start.elapsed();
        black_box(sink.load(AtomicOrdering::Relaxed));
        elapsed
    })
}

/// Where thread `t` starts in the probe vector, and which operations it writes.
///
/// Threads are spread evenly across the probe vector rather than all starting at zero: an
/// arm where every thread reads the same key at the same instant measures the cache line,
/// not the index.
#[inline]
fn probe_index(thread: usize, operation: u64, threads: usize, len: usize) -> usize {
    let stride = len / threads.max(1);
    (thread.wrapping_mul(stride).wrapping_add(operation as usize)) % len
}

#[inline]
fn is_write(operation: u64) -> bool {
    operation % WRITE_EVERY as u64 == 0
}

fn bench(c: &mut Criterion) {
    eprintln!(
        "conditions: debug_assertions={} arch={} os={} \
         arctic_smr=ps-reclaim(default)+no-op wti_features=concurrent lock=parking_lot \
         write_every={WRITE_EVERY} suite={}",
        cfg!(debug_assertions),
        std::env::consts::ARCH,
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION"),
    );

    let mut group = c.benchmark_group("arctic_concurrent");
    group.sample_size(SAMPLES);
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &n in SIZES {
        let paths = path_keys(n);
        let probes = shuffled_probes(&paths);
        let arctic_probes: Vec<&Str<NonNull>> = probes
            .iter()
            .map(|s| Str::<NonNull>::new(s).expect("no null byte"))
            .collect();

        let arctic_lockfree = ConcurrentMap::<BoxedStr<NonNull>, u64>::new();
        let arctic_noop = ConcurrentMap::<BoxedStr<NonNull>, u64, NoOp>::new();
        let mut arctic_seq = SequentialMap::<BoxedStr<NonNull>, u64>::new();
        let mut std_map = BTreeMap::<&str, u64>::new();
        let wti = WtiConcurrentMap::<&str, u64>::new();

        for (index, key) in arctic_probes.iter().enumerate() {
            let value = index as u64;
            let _ = arctic_lockfree.insert(*key, value);
            let _ = arctic_noop.insert(*key, value);
            let _ = arctic_seq.insert(*key, value);
        }
        for (index, key) in probes.iter().enumerate() {
            std_map.insert(key, index as u64);
            wti.insert(key, index as u64);
        }

        // Every arm must hold the same population before any of it is timed. Five
        // structures loaded by five different calls is five chances to measure a map that
        // is missing rows, and a smaller map is faster.
        assert_eq!(std_map.len(), n, "n={n}: std arm lost rows");
        assert_eq!(wti.len(), n, "n={n}: wti arm lost rows");
        assert_eq!(
            arctic_probes.iter().filter(|k| arctic_lockfree.get(k).is_some()).count(),
            n,
            "n={n}: arctic lock-free arm lost rows",
        );
        assert_eq!(
            arctic_probes.iter().filter(|k| arctic_noop.get(k).is_some()).count(),
            n,
            "n={n}: arctic no-op-SMR arm lost rows",
        );
        assert_eq!(
            arctic_probes.iter().filter(|k| arctic_seq.get(k).is_some()).count(),
            n,
            "n={n}: arctic sequential arm lost rows",
        );

        let arctic_seq = RwLock::new(arctic_seq);
        let std_map = RwLock::new(std_map);

        for &threads in THREADS {
            // One element per thread per iteration, so the reported throughput is the
            // aggregate the scaling question is actually about.
            group.throughput(Throughput::Elements(threads as u64));
            let id = format!("{n}/t{threads}");

            group.bench_function(BenchmarkId::new("arctic_lockfree", &id), |b| {
                b.iter_custom(|iters| {
                    scaled(threads, iters, |thread, operation| {
                        let index = probe_index(thread, operation, threads, arctic_probes.len());
                        let key = arctic_probes[index];
                        if is_write(operation) {
                            arctic_lockfree.upsert(key, operation);
                            operation
                        } else {
                            arctic_lockfree.get(key).map_or(0, |value| *value)
                        }
                    })
                })
            });

            group.bench_function(BenchmarkId::new("arctic_lockfree_noop", &id), |b| {
                b.iter_custom(|iters| {
                    scaled(threads, iters, |thread, operation| {
                        let index = probe_index(thread, operation, threads, arctic_probes.len());
                        let key = arctic_probes[index];
                        if is_write(operation) {
                            arctic_noop.upsert(key, operation);
                            operation
                        } else {
                            arctic_noop.get(key).map_or(0, |value| *value)
                        }
                    })
                })
            });

            group.bench_function(BenchmarkId::new("arctic_seq_rwlock", &id), |b| {
                b.iter_custom(|iters| {
                    scaled(threads, iters, |thread, operation| {
                        let index = probe_index(thread, operation, threads, arctic_probes.len());
                        let key = arctic_probes[index];
                        if is_write(operation) {
                            arctic_seq.write().upsert(key, operation);
                            operation
                        } else {
                            arctic_seq.read().get(key).copied().unwrap_or(0)
                        }
                    })
                })
            });

            group.bench_function(BenchmarkId::new("std_rwlock", &id), |b| {
                b.iter_custom(|iters| {
                    scaled(threads, iters, |thread, operation| {
                        let index = probe_index(thread, operation, threads, probes.len());
                        let key = probes[index];
                        if is_write(operation) {
                            std_map.write().insert(key, operation);
                            operation
                        } else {
                            std_map.read().get(key).copied().unwrap_or(0)
                        }
                    })
                })
            });

            // The null: `std_rwlock` again, unchanged. Anything inside this gap is not a
            // result, and on a machine shared with other agent lanes the gap moves.
            group.bench_function(BenchmarkId::new("std_rwlock_null", &id), |b| {
                b.iter_custom(|iters| {
                    scaled(threads, iters, |thread, operation| {
                        let index = probe_index(thread, operation, threads, probes.len());
                        let key = probes[index];
                        if is_write(operation) {
                            std_map.write().insert(key, operation);
                            operation
                        } else {
                            std_map.read().get(key).copied().unwrap_or(0)
                        }
                    })
                })
            });

            // WorkTablesIndex does have a concurrent story: a structural read/write lock
            // over per-node locks. `lookup_for_select` is its definitive owned point read,
            // which is the call a table's select path makes.
            group.bench_function(BenchmarkId::new("wti_concurrent", &id), |b| {
                b.iter_custom(|iters| {
                    scaled(threads, iters, |thread, operation| {
                        let index = probe_index(thread, operation, threads, probes.len());
                        let key = probes[index];
                        if is_write(operation) {
                            wti.insert(key, operation);
                            operation
                        } else {
                            wti.lookup_for_select(&key).unwrap_or(0)
                        }
                    })
                })
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
