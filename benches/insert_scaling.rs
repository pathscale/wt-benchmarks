//! Insert throughput against concurrent writer count.
//!
//!   cargo bench --bench insert_scaling
//!
//! The finding is the shape, not any single number: throughput is flat to four
//! writers and falls off a cliff at eight. Read the `vs 1` column.
//!
//! A number below 1.00x means adding writers made the table slower in
//! aggregate than a single thread. That was 0.25x at eight writers before
//! `EmptyLinkRegistry::pop_max` stopped taking a global mutex on every insert,
//! and 0.66x after. What remains is `DataPages::insert` taking an exclusive
//! write lock on the one page named by `current_page_id`: appends serialise by
//! construction, so this cannot reach 1.00x until writers get their own append
//! targets.
//!
//! Release only. In a debug build the per-operation overhead swamps the
//! contention and the curve inverts, reporting eight writers as *faster* than
//! one.

use wt_benchmarks::op_latency::scaling::{ROWS, SWEEP, throughput};

fn main() {
    assert!(
        !cfg!(debug_assertions),
        "run with --release: a debug build hides the contention and inverts the curve"
    );

    println!("insert throughput, {ROWS} rows per arm, best of three");
    println!("{:>8} {:>16} {:>14} {:>9}", "writers", "throughput", "per insert", "vs 1");
    println!("{}", "-".repeat(52));

    let mut single = 0.0f64;
    for &writers in &SWEEP {
        let rate = throughput(writers);
        if writers == 1 {
            single = rate;
        }
        println!(
            "{writers:>8} {rate:>13.0} /s {:>11.0} ns {:>8.2}x",
            1e9 / rate,
            rate / single
        );
    }
}
