//! Arctic against `BTreeMap` on structural-path keys, because a review said `BTreeMap` won.
//!
//! **Consumer profile: EKOPathRS.** See `docs/BENCHMARK_CATALOG.md`. A resident compiler
//! session holds 354 distinct regions across 163 units, capped at 512, and the query its
//! reactivity needs is a prefix scan over a structural path, not a point lookup. A backend
//! chosen on YCSB numbers is chosen on the wrong workload for this consumer.
//!
//! **Why this exists.** A storage review inside `~/code/EKOPathRS` measured Arctic's
//! `SequentialMap` against `std::collections::BTreeMap` on keys shaped `fn:<unit>/loop:<i>`
//! and reported `BTreeMap` ahead by about 2x at every size that compiler actually has -
//! 105 ns against 54 ns for a prefix scan at 163 keys, with the crossover near 131,000.
//! That is not what Arctic is sold on and the result was not believed. The scratch programs
//! were never committed, so this reconstructs the measurement and then isolates it.
//!
//! **The measurement it reproduces**, exactly as recorded: keys of the shape above, built
//! with `rustc -O` and `cargo --release`, prefix scan as `prefix(p).values(Ascend)` against
//! `range(p..).take_while(starts_with)`, plus a point `get`.
//!
//! **One hypothesis is already dead.** `arctic-wt/src/lib.rs:137` gates `validate!` and its
//! siblings on `cfg!(any(feature = "validate", debug_assertions, test))`, so a profile with
//! debug assertions on would make Arctic pay internal checks that `BTreeMap` never pays.
//! It was measured: `rustc -O` sets `debug_assertions = false`, so that is not the cause and
//! nobody needs to chase it again.
//!
//! **What this suspects instead, and why the arms are shaped this way.** The crate's own
//! summary says it out-performs "under various conditions (integer keys, skewed requests,
//! update-heavy)". The review used **string keys** on **`SequentialMap`**, which is Arctic
//! with both of its headline properties absent - not lock-free, not integer-keyed. A radix
//! tree must walk the bytes of a key; a comparison tree compares whole keys and exits early.
//! On 163 keys sharing the literal prefix `fn:`, those are very different amounts of work,
//! and none of it is a defect.
//!
//! So the arms follow `benches/nonunique.rs`, which documents this same trap in WorkTable's
//! own bench - two arms given different key types measure key handling as much as the index:
//!
//!   arctic_u128   vs btree_u128     the backend, on equal footing
//!   arctic_path   vs btree_path     the reported case, string keys
//!   btree_path    vs btree_u128     the key type, on one backend
//!   btree_path    vs btree_path_b   the null: identical work, two arms
//!
//! The null arm is not decoration. This suite has seen identical code report 7.8% at
//! p = 0.00, so any difference below the null floor is not a result.
//!
//! Run: `cargo bench --bench arctic_paths`. Print conditions with `--` `--verbose`.

use std::collections::BTreeMap;
use std::hint::black_box;

use arctic::key::{BoxedStr, NonNull, Str};
use arctic::{Order, SequentialMap};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use worktables_index::BTreeMap as WtiMap;
use wt_benchmarks::rng::{PROBE_SHUFFLE_SEED, shuffle_seeded};

/// The sizes the review reported, so the tables line up row for row.
const SIZES: &[usize] = &[163, 512, 8_192, 131_072];

/// The key shape from the compiler this came from: a structural path, long, with a shared
/// leading literal. Prefix sharing is the thing a radix tree pays for, so it is the variable.
fn path_keys(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("fn:unit{:06}/loop:{}", i / 3, i % 3)).collect()
}

/// The same population addressed as an integer, which is the condition Arctic claims.
/// Derived from the path rather than invented, so the two arms describe one workload.
fn int_keys(n: usize) -> Vec<u128> {
    path_keys(n)
        .iter()
        .map(|k| {
            let mut h: u128 = 0xcbf2_9ce4_8422_2325;
            for b in k.as_bytes() {
                h = (h ^ u128::from(*b)).wrapping_mul(0x1000_0000_01b3);
            }
            h
        })
        .collect()
}

