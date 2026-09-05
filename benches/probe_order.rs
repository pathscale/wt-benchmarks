//! Probe order as the independent variable, across three index backends.
//!
//! **Consumer profile: EKOPathRS.** See `docs/BENCHMARK_CATALOG.md`. The sibling bench
//! `benches/arctic_paths.rs` holds the conditions this one inherits: the same
//! `fn:unit%06d/loop:%d` key shape, the same 163 / 512 / 8,192 / 131,072 sizes, and the
//! same seeded shuffle, which now lives in `wt_benchmarks::rng` so both files produce one
//! permutation rather than two.
//!
//! **Why this exists.** Probe order turned out to be the largest single effect in that
//! comparison, larger than the choice of index. Shuffled, `std::collections::BTreeMap`
//! point get goes 18.8 -> 30.4 -> 213.0 -> 152.7 ns across those four sizes, about 8x,
//! while Arctic goes 28.0 -> 52.4 -> 129.1 -> 69.8 ns, about 2.5x. A separate investigation
//! then measured that probing *in key order* instead moves `BTreeMap` by 3.4x at 131,072,
//! and a single fixed probe is that same bias taken to its limit. That limit is what an
//! earlier review used, which is how it concluded `BTreeMap` won everywhere.
//!
//! So the harness variable becomes the axis. Four orders over one fixed population:
//!
//!   in_order    probes walk the keys in key order, the tree's own layout
//!   shuffled    the seeded permutation, no locality
//!   fixed       one key, forever: perfect locality, and the earlier review's bias
//!   zipf        YCSB Zipf 0.99 over rank, scattered through `mix64` as `ycsb` does
//!
//! **`zipf` reuses `wt_benchmarks::ycsb::ZipfCdf`**, the generator this suite already had,
//! rather than a second definition of "skewed" written next door.
//!
//! **Read the results as a ratio to that backend's own `in_order` arm.** Absolute
//! nanoseconds mix the backend in with the order; the ratio is the degradation shape, and
//! the shapes are what is comparable across three structures with different constant costs.
//!
//! **All four orders probe an `n`-entry vector**, `fixed` included: it repeats one key `n`
//! times rather than returning a one-element vector. Only the *map* access pattern is
//! allowed to differ. Getting this wrong was worth 47% on its own - see `probes` below.
//!
//! **The null arms are per order, not one for the whole bench.** Cache pressure differs by
//! order, and so does the floor. This suite has seen identical arms report 7.8% at
//! p = 0.00; anything inside the floor is not a result.
//!
//! Run: `cargo bench --bench probe_order`. One axis: `-- 'probe_order/std'`.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Duration;

use arctic::SequentialMap;
use arctic::key::{BoxedStr, NonNull, Str};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use worktables_index::BTreeMap as WtiMap;
use wt_benchmarks::rng::{PROBE_SHUFFLE_SEED, Rng, mix64, shuffle_seeded};
use wt_benchmarks::ycsb::ZipfCdf;

/// The sizes the sibling bench reports, so the tables line up row for row.
const SIZES: &[usize] = &[163, 512, 8_192, 131_072];

/// Bounded on purpose. Sixteen arms per size times four sizes is 64 cells, and the machine
/// is shared with other agent lanes. Shrink the fixture before raising any of these.
const SAMPLES: usize = 50;
const MEASURE: Duration = Duration::from_millis(1_500);
const WARM_UP: Duration = Duration::from_millis(400);

/// YCSB's canonical skew, matching `Config::default().zipf_theta`.
const ZIPF_THETA: f64 = 0.99;

/// The probe order under test. All four produce `n` probes, so the walk over the probe
/// vector itself costs the same in every arm and only the map access differs.
#[derive(Clone, Copy)]
enum Order {
    InOrder,
    Shuffled,
    Fixed,
    Zipf,
}

impl Order {
    const ALL: [Self; 4] = [Self::InOrder, Self::Shuffled, Self::Fixed, Self::Zipf];

    fn as_str(self) -> &'static str {
        match self {
            Self::InOrder => "in_order",
            Self::Shuffled => "shuffled",
            Self::Fixed => "fixed",
            Self::Zipf => "zipf",
        }
    }
}

/// The key shape from the compiler this came from, identical to `benches/arctic_paths.rs`.
fn path_keys(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!("fn:unit{:06}/loop:{}", i / 3, i % 3))
        .collect()
}

