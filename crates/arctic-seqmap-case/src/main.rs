//! Isolates the claim that `std::collections::BTreeMap` beats `arctic-wt`'s
//! `SequentialMap` by about 2x on keys shaped `fn:<unit>/loop:<i>`.
//!
//! Drive every condition from `run.sh`, which builds the four arms and labels
//! them. Run this binary directly only when pinning one condition:
//!
//! ```text
//! cargo run --release --bin arctic-seqmap-case
//! cargo run --release --features arctic-validate --bin arctic-seqmap-case
//! cargo run --release --features arctic-smr      --bin arctic-seqmap-case
//! ```
//!
//! Method, in order:
//!
//! 1. A null arm runs identical code against itself under a different label.
//!    Its separation from its twin is the floor. Any A/B difference below the
//!    floor is not a result.
//! 2. Key construction is hoisted out of every arm that claims to measure a
//!    map, and measured on its own in two separate arms.
//! 3. Arms are interleaved and their order is reshuffled every repetition, so
//!    thermal or frequency drift lands on all contenders equally.
//! 4. The run stops when the budget is spent. Shrink the fixture rather than
//!    raise the budget.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::ops::Bound;
use std::time::Duration;
use std::time::Instant;

use arctic::Order;
use arctic::key::BoxedStr;
use arctic::key::NonNull;
use arctic::key::Str;
use arctic::sequential;
use arctic_seqmap_case::Fixture;
use arctic_seqmap_case::Rng;
use arctic_seqmap_case::median;
use arctic_seqmap_case::quantile;

const SEED: u64 = 0x5eed_1eaf_c0ff_ee01;
const SIZES: &[usize] = &[163, 354, 512, 8_192, 32_768, 131_072];
const REPS: usize = 21;
const WARMUP_REPS: usize = 3;
const PROBES: usize = 50_000;
const DEFAULT_BUDGET_SECS: u64 = 180;

type ArcticMap = sequential::Map<BoxedStr<NonNull>, u64>;

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
            samples: Vec::with_capacity(REPS),
        }
    }
}

fn main() {
    let budget = std::env::args()
        .skip_while(|argument| argument != "--budget-secs")
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(DEFAULT_BUDGET_SECS));
    let jsonl = std::env::args().any(|argument| argument == "--jsonl");

    print_conditions(budget);

    let started = Instant::now();
    let mut raw: Vec<String> = Vec::new();

    for size in SIZES.iter().copied() {
        if started.elapsed() >= budget {
            println!(
                "\nBUDGET SPENT after {:.1}s: sizes from {size} on were not run.",
                started.elapsed().as_secs_f64()
            );
            break;
        }

        let fixture = Fixture::build(size, SEED);
        let arctic_keys = fixture.arctic_probe_keys();
        let btree_keys = fixture.btree_probe_keys();
        let arctic_ordered = fixture.arctic_keys_in_order();
        let btree_ordered = fixture.btree_keys_in_order();
        let prefix_keys = fixture.prefix_probe_keys();

        {
            let arctic = &fixture.arctic;
            let btree = &fixture.btree;
            let mut point = vec![
                Arm::new("arctic_get", || arctic_point(arctic, &arctic_keys)),
                Arm::new("arctic_get_NULL", || arctic_point(arctic, &arctic_keys)),
                Arm::new("btree_get", || btree_point(btree, &btree_keys)),
                Arm::new("btree_get_NULL", || btree_point(btree, &btree_keys)),
                // Hypothesis 3: is the original measurement timing key handling?
                Arm::new("arctic_get_validate_inline", || {
                    arctic_point_validate_inline(arctic, &btree_keys)
                }),
                Arm::new("arctic_get_unchecked_inline", || {
                    arctic_point_unchecked_inline(arctic, &btree_keys)
                }),
                // Probe-order axis: shuffled above, lexicographic here. Same maps,
                // same operation, same count.
                Arm::new("arctic_get_inorder", || {
                    arctic_point(arctic, &arctic_ordered)
                }),
                Arm::new("btree_get_inorder", || btree_point(btree, &btree_ordered)),
                Arm::new("keybuild_validate_only", || validate_only(&btree_keys)),
            ];
            run(&mut point);

            let mut scan = vec![
                Arm::new("arctic_prefix_values", || {
                    arctic_prefix(arctic, &prefix_keys)
                }),
                Arm::new("arctic_prefix_values_NULL", || {
                    arctic_prefix(arctic, &prefix_keys)
                }),
                Arm::new("btree_range_take_while", || {
                    btree_prefix(btree, &prefix_keys)
                }),
                Arm::new("btree_range_take_while_NULL", || {
                    btree_prefix(btree, &prefix_keys)
                }),
                // Same two scans with one `Vec` collected per probe. A scan
                // harness that materialises its result is measuring the
                // allocator as much as the map; these arms price that.
                Arm::new("arctic_prefix_collect", || {
                    arctic_prefix_collect(arctic, &prefix_keys)
                }),
                Arm::new("btree_range_collect", || {
                    btree_prefix_collect(btree, &prefix_keys)
                }),
            ];
            run(&mut scan);

            report(size, fixture.keys.len(), "point get", &mut point);
            report(size, fixture.keys.len(), "prefix scan", &mut scan);

            if jsonl {
                for arm in point.iter().chain(scan.iter()) {
                    for (rep, ns) in arm.samples.iter().enumerate() {
                        raw.push(format!(
                            "{{\"keys\":{},\"arm\":\"{}\",\"rep\":{rep},\"ns_per_op\":{ns:.4}}}",
                            fixture.keys.len(),
                            arm.name
                        ));
                    }
                }
            }
        }
    }

    println!(
        "\nTotal wall clock: {:.1}s",
        started.elapsed().as_secs_f64()
    );

    if jsonl {
        println!("\n--- raw per-repetition samples (JSONL) ---");
        for line in raw {
            println!("{line}");
        }
    }
}

