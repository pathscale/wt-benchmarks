//! `worktable-vec` against what an application actually writes instead of it.
//!
//! **Consumer profile: EKOPathRS.** See `docs/BENCHMARK_CATALOG.md`. That compiler holds
//! 2,082 `Vec<` across nine crates, 1,311 in the IR crate alone, and about 1,568 sites of
//! imperative vector manipulation inside transform bodies. The question this answers is
//! whether replacing that with named tables costs anything.
//!
//! **The claim under test: `worktable-vec` is within 1 to 2 ns of raw `Vec`.**
//!
//! **The comparison is deliberately not table-against-`Vec`.** `worktable-vec`'s own README
//! says what it replaces: *"Applications often grow an implicit table out of a `Vec` plus
//! bespoke scans or side indexes."* So the honest baseline for `IndexedTable` is not a bare
//! `Vec` - it is a `Vec` **plus the hand-rolled `BTreeMap` side index** the application would
//! have written, because that is the code being deleted. Timing a table with an index against
//! a `Vec` without one measures the index, calls it overhead, and rejects the crate for
//! carrying the thing it exists to carry.
//!
//! So four arms, in two honest pairs:
//!
//!   raw_vec_scan       vs linear_table     ordered rows, linear lookup, no index
//!   raw_vec_plus_index vs indexed_table    ordered rows plus a key to row-offset index
//!
//! Plus a **null arm**: `raw_vec_scan` run twice under two labels. This suite has seen
//! identical code report 7.8% at p = 0.00, and a 1-2 ns claim needs a floor well under 1 ns
//! to mean anything. On a busy machine that floor has been observed at 10% of 30 ns, which
//! is 3 ns - larger than the whole claim. **If the null arms differ by more than the gap you
//! are reading, there is no result on this machine today.** Run it quiet.
//!
//! Sizes start at the ones the consumer actually has. A compiler session holds 354 regions
//! over 163 units; the large sizes are there to show the shape, not because anyone has them.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use wtvec_pub::{IndexedTable, LinearTable};

/// The consumer's real size first, then decades to show the curve.
const SIZES: &[usize] = &[163, 512, 8_192];

/// Keys shaped like the thing being replaced: a structural path with a shared literal.
fn keys(n: usize) -> Vec<u64> {
    (0..n as u64).collect()
}

/// Probes in a deterministic shuffle. A single fixed probe, or probing in key order, is the
/// largest harness effect in this suite: it moved a `BTreeMap` point get by 3.4x at 131,072
/// keys, and it flatters whichever structure is laid out that way.
fn shuffled(n: usize) -> Vec<u64> {
    let mut out: Vec<u64> = (0..n as u64).collect();
    let mut state: u64 = 0x5eed_1eaf_c0ff_ee01;
    for i in (1..out.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.swap(i, (state % (i as u64 + 1)) as usize);
    }
    out
}

/// What an application writes when it grows a table out of a `Vec`: ordered rows, and a side
/// index it maintains by hand. This is `IndexedTable`'s real baseline.
struct VecPlusIndex {
    rows: Vec<(u64, u64)>,
    index: BTreeMap<u64, usize>,
}

impl VecPlusIndex {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            index: BTreeMap::new(),
        }
    }

    fn insert(&mut self, key: u64, value: u64) {
        let offset = self.rows.len();
        self.rows.push((key, value));
        self.index.insert(key, offset);
    }

    fn select(&self, key: &u64) -> Option<&u64> {
        self.index.get(key).map(|offset| &self.rows[*offset].1)
    }
}

fn bench(c: &mut Criterion) {
    eprintln!(
        "conditions: debug_assertions={} arch={} opt=release",
        cfg!(debug_assertions),
        std::env::consts::ARCH,
    );

    let mut group = c.benchmark_group("wtvec_vs_vec");

    for &n in SIZES {
        let ks = keys(n);
        let probes = shuffled(n);

        let raw: Vec<(u64, u64)> = ks.iter().map(|k| (*k, k * 2)).collect();
        let mut linear = LinearTable::new();
        let mut indexed = IndexedTable::new();
        let mut hand = VecPlusIndex::new();
        for k in &ks {
            linear.insert(*k, k * 2).expect("unique");
            indexed.insert(*k, k * 2).expect("unique");
            hand.insert(*k, k * 2);
        }

        // Every arm must find the same answer before any of them is timed. A lookup that
        // returns nothing is fast, and that is how a benchmark ends up timing a no-op.
        for k in probes.iter().take(16) {
            let want = Some(k * 2);
            assert_eq!(
                raw.iter().find(|(rk, _)| rk == k).map(|(_, v)| *v),
                want,
                "raw"
            );
            assert_eq!(linear.select(k).copied(), want, "linear_table");
            assert_eq!(indexed.select(k).copied(), want, "indexed_table");
            assert_eq!(hand.select(k).copied(), want, "vec_plus_index");
        }

        // Pair one: no index on either side.
        group.bench_with_input(BenchmarkId::new("raw_vec_scan", n), &n, |b, _| {
            let mut i = 0usize;
            b.iter(|| {
                i = (i + 1) % probes.len();
                let k = probes[i];
                black_box(raw.iter().find(|(rk, _)| *rk == k).map(|(_, v)| *v))
            })
        });
        group.bench_with_input(BenchmarkId::new("linear_table", n), &n, |b, _| {
            let mut i = 0usize;
            b.iter(|| {
                i = (i + 1) % probes.len();
                black_box(linear.select(&probes[i]).copied())
            })
        });

        // Pair two: an index on both sides. This is the pair the claim is about.
        group.bench_with_input(BenchmarkId::new("raw_vec_plus_index", n), &n, |b, _| {
            let mut i = 0usize;
            b.iter(|| {
                i = (i + 1) % probes.len();
                black_box(hand.select(&probes[i]).copied())
            })
        });
        group.bench_with_input(BenchmarkId::new("indexed_table", n), &n, |b, _| {
            let mut i = 0usize;
            b.iter(|| {
                i = (i + 1) % probes.len();
                black_box(indexed.select(&probes[i]).copied())
            })
        });

        // The null. Byte-identical to `raw_vec_scan`. Read the gap between these two before
        // reading any other gap; a 1-2 ns claim is only legible if this is well under 1 ns.
        group.bench_with_input(BenchmarkId::new("raw_vec_scan_null", n), &n, |b, _| {
            let mut i = 0usize;
            b.iter(|| {
                i = (i + 1) % probes.len();
                let k = probes[i];
                black_box(raw.iter().find(|(rk, _)| *rk == k).map(|(_, v)| *v))
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
