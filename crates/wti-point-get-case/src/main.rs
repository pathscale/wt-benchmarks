//! Is WorkTablesIndex's point get really 2.3x to 3.3x slower than `std`, and if so, why?
//!
//! **The claim being checked.** `benches/arctic_paths.rs` reports, on shuffled probes over
//! keys shaped `fn:unit%06d/loop:%d`:
//!
//! ```text
//!            163      512    8,192  131,072
//!   wti     62.5     84.1    342.7    348.8   ns
//!   std     18.8     30.4    213.0    152.7   ns
//! ```
//!
//! A house index losing a plain lookup to the standard library by 3.3x at 163 keys is
//! either a harness mistake or a defect, and the same file shows WorkTablesIndex *winning*
//! the prefix scan at 163 (45.5 against 62.0 ns), so it is not simply a slower structure.
//!
//! **Method, in order:**
//!
//! 1. A null arm runs identical code under a different label. Its separation from its twin
//!    is the floor. Any difference below the floor is not a result.
//! 2. Arms are interleaved and reshuffled every repetition, so drift lands on all of them.
//! 3. Probes are hoisted, shuffled with the suite's seed, and every arm walks the same
//!    vector in the same order. Probe order is the largest effect in this comparison.
//! 4. The run stops when the budget is spent. Shrink the fixture rather than raise it.
//!
//! **The arms, and the question each one closes:**
//!
//!   std_get / std_get_null      the reference, and the floor
//!   wti_get / wti_get_null      the reported case, and its own floor
//!   wti_contains_key            a second entry point: is `get` specifically wrong?
//!   wti_range_next              `range(k..).next()`: is the range path different?
//!   wti_get_owned_key           `BTreeMap<String, _>`, the key type the crate provides a
//!                               `Borrow<str>` impl for: is `&str` the wrong key type?
//!   wti_get_cap{16,64,256,1024} the same map at four node capacities. 1024 is the default
//!                               and the only one a caller gets without asking.
//!   std_get_u64 / wti_get_u64   the same comparison on integer keys: string-specific or not
//!   flat_partition_point        a plain sorted `Vec<(&str, u64)>` searched with
//!   flat_binary_search          `slice::partition_point` and with `slice::binary_search_by`
//!
//! **The last two arms are the ones that decide it.** At 163 and 512 keys WorkTablesIndex
//! holds the whole population in a single node, so `get` is a `slice::partition_point` over
//! one sorted array plus a trivial outer step. If the hand-written flat search costs the
//! same, the gap is the *shape* of a flat sorted array of `(&str, _)` against a shallow
//! B-tree with inline keys, and nothing in WorkTablesIndex is broken. If the hand-written
//! search is fast and WorkTablesIndex is slow, the cost is in the wrapper and that is a
//! defect with a name.
//!
//! Run: `cargo run --release --bin wti-point-get-case`. Budget with `--budget-secs N`.

use std::collections::BTreeMap as StdMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use indexset::BTreeMap as WtiMap;

/// The seed `wt_benchmarks::rng::PROBE_SHUFFLE_SEED` uses, so this reproducer probes the
/// identical permutation the benches do.
const SEED: u64 = 0x5eed_1eaf_c0ff_ee01;
const SIZES: &[usize] = &[163, 512, 8_192, 131_072];
const NODE_CAPS: &[usize] = &[16, 64, 256, 1_024];
const REPS: usize = 15;
const WARMUP_REPS: usize = 3;
/// Operations per timed repetition, not probes per vector. At 163 keys the probe vector is
/// walked 123 times to reach this; the first version of this file walked it once and then
/// divided by this constant anyway, which understated every small-`n` arm by 100x and left
/// the timed region short enough to report a 143% spread.
const OPS_PER_REP: usize = 20_000;
const DEFAULT_BUDGET_SECS: u64 = 240;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for index in (1..items.len()).rev() {
            let target = (self.next_u64() % (index as u64 + 1)) as usize;
            items.swap(index, target);
        }
    }
}

