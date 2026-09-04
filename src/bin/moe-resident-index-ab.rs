use std::time::Duration;

use wt_benchmarks::moe_resident::{
    build, query_arctic_vec, query_btree, query_keys, query_linear, query_worktable,
    query_worktable_arctic_index, query_worktable_congee, query_worktable_data,
    query_worktable_data_pinned, query_worktable_wti, worktable_arctic_links,
};

const ROWS: usize = 1_528;
const QUERIES: usize = 8_000_000;
const SAMPLES: usize = 9;

#[derive(Clone, Copy)]
enum Arm {
    Linear,
    Btree,
    ArcticVec,
    WorktableArctic,
    WorktableWti,
    WorktableCongee,
}

impl Arm {
    const ALL: [Self; 6] = [
        Self::Linear,
        Self::Btree,
        Self::ArcticVec,
        Self::WorktableArctic,
        Self::WorktableWti,
        Self::WorktableCongee,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Linear => 0,
            Self::Btree => 1,
            Self::ArcticVec => 2,
            Self::WorktableArctic => 3,
            Self::WorktableWti => 4,
            Self::WorktableCongee => 5,
        }
    }
}

fn stats(mut values: Vec<Duration>) -> [Duration; 5] {
    values.sort_unstable();
    [
        values[0],
        values[values.len() / 4],
        values[values.len() / 2],
        values[values.len() * 3 / 4],
        values[values.len() - 1],
    ]
}

fn ns_per_query(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1e9 / QUERIES as f64
}

fn report(label: &str, values: Vec<Duration>) -> Duration {
    let [min, p25, median, p75, max] = stats(values);
    println!(
        "  {label:<18} median={:>8.2} ns  min={:>8.2} p25={:>8.2} p75={:>8.2} max={:>8.2}",
        ns_per_query(median),
        ns_per_query(min),
        ns_per_query(p25),
        ns_per_query(p75),
        ns_per_query(max),
    );
    median
}

fn main() {
    let (tables, _) = build(ROWS);
    let keys = query_keys(ROWS, QUERIES);

    if std::env::args().nth(1).as_deref() == Some("breakdown") {
        let links = worktable_arctic_links(&tables.worktable_arctic, &keys);
        let mut samples: [Vec<Duration>; 4] = std::array::from_fn(|_| Vec::with_capacity(SAMPLES));
        for round in 0..SAMPLES {
            for offset in 0..4 {
                let arm = (round + offset) % 4;
                let measurement = match arm {
                    0 => query_worktable_arctic_index(&tables.worktable_arctic, &keys),
                    1 => query_worktable_data(&tables.worktable_arctic, &links),
                    2 => query_worktable_data_pinned(&tables.worktable_arctic, &links),
                    3 => query_worktable(&tables.worktable_arctic, &keys),
                    _ => unreachable!(),
                };
                std::hint::black_box(measurement.checksum);
                samples[arm].push(measurement.elapsed);
            }
        }

        println!("resident provenance WorkTable+Arctic lookup breakdown");
        println!("rows={ROWS} queries={QUERIES} samples={SAMPLES} order=rotated");
        report("WT Arctic index", std::mem::take(&mut samples[0]));
        report("cell+row", std::mem::take(&mut samples[1]));
        report("outer pin+cell+row", std::mem::take(&mut samples[2]));
        report("full WT Arctic", std::mem::take(&mut samples[3]));
        return;
    }

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

    let mut samples: [Vec<Duration>; 6] = std::array::from_fn(|_| Vec::with_capacity(SAMPLES));
    let mut checksum = None;
    for round in 0..SAMPLES {
        for offset in 0..Arm::ALL.len() {
            let arm = Arm::ALL[(round + offset) % Arm::ALL.len()];
            let measurement = match arm {
                Arm::Linear => query_linear(&tables.linear, &keys),
                Arm::Btree => query_btree(&tables.btree, &keys),
                Arm::ArcticVec => query_arctic_vec(&tables.arctic_vec, &keys),
                Arm::WorktableArctic => query_worktable(&tables.worktable_arctic, &keys),
                Arm::WorktableWti => query_worktable_wti(&tables.worktable_wti, &keys),
                Arm::WorktableCongee => query_worktable_congee(&tables.worktable_congee, &keys),
            };
            if let Some(expected) = checksum {
                assert_eq!(measurement.checksum, expected);
            } else {
                checksum = Some(measurement.checksum);
            }
            samples[arm.index()].push(measurement.elapsed);
        }
    }

    println!("resident provenance point lookup");
    println!("rows={ROWS} queries={QUERIES} samples={SAMPLES} order=rotated");
    println!("all checksums={}", checksum.unwrap());
    println!();
    println!("warm lookup distribution");
    let linear = report("vec linear", std::mem::take(&mut samples[0]));
    let btree = report("vec + btree", std::mem::take(&mut samples[1]));
    let arctic_vec = report("vec + arctic", std::mem::take(&mut samples[2]));
    let worktable = report("worktable arctic", std::mem::take(&mut samples[3]));
    let worktable_wti = report("worktable wti", std::mem::take(&mut samples[4]));
    let worktable_congee = report("worktable congee", std::mem::take(&mut samples[5]));
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
