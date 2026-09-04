use std::hint::black_box;
use std::process::Command;
use std::time::{Duration, Instant};

use worktable_vec::IndexedTable;
use wt_benchmarks::moe_resident::{
    MoeResidentOriginRow, MoeResidentOriginWorkTable, OriginValue, logical_rows,
};

const ROWS: usize = 1_528;
const POPULATIONS_PER_SAMPLE: u32 = 8;
const WARMUPS: usize = 8;
const SAMPLES: usize = 31;

#[derive(Clone, Copy)]
enum Arm {
    Btree,
    PerRowBlockOn,
    OneExecutor,
    InsertMany,
}

impl Arm {
    const ALL: [Self; 4] = [
        Self::Btree,
        Self::PerRowBlockOn,
        Self::OneExecutor,
        Self::InsertMany,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Btree => 0,
            Self::PerRowBlockOn => 1,
            Self::OneExecutor => 2,
            Self::InsertMany => 3,
        }
    }
}

fn worktable_row(key: u64, value: OriginValue) -> MoeResidentOriginRow {
    MoeResidentOriginRow {
        origin_key: key,
        source: value.source,
        ordinal: value.ordinal,
        example: value.example,
        path_count: value.path_count,
        origin_count: value.origin_count,
    }
}

fn measure(
    arm: Arm,
    rows: &[(u64, OriginValue)],
    worktable_rows: &[MoeResidentOriginRow],
) -> Duration {
    let start = Instant::now();
    for _ in 0..POPULATIONS_PER_SAMPLE {
        measure_population(arm, rows, worktable_rows);
    }
    start.elapsed()
}

fn measure_population(
    arm: Arm,
    rows: &[(u64, OriginValue)],
    worktable_rows: &[MoeResidentOriginRow],
) -> Duration {
    let start = Instant::now();
    match arm {
        Arm::Btree => {
            let mut table = IndexedTable::with_capacity(rows.len());
            for (key, value) in rows.iter().copied() {
                table.insert(key, value).expect("unique origin");
            }
            black_box(table);
        }
        Arm::PerRowBlockOn => {
            let table = MoeResidentOriginWorkTable::default();
            for row in worktable_rows.iter().cloned() {
                futures::executor::block_on(table.insert(row)).expect("unique origin");
            }
            black_box(table);
        }
        Arm::OneExecutor => {
            let table = MoeResidentOriginWorkTable::default();
            futures::executor::block_on(async {
                for row in worktable_rows.iter().cloned() {
                    table.insert(row).await.expect("unique origin");
                }
            });
            black_box(table);
        }
        Arm::InsertMany => {
            let table = MoeResidentOriginWorkTable::default();
            futures::executor::block_on(table.insert_many(worktable_rows.to_vec()))
                .expect("unique origins");
            black_box(table);
        }
    }
    start.elapsed()
}

fn report(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let ns_per_row = |sample: Duration| {
        sample.as_secs_f64() * 1e9 / (ROWS as f64 * f64::from(POPULATIONS_PER_SAMPLE))
    };
    let ms = |sample: Duration| sample.as_secs_f64() * 1e3 / f64::from(POPULATIONS_PER_SAMPLE);
    println!(
        "{label:<24} median={:>8.3} ms {:>8.1} ns/row  min={:>8.3} p25={:>8.3} p75={:>8.3} max={:>8.3}",
        ms(samples[SAMPLES / 2]),
        ns_per_row(samples[SAMPLES / 2]),
        ms(samples[0]),
        ms(samples[SAMPLES / 4]),
        ms(samples[SAMPLES * 3 / 4]),
        ms(samples[SAMPLES - 1]),
    );
}

fn report_cold(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let ns_per_row = |sample: Duration| sample.as_secs_f64() * 1e9 / ROWS as f64;
    let ms = |sample: Duration| sample.as_secs_f64() * 1e3;
    println!(
        "{label:<24} median={:>8.3} ms {:>8.1} ns/row  min={:>8.3} p25={:>8.3} p75={:>8.3} max={:>8.3}",
        ms(samples[SAMPLES / 2]),
        ns_per_row(samples[SAMPLES / 2]),
        ms(samples[0]),
        ms(samples[SAMPLES / 4]),
        ms(samples[SAMPLES * 3 / 4]),
        ms(samples[SAMPLES - 1]),
    );
}

fn rows() -> (Vec<(u64, OriginValue)>, Vec<MoeResidentOriginRow>) {
    let rows = logical_rows(ROWS);
    let worktable_rows = rows
        .iter()
        .copied()
        .map(|(key, value)| worktable_row(key, value))
        .collect();
    (rows, worktable_rows)
}

fn cold_child(arm_index: usize) {
    let arm = Arm::ALL[arm_index];
    let (rows, worktable_rows) = rows();
    println!(
        "{}",
        measure_population(arm, &rows, &worktable_rows).as_nanos()
    );
}

fn cold_samples() -> [Vec<Duration>; 4] {
    let executable = std::env::current_exe().expect("current benchmark executable");
    let mut samples: [Vec<Duration>; 4] = std::array::from_fn(|_| Vec::with_capacity(SAMPLES));
    for round in 0..SAMPLES {
        for offset in 0..Arm::ALL.len() {
            let arm = Arm::ALL[(round + offset) % Arm::ALL.len()];
            let output = Command::new(&executable)
                .arg("--cold-child")
                .arg(arm.index().to_string())
                .output()
                .expect("run cold construction child");
            assert!(output.status.success(), "cold child failed: {output:?}");
            let nanos: u64 = std::str::from_utf8(&output.stdout)
                .expect("cold child output is UTF-8")
                .trim()
                .parse()
                .expect("cold child output is nanoseconds");
            samples[arm.index()].push(Duration::from_nanos(nanos));
        }
    }
    samples
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--cold-child") {
        cold_child(
            args.get(2)
                .expect("cold child arm")
                .parse()
                .expect("cold child arm index"),
        );
        return;
    }

    let (rows, worktable_rows) = rows();

    for round in 0..WARMUPS {
        for offset in 0..Arm::ALL.len() {
            let arm = Arm::ALL[(round + offset) % Arm::ALL.len()];
            black_box(measure(arm, &rows, &worktable_rows));
        }
    }

    let mut samples: [Vec<Duration>; 4] = std::array::from_fn(|_| Vec::with_capacity(SAMPLES));
    for round in 0..SAMPLES {
        for offset in 0..Arm::ALL.len() {
            let arm = Arm::ALL[(round + offset) % Arm::ALL.len()];
            samples[arm.index()].push(measure(arm, &rows, &worktable_rows));
        }
    }

    println!("MoE resident construction A/B");
    println!(
        "rows={ROWS} populations/sample={POPULATIONS_PER_SAMPLE} warmups={WARMUPS} samples={SAMPLES} order=rotated"
    );
    report("Vec + BTreeMap", std::mem::take(&mut samples[0]));
    report("WT per-row block_on", std::mem::take(&mut samples[1]));
    report("WT one executor", std::mem::take(&mut samples[2]));
    report("WT insert_many", std::mem::take(&mut samples[3]));

    println!();
    println!("independent-process first population");
    let mut cold = cold_samples();
    report_cold("Vec + BTreeMap", std::mem::take(&mut cold[0]));
    report_cold("WT per-row block_on", std::mem::take(&mut cold[1]));
    report_cold("WT one executor", std::mem::take(&mut cold[2]));
    report_cold("WT insert_many", std::mem::take(&mut cold[3]));
}