struct Arm<'a> {
    name: &'static str,
    body: Box<dyn FnMut() -> u64 + 'a>,
    /// Per-repetition ns/op.
    samples: Vec<f64>,
}

impl<'a> Arm<'a> {
    fn new(name: &'static str, body: impl FnMut() -> u64 + 'a) -> Self {
        Self {
            name,
            body: Box::new(body),
            samples: Vec::new(),
        }
    }
}

fn main() {
    let budget = Duration::from_secs(
        std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|pair| pair[0] == "--budget-secs")
            .and_then(|pair| pair[1].parse().ok())
            .unwrap_or(DEFAULT_BUDGET_SECS),
    );
    print_conditions(budget);

    let deadline = Instant::now() + budget;

    for &size in SIZES {
        if Instant::now() >= deadline {
            println!("\n== budget spent before n={size}; remaining sizes not measured ==");
            break;
        }

        let keys: Vec<String> = (0..size)
            .map(|i| format!("fn:unit{:06}/loop:{}", i / 3, i % 3))
            .collect();
        let mut probes: Vec<&str> = keys.iter().map(String::as_str).collect();
        Rng::new(SEED).shuffle(&mut probes);
        let probes = &probes[..OPS_PER_REP.min(probes.len())];
        // How many times an arm walks the probe vector to reach a timed region long enough
        // to swamp `Instant::now`. The population, not the vector, is what varies by size.
        let rounds = OPS_PER_REP.div_ceil(probes.len());
        let ops = rounds * probes.len();

        // Integer keys derived from the same population, so the two integer arms describe
        // this workload rather than an invented one. Same hash `benches/arctic_paths.rs`
        // uses, narrowed to `u64`.
        let ints: Vec<u64> = probes
            .iter()
            .map(|k| {
                let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                for byte in k.as_bytes() {
                    h = (h ^ u64::from(*byte)).wrapping_mul(0x1000_0000_01b3);
                }
                h
            })
            .collect();

        let std_map: StdMap<&str, u64> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i as u64))
            .collect();
        let mut wti_map = WtiMap::<&str, u64>::new();
        let mut wti_owned = WtiMap::<String, u64>::new();
        for (index, key) in keys.iter().enumerate() {
            wti_map.insert(key.as_str(), index as u64);
            wti_owned.insert(key.clone(), index as u64);
        }

        let capped: Vec<(usize, WtiMap<&str, u64>)> = NODE_CAPS
            .iter()
            .map(|&cap| {
                let mut map = WtiMap::<&str, u64>::with_maximum_node_size(cap);
                for (index, key) in keys.iter().enumerate() {
                    map.insert(key.as_str(), index as u64);
                }
                (cap, map)
            })
            .collect();

        let std_ints: StdMap<u64, u64> = ints
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, i as u64))
            .collect();
        let mut wti_ints = WtiMap::<u64, u64>::new();
        for (index, key) in ints.iter().enumerate() {
            wti_ints.insert(*key, index as u64);
        }

        // Every arm must find every probe before any of it is timed. A miss is a different
        // code path in both structures, and a map that quietly lost rows is a faster map.
        assert_eq!(
            probes.iter().filter(|k| std_map.contains_key(**k)).count(),
            probes.len()
        );
        assert_eq!(
            probes.iter().filter(|k| wti_map.get(**k).is_some()).count(),
            probes.len()
        );
        assert_eq!(
            probes
                .iter()
                .filter(|k| wti_owned.get(**k).is_some())
                .count(),
            probes.len()
        );
        assert_eq!(
            ints.iter().filter(|k| wti_ints.get(k).is_some()).count(),
            ints.len()
        );
        for (cap, map) in &capped {
            assert_eq!(
                probes.iter().filter(|k| map.get(**k).is_some()).count(),
                probes.len(),
                "node capacity {cap} lost rows",
            );
        }

        // The single-node control: the same population as one sorted flat array. At 163 and
        // 512 keys this *is* the shape of a WorkTablesIndex node, so the two should agree.
        let mut flat: Vec<(&str, u64)> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i as u64))
            .collect();
        flat.sort_unstable_by(|left, right| left.0.cmp(right.0));

        let mut arms = vec![
            Arm::new("std_get", || std_point(&std_map, probes, rounds)),
            Arm::new("std_get_null", || std_point(&std_map, probes, rounds)),
            Arm::new("wti_get", || wti_point(&wti_map, probes, rounds)),
            Arm::new("wti_get_null", || wti_point(&wti_map, probes, rounds)),
            Arm::new("wti_contains_key", || {
                let mut hits = 0u64;
                for _ in 0..rounds {
                    for key in probes {
                        hits += u64::from(black_box(wti_map.contains_key(*key)));
                    }
                }
                hits
            }),
            Arm::new("wti_range_next", || {
                let mut hits = 0u64;
                for _ in 0..rounds {
                    for key in probes {
                        hits += black_box(
                            wti_map
                                .range(*key..)
                                .next()
                                .map(|(_, value)| *value)
                                .unwrap_or(0),
                        );
                    }
                }
                hits
            }),
            Arm::new("wti_get_owned_key", || {
                let mut hits = 0u64;
                for _ in 0..rounds {
                    for key in probes {
                        hits += black_box(wti_owned.get(*key).copied().unwrap_or(0));
                    }
                }
                hits
            }),
            Arm::new("std_get_u64", || {
                let mut hits = 0u64;
                for _ in 0..rounds {
                    for key in &ints {
                        hits += black_box(std_ints.get(key).copied().unwrap_or(0));
                    }
                }
                hits
            }),
            Arm::new("wti_get_u64", || {
                let mut hits = 0u64;
                for _ in 0..rounds {
                    for key in &ints {
                        hits += black_box(wti_ints.get(key).copied().unwrap_or(0));
                    }
                }
                hits
            }),
            // `partition_point` never exits early on equality, which is exactly what
            // `BTreeMap::get_key_value` does inside a node.
            Arm::new("flat_partition_point", || {
                let mut hits = 0u64;
                for _ in 0..rounds {
                    for key in probes {
                        let index = flat.partition_point(|(candidate, _)| *candidate < *key);
                        hits += black_box(match flat.get(index) {
                            Some((candidate, value)) if candidate == key => *value,
                            _ => 0,
                        });
                    }
                }
                hits
            }),
            // The same array with an early-exit search, which is what the crate's own
            // `search_backend` does for the paths that reach it.
            Arm::new("flat_binary_search", || {
                let mut hits = 0u64;
                for _ in 0..rounds {
                    for key in probes {
                        hits += black_box(
                            flat.binary_search_by(|(candidate, _)| (*candidate).cmp(*key))
                                .map(|index| flat[index].1)
                                .unwrap_or(0),
                        );
                    }
                }
                hits
            }),
        ];

        // The node-capacity sweep. Labels are static because `Arm` names are, and the
        // capacities are a fixed list; index into it rather than formatting a string.
        const CAP_LABELS: [&str; 4] = [
            "wti_get_cap16",
            "wti_get_cap64",
            "wti_get_cap256",
            "wti_get_cap1024",
        ];
        for (slot, (_, map)) in capped.iter().enumerate() {
            arms.push(Arm::new(CAP_LABELS[slot], move || {
                wti_point(map, probes, rounds)
            }));
        }

        run(&mut arms, deadline, ops);
        report(size, probes.len(), rounds, ops, &mut arms);
    }
}

