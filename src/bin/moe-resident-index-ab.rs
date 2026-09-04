use std::time::Duration;

use wt_benchmarks::moe_resident::{
    build, query_arctic_vec, query_btree, query_keys, query_linear, query_worktable,
    query_worktable_congee, query_worktable_wti,
};

const ROWS: usize = 1_528;
const QUERIES: usize = 1_000_000;
const SAMPLES: usize = 9;

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

fn ns_per_query(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1e9 / QUERIES as f64
}

fn main() {
    let (tables, build) = build(ROWS);
    let keys = query_keys(ROWS, QUERIES);

    // Warm every implementation and prove the comparison before timing it.
    let expected = query_linear(&tables.linear, &keys[..10_000]).checksum;
    assert_eq!(
        query_btree(&tables.btree, &keys[..10_000]).checksum,
        expected
    );
    assert_eq!(
        query_arctic_vec(&tables.arctic_vec, &keys[..10_000]).checksum,
        expected
    );
    assert_eq!(
        query_worktable(&tables.worktable_arctic, &keys[..10_000]).checksum,
        expected
    );
    assert_eq!(
        query_worktable_wti(&tables.worktable_wti, &keys[..10_000]).checksum,
        expected
    );
    assert_eq!(
        query_worktable_congee(&tables.worktable_congee, &keys[..10_000]).checksum,
        expected
    );

    let mut linear = Vec::with_capacity(SAMPLES);
    let mut btree = Vec::with_capacity(SAMPLES);
    let mut arctic_vec = Vec::with_capacity(SAMPLES);
    let mut worktable = Vec::with_capacity(SAMPLES);
    let mut worktable_wti = Vec::with_capacity(SAMPLES);
    let mut worktable_congee = Vec::with_capacity(SAMPLES);
    let mut checksum = None;
    for _ in 0..SAMPLES {
        let measurements = [
            query_linear(&tables.linear, &keys),
            query_btree(&tables.btree, &keys),
            query_arctic_vec(&tables.arctic_vec, &keys),
            query_worktable(&tables.worktable_arctic, &keys),
            query_worktable_wti(&tables.worktable_wti, &keys),
            query_worktable_congee(&tables.worktable_congee, &keys),
        ];
        for measurement in measurements {
            if let Some(expected) = checksum {
                assert_eq!(measurement.checksum, expected);
            } else {
                checksum = Some(measurement.checksum);
            }
        }
        linear.push(measurements[0].elapsed);
        btree.push(measurements[1].elapsed);
        arctic_vec.push(measurements[2].elapsed);
        worktable.push(measurements[3].elapsed);
        worktable_wti.push(measurements[4].elapsed);
        worktable_congee.push(measurements[5].elapsed);
    }

    let linear = median(linear);
    let btree = median(btree);
    let arctic_vec = median(arctic_vec);
    let worktable = median(worktable);
    let worktable_wti = median(worktable_wti);
    let worktable_congee = median(worktable_congee);

    println!("resident provenance point lookup");
    println!("rows={ROWS} queries={QUERIES} samples={SAMPLES}");
    println!("all checksums={}", checksum.unwrap());
    println!();
    println!("build (one population)");
    println!(
        "  vec linear       {:>10.3} ms",
        build.linear.as_secs_f64() * 1e3
    );
    println!(
        "  vec + btree      {:>10.3} ms",
        build.btree.as_secs_f64() * 1e3
    );
    println!(
        "  vec + arctic     {:>10.3} ms",
        build.arctic_vec.as_secs_f64() * 1e3
    );
    println!(
        "  worktable arctic {:>10.3} ms",
        build.worktable_arctic.as_secs_f64() * 1e3
    );
    println!(
        "  worktable wti    {:>10.3} ms",
        build.worktable_wti.as_secs_f64() * 1e3
    );
    println!(
        "  worktable congee {:>10.3} ms",
        build.worktable_congee.as_secs_f64() * 1e3
    );
    println!();
    println!("warm median lookup");
    println!("  vec linear       {:>10.2} ns/query", ns_per_query(linear));
    println!("  vec + btree      {:>10.2} ns/query", ns_per_query(btree));
    println!(
        "  vec + arctic     {:>10.2} ns/query",
        ns_per_query(arctic_vec)
    );
    println!(
        "  worktable arctic {:>10.2} ns/query",
        ns_per_query(worktable)
    );
    println!(
        "  worktable wti    {:>10.2} ns/query",
        ns_per_query(worktable_wti)
    );
    println!(
        "  worktable congee {:>10.2} ns/query",
        ns_per_query(worktable_congee)
    );
    println!();
    println!(
        "index-only arctic / btree = {:.3}x (below 1 is faster)",
        arctic_vec.as_secs_f64() / btree.as_secs_f64()
    );
    println!(
        "full WorkTable / vec-arctic = {:.3}x",
        worktable.as_secs_f64() / arctic_vec.as_secs_f64()
    );
    println!(
        "full WorkTable / current linear scan = {:.3}x",
        worktable.as_secs_f64() / linear.as_secs_f64()
    );
    println!(
        "worktable arctic / wti = {:.3}x",
        worktable.as_secs_f64() / worktable_wti.as_secs_f64()
    );
    println!(
        "worktable congee / wti = {:.3}x",
        worktable_congee.as_secs_f64() / worktable_wti.as_secs_f64()
    );
}