fn print_conditions(budget: Duration) {
    println!("arctic-wt SequentialMap vs BTreeMap - isolating conditions");
    println!("=========================================================");
    println!("target triple      : {}", env!("CASE_TARGET"));
    println!("rustc              : {}", env!("CASE_RUSTC"));
    println!("profile            : {}", env!("CASE_PROFILE"));
    println!("opt-level          : {}", env!("CASE_OPT_LEVEL"));
    println!("rustflags          : {:?}", env!("CASE_RUSTFLAGS"));
    println!(
        "debug_assertions   : {} (this crate; a RUSTFLAGS -C debug-assertions reaches arctic-wt too)",
        cfg!(debug_assertions)
    );
    println!(
        "arctic-wt features : validate={} smr-ps-reclaim={} (default features OFF in Cargo.toml)",
        cfg!(feature = "arctic-validate"),
        cfg!(feature = "arctic-smr")
    );
    println!(
        "arctic-wt asserts  : {} (validate! / validate_eq! live inside the data structure)",
        cfg!(feature = "arctic-validate") || cfg!(debug_assertions)
    );
    println!("key shape          : fn:<unit>/loop:<i>, SYNTHETIC unit names, fixed width");
    println!("density            : 354 regions over 163 units, as ekoctl mem reported");
    println!("seed               : {SEED:#x}");
    println!(
        "reps               : {REPS} measured, {WARMUP_REPS} discarded warmup, {PROBES} ops each"
    );
    println!("budget             : {}s", budget.as_secs());
    println!("allocator          : system");
}