fn print_conditions(budget: Duration) {
    println!("== conditions ==");
    println!("debug_assertions: {}", cfg!(debug_assertions));
    println!(
        "arch: {} os: {}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    println!("profile: release (lto=fat, codegen-units=1)");
    println!("WorkTablesIndex default features (wt-slice-binary-search), sequential map");
    println!("key shape: fn:unit%06d/loop:%d, probes shuffled with seed {SEED:#x}");
    println!("sizes: {SIZES:?}  node capacities: {NODE_CAPS:?}");
    println!("reps: {REPS} (+{WARMUP_REPS} discarded)  target ops/rep: {OPS_PER_REP}");
    println!("budget: {} s", budget.as_secs());
}

/// Interleave the arms: every repetition runs all of them, in a fresh order.
fn run(arms: &mut [Arm<'_>], deadline: Instant, ops: usize) {
    let mut rng = Rng::new(SEED ^ 0x9e37_79b9_7f4a_7c15);
    let mut order: Vec<usize> = (0..arms.len()).collect();

    for rep in 0..REPS + WARMUP_REPS {
        if Instant::now() >= deadline {
            break;
        }
        rng.shuffle(&mut order);
        for index in order.iter().copied() {
            let started = Instant::now();
            let checksum = (arms[index].body)();
            let elapsed = started.elapsed();
            black_box(checksum);
            if rep >= WARMUP_REPS {
                arms[index]
                    .samples
                    .push(elapsed.as_nanos() as f64 / ops as f64);
            }
        }
    }
}

fn std_point(map: &StdMap<&str, u64>, probes: &[&str], rounds: usize) -> u64 {
    let mut hits = 0u64;
    for _ in 0..rounds {
        for key in probes {
            hits += black_box(map.get(*key).copied().unwrap_or(0));
        }
    }
    hits
}

fn wti_point(map: &WtiMap<&str, u64>, probes: &[&str], rounds: usize) -> u64 {
    let mut hits = 0u64;
    for _ in 0..rounds {
        for key in probes {
            hits += black_box(map.get(*key).copied().unwrap_or(0));
        }
    }
    hits
}

fn report(size: usize, probes: usize, rounds: usize, ops: usize, arms: &mut [Arm<'_>]) {
    println!(
        "\n== n={size}, {probes} distinct probes x {rounds} rounds = {ops} ops per repetition =="
    );
    println!(
        "{:<20} {:>10} {:>10} {:>10} {:>8}",
        "arm", "median", "p10", "p90", "spread"
    );

    let mut medians = Vec::new();
    for arm in arms.iter_mut() {
        arm.samples.sort_by(f64::total_cmp);
        if arm.samples.is_empty() {
            println!("{:<20} {:>10}", arm.name, "no data");
            continue;
        }
        let median = quantile(&arm.samples, 0.5);
        println!(
            "{:<20} {median:>10.2} {:>10.2} {:>10.2} {:>7.1}%",
            arm.name,
            quantile(&arm.samples, 0.10),
            quantile(&arm.samples, 0.90),
            spread_pct(&arm.samples),
        );
        medians.push((arm.name, median));
    }

    let lookup = |name: &str| medians.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
    if let (Some(std_get), Some(std_null)) = (lookup("std_get"), lookup("std_get_null")) {
        let floor = (std_get - std_null).abs() / std_get.min(std_null) * 100.0;
        println!("null floor (std_get vs std_get_null): {floor:.1}%");
    }
    if let (Some(wti_get), Some(wti_null)) = (lookup("wti_get"), lookup("wti_get_null")) {
        let floor = (wti_get - wti_null).abs() / wti_get.min(wti_null) * 100.0;
        println!("null floor (wti_get vs wti_get_null): {floor:.1}%");
    }
    if let (Some(std_get), Some(wti_get)) = (lookup("std_get"), lookup("wti_get")) {
        println!("wti_get / std_get: {:.2}x", wti_get / std_get);
    }
    if let (Some(std_int), Some(wti_int)) = (lookup("std_get_u64"), lookup("wti_get_u64")) {
        println!("wti_get_u64 / std_get_u64: {:.2}x", wti_int / std_int);
    }
    if let (Some(default), Some(best)) = (lookup("wti_get_cap1024"), lookup("wti_get_cap64")) {
        println!("wti_get_cap1024 / wti_get_cap64: {:.2}x", default / best);
    }
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let position = q * (sorted.len() - 1) as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    if low == high {
        return sorted[low];
    }
    sorted[low] + (sorted[high] - sorted[low]) * (position - low as f64)
}

fn spread_pct(sorted: &[f64]) -> f64 {
    let median = quantile(sorted, 0.5);
    if median == 0.0 {
        return f64::NAN;
    }
    (quantile(sorted, 0.90) - quantile(sorted, 0.10)) / median * 100.0
}