/// The prefix a scan asks for: every loop of one unit, which is the query reactivity needs.
fn scan_prefix(i: usize) -> String {
    format!("fn:unit{:06}/", i)
}

/// Probes in a deterministic shuffle rather than in key order.
///
/// Probe order is the largest effect in this comparison and it is entirely harness: an
/// in-order walk moves `BTreeMap` point get by 3.4x at 131,072 keys against a shuffled one,
/// because it walks the tree the way the tree is laid out. Seeded so two runs compare.
///
/// The shuffle itself now lives in `wt_benchmarks::rng`, because `benches/probe_order.rs`
/// makes this order one of four and has to produce the identical permutation. Two benches
/// with two private shuffles cannot be read against each other.
fn shuffled_probes(keys: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = keys.iter().map(String::as_str).collect();
    shuffle_seeded(&mut out, PROBE_SHUFFLE_SEED);
    out
}

fn bench(c: &mut Criterion) {
    eprintln!(
        "conditions: debug_assertions={} target={} arctic={}",
        cfg!(debug_assertions),
        std::env::consts::ARCH,
        env!("CARGO_PKG_VERSION"),
    );

    let mut group = c.benchmark_group("arctic_paths");

    for &n in SIZES {
        let paths = path_keys(n);
        let ints = int_keys(n);

        let mut arctic_path = SequentialMap::<BoxedStr<NonNull>, u64>::new();
        for (i, k) in paths.iter().enumerate() {
            let key = Str::<NonNull>::new(k.as_str()).expect("no null byte");
            let _ = arctic_path.insert(key, i as u64);
        }
        let mut arctic_int = SequentialMap::<u128, u64>::new();
        for (i, k) in ints.iter().enumerate() {
            let _ = arctic_int.insert(*k, i as u64);
        }
        let btree_path: BTreeMap<&str, u64> =
            paths.iter().enumerate().map(|(i, k)| (k.as_str(), i as u64)).collect();
        let btree_int: BTreeMap<u128, u64> =
            ints.iter().enumerate().map(|(i, k)| (*k, i as u64)).collect();
        let mut wti_path = WtiMap::<&str, u64>::new();
        for (i, k) in paths.iter().enumerate() {
            wti_path.insert(k.as_str(), i as u64);
        }

        let p = scan_prefix((n / 3) / 2);
        let probe_int = ints[n / 2];

        // **The guard that makes this benchmark honest.** A scan returning nothing is
        // constant-time, and the review being reproduced reported Arctic "flat" at 105 to
        // 143 ns across three orders of magnitude, which is exactly the shape of an
        // always-empty result. The same review separately found that
        // `SequentialMap::prefix` accepts a validated key and silently returns zero hits,
        // because a terminated string can never be a proper prefix of another. So both arms
        // are required to find the same non-zero number before either is timed. If this
        // panics, the original measurement timed a no-op against real work.
        // **Fixed in 0.1.9**, and this file's pin is what said so. Until then the validated
        // form returned zero while the bare `&str` returned the right count, so the timed arm
        // used the bare form and asserted the validated one still returned zero. When the fix
        // landed that assertion failed by design, naming the file and telling the reader to
        // re-measure. The timed arm now uses the validated form, which is what a caller with
        // a checked key actually writes.
        let validated = Str::<NonNull>::new(p.as_str()).expect("no null byte");
        let arctic_hits = arctic_path.prefix(validated.into()).values(Order::Ascend).count();
        let bare_hits = arctic_path.prefix(p.as_str().into()).values(Order::Ascend).count();
        let btree_hits = btree_path
            .range(p.as_str()..)
            .take_while(|(k, _)| k.starts_with(p.as_str()))
            .count();
        // The durable guard the pin became: the two call forms must agree. That is the
        // property the defect broke, and it holds independently of which one is timed.
        assert_eq!(
            arctic_hits, bare_hits,
            "n={n}: the validated and bare call forms disagree - validated {arctic_hits}, \
             bare {bare_hits}. This is the 0.1.9 defect returning, in one direction or the \
             other",
        );
        assert!(
            arctic_hits > 0 && arctic_hits == btree_hits,
            "n={n}: the two arms must scan the same population before either is timed - \
             arctic returned {arctic_hits}, btree returned {btree_hits}",
        );

        // The reported case: prefix scan, string keys.
        group.bench_with_input(BenchmarkId::new("arctic_path_prefix", n), &n, |b, _| {
            b.iter(|| {
                black_box(arctic_path.prefix(p.as_str().into()).values(Order::Ascend).count())
            })
        });
        group.bench_with_input(BenchmarkId::new("btree_path_prefix", n), &n, |b, _| {
            b.iter(|| {
                black_box(
                    btree_path
                        .range(p.as_str()..)
                        .take_while(|(k, _)| k.starts_with(p.as_str()))
                        .count(),
                )
            })
        });
        // The null: the same arm twice. Anything below this gap is not a result.
        group.bench_with_input(BenchmarkId::new("btree_path_prefix_null", n), &n, |b, _| {
            b.iter(|| {
                black_box(
                    btree_path
                        .range(p.as_str()..)
                        .take_while(|(k, _)| k.starts_with(p.as_str()))
                        .count(),
                )
            })
        });

        // Point get, string keys.
        //
        // Two things here are deliberate, and both were wrong in the first version of this
        // file. **Key construction is hoisted**: `Str::new` costs 6.4 ns and validating
        // inside the timed loop charged Arctic +17.5% while the `BTreeMap` arm compared a
        // plain `&str`, which measures key handling and calls it the index. **And the probe
        // rotates over a shuffled order** rather than hammering one key: probing in key
        // order instead of shuffled moves `BTreeMap` point get by 3.4x at 131,072, so a
        // single fixed probe is that bias taken to its limit and it flatters the comparison
        // tree. A deterministic shuffle keeps the two arms on one population.
        let probes: Vec<&str> = shuffled_probes(&paths);
        let arctic_probes: Vec<&Str<NonNull>> = probes
            .iter()
            .map(|s| Str::<NonNull>::new(s).expect("no null byte"))
            .collect();

        group.bench_with_input(BenchmarkId::new("arctic_path_get", n), &n, |b, _| {
            let mut i = 0usize;
            b.iter(|| {
                i = (i + 1) % arctic_probes.len();
                black_box(arctic_path.get(arctic_probes[i]).copied())
            })
        });
        group.bench_with_input(BenchmarkId::new("btree_path_get", n), &n, |b, _| {
            let mut i = 0usize;
            b.iter(|| {
                i = (i + 1) % probes.len();
                black_box(btree_path.get(probes[i]).copied())
            })
        });

        // The third arm: the house index, on the same footing. WorkTablesIndex is a
        // comparison tree like std's rather than a radix tree like Arctic, so it is the
        // control that says whether Arctic's prefix advantage is about radix structure or
        // about this crate. Called directly rather than through worktable's `using` clause,
        // because the question is the index and not the table around it.
        group.bench_with_input(BenchmarkId::new("wti_path_prefix", n), &n, |b, _| {
            b.iter(|| {
                black_box(
                    wti_path
                        .range(p.as_str()..)
                        .take_while(|(k, _)| k.starts_with(p.as_str()))
                        .count(),
                )
            })
        });
        group.bench_with_input(BenchmarkId::new("wti_path_get", n), &n, |b, _| {
            let mut i = 0usize;
            b.iter(|| {
                i = (i + 1) % probes.len();
                black_box(wti_path.get(probes[i]).copied())
            })
        });

        // The backend on equal footing: integer keys, the condition Arctic claims.
        group.bench_with_input(BenchmarkId::new("arctic_int_get", n), &n, |b, _| {
            b.iter(|| black_box(arctic_int.get(&probe_int).copied()))
        });
        group.bench_with_input(BenchmarkId::new("btree_int_get", n), &n, |b, _| {
            b.iter(|| black_box(btree_int.get(&probe_int).copied()))
        });
    }

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
