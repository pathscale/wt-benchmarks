//! The MoE-PGO profile: profiling counters, and map publication under readers.
//!
//! Every group runs on all three primary-index backends. The key is a dense
//! `u32`, which is the shape ART indexes exist for, and the measured gap
//! between them is not small, so a single-backend number would be misleading
//! rather than incomplete. Adding a fourth backend is one `moe_backend!` line
//! plus a `Backend` variant.
//!
//! `control` is the validity gate and should be read first. It contains no
//! WorkTable at all, so its result cannot legitimately vary between arms or
//! between runs. If it moves, the machine moved and every other number in the
//! run is void. This exists because an earlier session reported a 3.6x spread
//! on a pure dereference and treated the surrounding figures as real.
//!
//! The axis this suite is really for is **WorkTable versions**, not backends:
//! run it against the published crate and against the local checkout, and a
//! local build that is slower than the published one is the regression worth
//! chasing. `scripts/compare-worktable-versions.sh` drives both sides.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use wt_benchmarks::moe_pgo::{Backend, accumulate, accumulate_array, control, republish};

/// Layer widths. 12288 is the 8B donor's real width and is the one that counts;
/// the smaller ones show whether per-row cost is flat in the key set size.
const WIDTHS: [u32; 3] = [1024, 4096, 12288];
const UPDATES: u64 = 200_000;
const READERS: usize = 4;
const VERSIONS: u16 = 8;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
}

/// Read this first. No WorkTable in it; if it moves, the run is void.
fn validity(c: &mut Criterion) {
    let mut group = c.benchmark_group("moe_pgo/control");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);
    group.throughput(Throughput::Elements(1_000_000));
    group.bench_function("fixed_work", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += control(1_000_000);
            }
            total
        })
    });
    group.finish();
}

/// Profiling: read-modify-write over the whole key set, no locality.
fn accumulate_phase(c: &mut Criterion) {
    let runtime = rt();
    let mut group = c.benchmark_group("moe_pgo/accumulate");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);
    group.throughput(Throughput::Elements(UPDATES));

    for width in WIDTHS {
        for backend in Backend::ALL {
            group.bench_with_input(
                BenchmarkId::new(backend.label(), width),
                &width,
                |b, &width| {
                    b.iter_custom(|iters| {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            total += runtime.block_on(accumulate(backend, width, UPDATES));
                        }
                        total
                    })
                },
            );
        }
        // The denominator: the same workload with no database under it. No
        // index, no guard, no reclamation, no persistence, no schema. If the
        // gap is large, these counters do not belong in a database; if it is
        // ever small, the durability is worth paying for.
        group.bench_with_input(BenchmarkId::new("array", width), &width, |b, &width| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += accumulate_array(width, UPDATES);
                }
                total
            })
        });
    }
    group.finish();
}

/// Building a new map version. Insert-dominated, and the larger half of the
/// publish cycle by two orders of magnitude.
fn publish_phase(c: &mut Criterion) {
    let mut group = c.benchmark_group("moe_pgo/publish");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    for width in WIDTHS {
        group.throughput(Throughput::Elements(VERSIONS as u64 * width as u64));
        for backend in Backend::ALL {
            group.bench_with_input(
                BenchmarkId::new(backend.label(), width),
                &width,
                |b, &width| {
                    b.iter_custom(|iters| {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            total += republish(backend, width, READERS, VERSIONS).publish;
                        }
                        total
                    })
                },
            );
        }
    }
    group.finish();
}

/// Retiring the version the readers left behind, while they keep arriving.
///
/// The one place reclamation is load-bearing for this consumer: readers are
/// continuous, so there is never a quiet instant for a quiescence-based scheme
/// to use.
fn retire_phase(c: &mut Criterion) {
    let mut group = c.benchmark_group("moe_pgo/retire");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);
    group.throughput(Throughput::Elements(VERSIONS as u64));

    for width in WIDTHS {
        for backend in Backend::ALL {
            group.bench_with_input(
                BenchmarkId::new(backend.label(), width),
                &width,
                |b, &width| {
                    b.iter_custom(|iters| {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            total += republish(backend, width, READERS, VERSIONS).retire;
                        }
                        total
                    })
                },
            );
        }
    }
    group.finish();
}

criterion_group!(
    moe_pgo_benchmarks,
    validity,
    accumulate_phase,
    publish_phase,
    retire_phase
);
criterion_main!(moe_pgo_benchmarks);