/// Interleave the arms: every repetition runs all of them, in a fresh order.
fn run(arms: &mut [Arm<'_>]) {
    let mut rng = Rng::new(SEED ^ 0x9e37_79b9_7f4a_7c15);
    let mut order: Vec<usize> = (0..arms.len()).collect();
    for rep in 0..(WARMUP_REPS + REPS) {
        rng.shuffle(&mut order);
        for index in order.iter().copied() {
            let start = Instant::now();
            let checksum = (arms[index].body)();
            let elapsed = start.elapsed();
            black_box(checksum);
            if rep >= WARMUP_REPS {
                arms[index]
                    .samples
                    .push(elapsed.as_secs_f64() * 1e9 / PROBES as f64);
            }
        }
    }
}

#[inline(never)]
fn arctic_point(map: &ArcticMap, keys: &[&Str<NonNull>]) -> u64 {
    let keys = black_box(keys);
    let mut sum = 0u64;
    for key in keys.iter().copied().cycle().take(PROBES) {
        sum = sum.wrapping_add(map.get(key).copied().unwrap_or(0));
    }
    sum
}

#[inline(never)]
fn btree_point(map: &BTreeMap<String, u64>, keys: &[&str]) -> u64 {
    // The barrier goes on the slice, once, rather than on each key: a `&str`
    // is two words and a `&Str<NonNull>` is one, so a per-iteration
    // `black_box` on the key would tax the two arms differently. The sum is
    // returned and consumed, so no lookup can be elided.
    let keys = black_box(keys);
    let mut sum = 0u64;
    for key in keys.iter().copied().cycle().take(PROBES) {
        sum = sum.wrapping_add(map.get(key).copied().unwrap_or(0));
    }
    sum
}

/// The unfair shape: validate the key inside the timed region while the
/// `BTreeMap` arm compares a plain `&str`.
#[inline(never)]
fn arctic_point_validate_inline(map: &ArcticMap, keys: &[&str]) -> u64 {
    let mut sum = 0u64;
    for key in keys.iter().copied().cycle().take(PROBES) {
        let validated = Str::<NonNull>::new(black_box(key)).expect("No null byte");
        sum = sum.wrapping_add(map.get(validated).copied().unwrap_or(0));
    }
    sum
}

/// The same shape with the validation skipped, so the scan for a null byte can
/// be priced on its own.
#[inline(never)]
fn arctic_point_unchecked_inline(map: &ArcticMap, keys: &[&str]) -> u64 {
    let mut sum = 0u64;
    for key in keys.iter().copied().cycle().take(PROBES) {
        // SAFETY: the fixture generates ASCII keys with no null byte.
        let validated = unsafe { Str::<NonNull>::new_unchecked(black_box(key)) };
        sum = sum.wrapping_add(map.get(validated).copied().unwrap_or(0));
    }
    sum
}

/// Key validation with no map at all, so the arms above can be decomposed.
#[inline(never)]
fn validate_only(keys: &[&str]) -> u64 {
    let mut sum = 0u64;
    for key in keys.iter().copied().cycle().take(PROBES) {
        let validated = Str::<NonNull>::new(black_box(key)).expect("No null byte");
        sum = sum.wrapping_add(black_box(validated).as_str().len() as u64);
    }
    sum
}

#[inline(never)]
fn arctic_prefix(map: &ArcticMap, prefixes: &[&str]) -> u64 {
    let mut sum = 0u64;
    for prefix in prefixes.iter().copied().cycle().take(PROBES) {
        let shard = map.prefix(black_box(prefix).into());
        for value in shard.values(Order::Ascend) {
            sum = sum.wrapping_add(*value);
        }
    }
    sum
}

#[inline(never)]
fn btree_prefix(map: &BTreeMap<String, u64>, prefixes: &[&str]) -> u64 {
    let mut sum = 0u64;
    for prefix in prefixes.iter().copied().cycle().take(PROBES) {
        let prefix = black_box(prefix);
        for (_, value) in map
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            sum = sum.wrapping_add(*value);
        }
    }
    sum
}

#[inline(never)]
fn arctic_prefix_collect(map: &ArcticMap, prefixes: &[&str]) -> u64 {
    let mut sum = 0u64;
    for prefix in prefixes.iter().copied().cycle().take(PROBES) {
        let shard = map.prefix(black_box(prefix).into());
        let found = shard.values(Order::Ascend).copied().collect::<Vec<u64>>();
        sum = sum.wrapping_add(black_box(&found).iter().sum::<u64>());
    }
    sum
}

#[inline(never)]
fn btree_prefix_collect(map: &BTreeMap<String, u64>, prefixes: &[&str]) -> u64 {
    let mut sum = 0u64;
    for prefix in prefixes.iter().copied().cycle().take(PROBES) {
        let prefix = black_box(prefix);
        let found = map
            .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(_, value)| *value)
            .collect::<Vec<u64>>();
        sum = sum.wrapping_add(black_box(&found).iter().sum::<u64>());
    }
    sum
}

fn report(size: usize, actual: usize, operation: &str, arms: &mut [Arm<'_>]) {
    arms.sort_by_key(|arm| arm.name);
    println!("\n[{operation}] requested {size} keys, loaded {actual}");
    println!(
        "  {:<30} {:>10} {:>10} {:>10} {:>10}",
        "arm", "median", "p25", "p75", "max-min"
    );
    for arm in arms.iter() {
        println!(
            "  {:<30} {:>9.1}n {:>9.1}n {:>9.1}n {:>9.1}%",
            arm.name,
            median(&arm.samples),
            quantile(&arm.samples, 0.25),
            quantile(&arm.samples, 0.75),
            spread_pct(&arm.samples),
        );
    }

    for (real, null) in pairs(arms) {
        let floor =
            (median(&null.samples) - median(&real.samples)).abs() / median(&real.samples) * 100.0;
        println!(
            "  NULL FLOOR {:<26} {:>6.2}% apart between identical code, worst within-arm spread {:.2}%",
            real.name,
            floor,
            spread_pct(&real.samples).max(spread_pct(&null.samples))
        );
    }
}

fn pairs<'a>(arms: &'a [Arm<'_>]) -> Vec<(&'a Arm<'a>, &'a Arm<'a>)> {
    arms.iter()
        .filter(|arm| arm.name.ends_with("_NULL"))
        .filter_map(|null| {
            let base = null.name.trim_end_matches("_NULL");
            arms.iter()
                .find(|arm| arm.name == base)
                .map(|real| (real, null))
        })
        .collect()
}

fn spread_pct(samples: &[f64]) -> f64 {
    let med = median(samples);
    let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
    let max = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (max - min) / med * 100.0
}
