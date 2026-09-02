//! Non-unique index insert and read against key fan-out.
//!
//!   cargo bench --bench fanout
//!
//! Row count is fixed at `fanout::ROWS`; only the number of rows sharing a key
//! changes. A sound index reports roughly the same per-row time across the
//! sweep. A rising line means the index is scanning the values already under
//! the key, which is what `WorkTablesIndex` 0.0.8 did and what reached a
//! consumer as a 21x regression.
//!
//! Read the arms against each other, not against an absolute: the guard is the
//! slope within one backend, and a slope survives a noisy machine in a way an
//! absolute number does not.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use wt_benchmarks::fanout::{self, FAN_OUT, ROWS};

// Each iteration of the insert sweep builds a table from empty, so samples are
// expensive and few. The budget is fixed rather than left to Criterion: a
// regression here shows as a slope across the sweep, which does not need tight
// confidence intervals, and a larger budget would let a scanning index run for
// minutes before reporting anything.
const SAMPLES: usize = 10;
const MEASURE: Duration = Duration::from_secs(10);
const WARM_UP: Duration = Duration::from_secs(1);

fn insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout/insert");
    group.throughput(Throughput::Elements(ROWS));
    group.sample_size(SAMPLES);
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for backend in fanout::BACKENDS {
        for fan_out in FAN_OUT {
            group.bench_with_input(
                BenchmarkId::new(backend.benchmark_label(), fan_out),
                &fan_out,
                |b, &fan_out| b.iter(|| fanout::insert_at_fan_out(backend, fan_out)),
            );
        }
    }
    group.finish();
}

criterion_group!(benches, insert);
criterion_main!(benches);
