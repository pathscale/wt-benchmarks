//! Bulk map construction and retirement for MoE-PGO.

use std::time::Duration;

use criterion::{
    BenchmarkId, Criterion, SamplingMode, Throughput, criterion_group, criterion_main,
};

use wt_benchmarks::moe_pgo::{Backend, control};
use wt_benchmarks::moe_pgo2::{delete_loop, delete_many, delete_range, insert_loop, insert_many};

const WIDTHS: [u32; 4] = [1, 64, 1024, 12288];

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
}

fn configure(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(1));
}

fn validity(c: &mut Criterion) {
    let mut group = c.benchmark_group("moe_pgo2/control");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));
    group.throughput(Throughput::Elements(1_000_000));
    group.bench_function("fixed_work", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += control(1_000_000);
            }
            total
        });
    });
    group.finish();
}

fn build_map(c: &mut Criterion) {
    let runtime = rt();
    let mut group = c.benchmark_group("moe_pgo2/build");
    configure(&mut group);

    for width in WIDTHS {
        group.throughput(Throughput::Elements(u64::from(width)));
        for backend in Backend::ALL {
            group.bench_with_input(
                BenchmarkId::new(format!("{}/insert_loop", backend.label()), width),
                &width,
                |b, &width| {
                    b.iter_custom(|iters| {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            total += runtime.block_on(insert_loop(backend, width));
                        }
                        total
                    });
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("{}/insert_many", backend.label()), width),
                &width,
                |b, &width| {
                    b.iter_custom(|iters| {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            total += runtime.block_on(insert_many(backend, width));
                        }
                        total
                    });
                },
            );
        }
    }
    group.finish();
}

fn retire_map(c: &mut Criterion) {
    let runtime = rt();
    let mut group = c.benchmark_group("moe_pgo2/retire");
    configure(&mut group);

    for width in WIDTHS {
        group.throughput(Throughput::Elements(u64::from(width)));
        for backend in Backend::ALL {
            macro_rules! bench_delete {
                ($operation:literal, $run:path) => {
                    group.bench_with_input(
                        BenchmarkId::new(format!("{}/{}", backend.label(), $operation), width),
                        &width,
                        |b, &width| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    total += runtime.block_on($run(backend, width));
                                }
                                total
                            });
                        },
                    );
                };
            }

            bench_delete!("delete_loop", delete_loop);
            bench_delete!("delete_many", delete_many);
            bench_delete!("delete_range", delete_range);
        }
    }
    group.finish();
}

criterion_group!(moe_pgo2_benchmarks, validity, build_map, retire_map);
criterion_main!(moe_pgo2_benchmarks);