/// The probe sequence for one order over one population.
///
/// **Every order returns `n` probes, `Fixed` included.** Each arm walks its vector with
/// `i = (i + 1) % len`, so the vector access is sequential and identically shaped
/// everywhere and only the *map* access pattern differs. The first version of this file
/// returned a single element for `Fixed`, which changed the modulus, the vector footprint
/// and the loop shape all at once, and it showed: `std` reported 27.4 ns for `fixed` at 163
/// keys against 18.6 ns for `shuffled`, a perfectly cache-resident probe losing to a
/// scattered one. That was the harness, not the index.
fn probes(keys: &[String], order: Order) -> Vec<&str> {
    match order {
        // `path_keys` emits `fn:unit000000/loop:0`, `..loop:1`, `..loop:2`, `fn:unit000001/..`
        // with a fixed-width unit number, so generation order is already lexicographic
        // order. Sorting anyway, because the arm's claim is "in key order" and that claim
        // should not depend on remembering the format string.
        Order::InOrder => {
            let mut out: Vec<&str> = keys.iter().map(String::as_str).collect();
            out.sort_unstable();
            out
        }
        Order::Shuffled => {
            let mut out: Vec<&str> = keys.iter().map(String::as_str).collect();
            shuffle_seeded(&mut out, PROBE_SHUFFLE_SEED);
            out
        }
        // The middle key, which is the probe `benches/arctic_paths.rs` uses for its integer
        // arms, repeated to the same length as the other orders. Hit forever: every level
        // of every structure stays resident. This is perfect locality, and it is also the
        // bias an earlier review took to its limit when it probed one key and concluded
        // `BTreeMap` won everywhere.
        Order::Fixed => vec![keys[keys.len() / 2].as_str(); keys.len()],
        // Rank drawn Zipf, then scattered through `mix64` so rank is not position. That is
        // exactly what `ycsb::generator::sample_key` does for `Distribution::Zipfian`; a
        // Zipf directly over position would put the hot set in one contiguous run and
        // measure locality twice.
        Order::Zipf => {
            let zipf = ZipfCdf::new(keys.len(), ZIPF_THETA);
            let mut rng = Rng::new(PROBE_SHUFFLE_SEED);
            (0..keys.len())
                .map(|_| {
                    let rank = zipf.sample(&mut rng, keys.len()) as u64;
                    keys[(mix64(rank) % keys.len() as u64) as usize].as_str()
                })
                .collect()
        }
    }
}

fn bench(c: &mut Criterion) {
    eprintln!(
        "conditions: debug_assertions={} arch={} os={} threads=1 \
         arctic_default_features=on wti_default_features=on suite={}",
        cfg!(debug_assertions),
        std::env::consts::ARCH,
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION"),
    );

    let mut group = c.benchmark_group("probe_order");
    group.sample_size(SAMPLES);
    group.measurement_time(MEASURE);
    group.warm_up_time(WARM_UP);

    for &n in SIZES {
        let paths = path_keys(n);

        let mut arctic = SequentialMap::<BoxedStr<NonNull>, u64>::new();
        for (i, k) in paths.iter().enumerate() {
            let key = Str::<NonNull>::new(k.as_str()).expect("no null byte");
            let _ = arctic.insert(key, i as u64);
        }
        let btree: BTreeMap<&str, u64> = paths
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i as u64))
            .collect();
        let mut wti = WtiMap::<&str, u64>::new();
        for (i, k) in paths.iter().enumerate() {
            wti.insert(k.as_str(), i as u64);
        }

        for order in Order::ALL {
            let probes = probes(&paths, order);

            // Key construction is hoisted, as in the sibling bench: `Str::new` costs 6.4 ns
            // and validating inside the timed loop charges Arctic for work the `&str` arms
            // never do. A caller with a checked key has already paid this.
            let arctic_probes: Vec<&Str<NonNull>> = probes
                .iter()
                .map(|s| Str::<NonNull>::new(s).expect("no null byte"))
                .collect();

            // Every arm must find every probe. A miss is a different code path in all three
            // structures, and an order that silently probed absent keys would be timing the
            // negative path against the positive one.
            assert!(
                probes.iter().all(|k| btree.contains_key(k)),
                "n={n} order={}: probes must all be present",
                order.as_str(),
            );
            assert_eq!(
                probes.iter().filter(|k| wti.get(**k).is_some()).count(),
                probes.len(),
                "n={n} order={}: wti disagrees with std about the population",
                order.as_str(),
            );
            assert_eq!(
                arctic_probes
                    .iter()
                    .filter(|k| arctic.get(k).is_some())
                    .count(),
                probes.len(),
                "n={n} order={}: arctic disagrees with std about the population",
                order.as_str(),
            );

            let label = order.as_str();

            group.bench_with_input(BenchmarkId::new(format!("std/{label}"), n), &n, |b, _| {
                let mut i = 0usize;
                b.iter(|| {
                    i = (i + 1) % probes.len();
                    black_box(btree.get(probes[i]).copied())
                })
            });
            // The null: the `std` arm again, same order, same data, no change. The gap
            // between this and its twin is the floor for that (backend-free) order.
            group.bench_with_input(
                BenchmarkId::new(format!("std_null/{label}"), n),
                &n,
                |b, _| {
                    let mut i = 0usize;
                    b.iter(|| {
                        i = (i + 1) % probes.len();
                        black_box(btree.get(probes[i]).copied())
                    })
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("arctic/{label}"), n),
                &n,
                |b, _| {
                    let mut i = 0usize;
                    b.iter(|| {
                        i = (i + 1) % arctic_probes.len();
                        black_box(arctic.get(arctic_probes[i]).copied())
                    })
                },
            );
            group.bench_with_input(BenchmarkId::new(format!("wti/{label}"), n), &n, |b, _| {
                let mut i = 0usize;
                b.iter(|| {
                    i = (i + 1) % probes.len();
                    black_box(wti.get(probes[i]).copied())
                })
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
