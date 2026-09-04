//! Does vacuum stay out of the way, and does it still return everything?
//!
//! The other suite asks what a sweep costs while it runs. This one asks the
//! question that decides whether the pacing design works at all: under
//! sustained upsert and delete pressure, does vacuum quietly wait its turn?
//!
//! A sweep that measurably slows the foreground is failing, however much
//! memory it hands back. So is one that waits politely and then never returns
//! the memory. A deliberately unpaced sweep is the positive control: unless
//! that arm is measurably worse than vacuum-off, this workload was not
//! sensitive enough to say anything about the reactive arm. The result is
//! measured in two phases:
//!
//! 1. **Load.** Upserts and deletes as fast as the table will take them, for a
//!    fixed wall-clock window. Vacuum is running and has plenty of reason to:
//!    the deletes are freeing pages the whole time. The measurement is the
//!    foreground's own latency, against the identical arm with vacuum stopped.
//!    Any daylight between them is vacuum interfering, which is a failure.
//!
//! 2. **Quiet.** The load stops. Vacuum should now take its turn and give the
//!    memory back, and the arm stays open until it stops reclaiming. Time to
//!    that point is vacuum's latency; pages still held against what a packed
//!    table would need is its effectiveness. Anything short of a full return
//!    means it exited early.
//!
//! Every mode runs repeatedly in a rotated order. The vacuum-off passes
//! measure the machine's own spread, which is the floor below which an
//! apparent reactive-vacuum cost is noise rather than evidence.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::result::LatencySummary;

/// How long the sweep is given to finish reclaiming once the load stops.
const DRAIN_LIMIT: Duration = Duration::from_secs(20);

/// No further reclamation for this long counts as finished.
const DRAIN_SETTLED: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VacuumMode {
    Off,
    Reactive,
    /// Positive control: hold the vacuum exclusion for the whole pass and do
    /// not consult foreground activity between batches.
    Unpaced,
}

impl VacuumMode {
    fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Reactive => "reactive",
            Self::Unpaced => "unpaced",
        }
    }

    fn is_running(self) -> bool {
        self != Self::Off
    }
}

#[derive(Debug, Serialize)]
pub struct YieldArm {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'static str,
    pub backend: &'static str,
    /// `off` is the null, `reactive` is the shipping policy, and `unpaced` is
    /// the deliberately interfering positive control.
    pub vacuum_mode: &'static str,
    /// Whether vacuum was running during the load phase.
    pub vacuum_running: bool,
    /// Which pass of this configuration this is.
    ///
    /// Every configuration runs repeatedly, including vacuum-off. The
    /// vacuum-off passes are the null: whatever they differ by is what this
    /// machine produces from identical work, and an on-versus-off difference
    /// smaller than that is not evidence of anything. Without it the
    /// interference number cannot gate a release, because the spread between
    /// runs on this machine reaches 30 points while any real interference is
    /// far smaller.
    pub repetition: u32,
    /// Upserts of keys that are already present.
    pub upserts: u64,
    /// Upserts that reinsert a key deleted by an earlier turn.
    pub reinserts: u64,
    pub deletes: u64,
    /// Foreground latency during the load. The comparison against the
    /// vacuum-off arm is the whole point: equal means vacuum stayed out of the
    /// way, worse means it did not.
    pub upsert_latency: LatencySummary,
    pub reinsert_latency: LatencySummary,
    pub delete_latency: LatencySummary,
    pub load_elapsed_ns: u128,
    /// Sweeps that ran *during* the load. Should be none: there is pressure
    /// the entire time and vacuum is supposed to defer under it.
    pub sweeps_during_load: u64,
    /// Sweeps that ran once the table went quiet, which is when it should.
    pub sweeps_after_load: u64,
    pub pages_after_load: usize,
    pub pages_after_drain: usize,
    /// Pages a packed table holding `rows_left` would need. Equal to
    /// `pages_after_drain` is a full return.
    pub ideal_pages: usize,
    pub rows_left: u64,
    /// How long after the load stopped until reclamation finished.
    pub drain_ns: u128,
    pub target_arch: &'static str,
    pub target_os: &'static str,
}

pub fn emit(arm: &YieldArm) {
    println!("{}", serde_json::to_string(&arm).expect("arm serialises"));
}

#[derive(Serialize)]
struct YieldModeSummary {
    vacuum_mode: &'static str,
    repetitions: usize,
    operations_min: u64,
    operations_median: u64,
    operations_max: u64,
    upsert_p99_ns_min: u64,
    upsert_p99_ns_median: u64,
    upsert_p99_ns_max: u64,
    reinsert_p99_ns_min: u64,
    reinsert_p99_ns_median: u64,
    reinsert_p99_ns_max: u64,
    delete_p99_ns_min: u64,
    delete_p99_ns_median: u64,
    delete_p99_ns_max: u64,
    exact_reclamation_runs: usize,
    max_excess_pages: usize,
    sweeps_during_load: u64,
}

#[derive(Serialize)]
struct YieldSummary {
    schema_version: u32,
    suite: &'static str,
    engine: &'static str,
    backend: &'static str,
    repetitions: usize,
    modes: Vec<YieldModeSummary>,
    target_arch: &'static str,
    target_os: &'static str,
}

fn range_and_median(mut values: Vec<u64>) -> (u64, u64, u64) {
    values.sort_unstable();
    let midpoint = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        values[midpoint - 1] + (values[midpoint] - values[midpoint - 1]) / 2
    } else {
        values[midpoint]
    };
    (values[0], median, values[values.len() - 1])
}

fn summarize_mode(arms: &[&YieldArm], mode: VacuumMode) -> YieldModeSummary {
    let matching: Vec<_> = arms
        .iter()
        .copied()
        .filter(|arm| arm.vacuum_mode == mode.name())
        .collect();
    let (operations_min, operations_median, operations_max) =
        range_and_median(matching.iter().map(|arm| arm.upserts).collect());
    let (upsert_p99_ns_min, upsert_p99_ns_median, upsert_p99_ns_max) = range_and_median(
        matching
            .iter()
            .map(|arm| arm.upsert_latency.p99_ns.expect("load produced samples"))
            .collect(),
    );
    let (reinsert_p99_ns_min, reinsert_p99_ns_median, reinsert_p99_ns_max) = range_and_median(
        matching
            .iter()
            .map(|arm| arm.reinsert_latency.p99_ns.expect("load produced samples"))
            .collect(),
    );
    let (delete_p99_ns_min, delete_p99_ns_median, delete_p99_ns_max) = range_and_median(
        matching
            .iter()
            .map(|arm| arm.delete_latency.p99_ns.expect("load produced samples"))
            .collect(),
    );
    YieldModeSummary {
        vacuum_mode: mode.name(),
        repetitions: matching.len(),
        operations_min,
        operations_median,
        operations_max,
        upsert_p99_ns_min,
        upsert_p99_ns_median,
        upsert_p99_ns_max,
        reinsert_p99_ns_min,
        reinsert_p99_ns_median,
        reinsert_p99_ns_max,
        delete_p99_ns_min,
        delete_p99_ns_median,
        delete_p99_ns_max,
        exact_reclamation_runs: matching
            .iter()
            .filter(|arm| arm.pages_after_drain == arm.ideal_pages)
            .count(),
        max_excess_pages: matching
            .iter()
            .map(|arm| arm.pages_after_drain.saturating_sub(arm.ideal_pages))
            .max()
            .unwrap_or(0),
        sweeps_during_load: matching.iter().map(|arm| arm.sweeps_during_load).sum(),
    }
}

fn emit_summaries(arms: &[YieldArm], repetitions: usize) {
    for backend in ["wti", "arctic", "congee"] {
        let backend_arms: Vec<_> = arms.iter().filter(|arm| arm.backend == backend).collect();
        let summary = YieldSummary {
            schema_version: 1,
            suite: "vacuum-yield-summary",
            engine: "worktable",
            backend,
            repetitions,
            modes: [VacuumMode::Off, VacuumMode::Reactive, VacuumMode::Unpaced]
                .into_iter()
                .map(|mode| summarize_mode(&backend_arms, mode))
                .collect(),
            target_arch: std::env::consts::ARCH,
            target_os: std::env::consts::OS,
        };
        println!(
            "{}",
            serde_json::to_string(&summary).expect("summary serialises")
        );
    }
}

pub struct Config {
    pub seed_rows: u64,
    pub load: Duration,
    pub repetitions: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed_rows: 200_000,
            load: Duration::from_secs(2),
            repetitions: 6,
        }
    }
}

impl Config {
    pub fn from_args() -> Result<Self, String> {
        let mut config = Self::default();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--rows" => {
                    let v = args.get(i + 1).ok_or("--rows needs a value")?;
                    config.seed_rows = v
                        .parse()
                        .map_err(|_| format!("--rows: {v} is not a number"))?;
                    if config.seed_rows < 2 {
                        return Err("--rows must be at least 2".into());
                    }
                    i += 2;
                }
                "--load-secs" => {
                    let v = args.get(i + 1).ok_or("--load-secs needs a value")?;
                    let secs: u64 = v
                        .parse()
                        .map_err(|_| format!("--load-secs: {v} is not a number"))?;
                    if secs == 0 || secs > 30 {
                        return Err("--load-secs must be between 1 and 30".into());
                    }
                    config.load = Duration::from_secs(secs);
                    i += 2;
                }
                "--repetitions" => {
                    let v = args.get(i + 1).ok_or("--repetitions needs a value")?;
                    config.repetitions = v
                        .parse()
                        .map_err(|_| format!("--repetitions: {v} is not a number"))?;
                    if !(3..=12).contains(&config.repetitions) {
                        return Err("--repetitions must be between 3 and 12".into());
                    }
                    i += 2;
                }
                "--help" => {
                    return Err(
                        "usage: vacuum-yield-worktable [--rows N] [--load-secs S] [--repetitions N]"
                            .into(),
                    );
                }
                other => return Err(format!("unrecognised argument {other}")),
            }
        }
        Ok(config)
    }
}

fn stop_after(duration: Duration) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let flag = Arc::new(AtomicBool::new(false));
    let handle = {
        let flag = Arc::clone(&flag);
        std::thread::spawn(move || {
            std::thread::sleep(duration);
            flag.store(true, Ordering::Release);
        })
    };
    (flag, handle)
}

macro_rules! vacuum_yield_backend {
    ($module:ident, $backend:ident) => {
        pub mod $module {
            use super::*;
            use worktable::prelude::*;
            use worktable::vacuum::{EmptyDataVacuum, VacuumManager, VacuumManagerConfig, VacuumPacing};
            use worktable::worktable;

            worktable!(
                name: VacYield,
                persist: false,
                columns: {
                    id: u64 primary_key,
                    payload: u64,
                    bucket: u32,
                },
                indexes: {
                    bucket_idx: bucket,
                },
                queries: {
                    update: {
                        PayloadById(payload) by id,
                    }
                }
            );

            fn row(id: u64) -> VacYieldRow {
                VacYieldRow {
                    id,
                    payload: id.wrapping_mul(2_654_435_761),
                    bucket: (id % 64) as u32,
                }
            }

            pub async fn arm(
                seed_rows: u64,
                load: Duration,
                mode: VacuumMode,
                repetition: u32,
            ) -> YieldArm {
                let table = Arc::new(VacYieldWorkTable::default());
                for id in 0..seed_rows {
                    table.insert(row(id)).await.expect("seed");
                }
                // Exact control for the final row count. A ratio derived from
                // the full table can hide page-boundary effects; release
                // validation requires vacuum to land on the page count an
                // independently packed table actually uses.
                let packed_control = VacYieldWorkTable::default();
                for id in 0..seed_rows / 2 {
                    packed_control.insert(row(id)).await.expect("packed control");
                }
                let ideal_pages = packed_control.0.data.allocated_pages()
                    - packed_control.0.data.reusable_pages();
                drop(packed_control);

                // Start every measured arm at 50% fragmentation. Keeping the
                // deletes inside the timed phase let a small fixture consume
                // all of them early and spend most of the window doing only
                // in-place upserts. That workload could not distinguish an
                // unpaced sweep from vacuum-off, so it could not validate the
                // reactive result either.
                let mut missing = VecDeque::with_capacity(seed_rows as usize / 2);
                let mut live = VecDeque::with_capacity(seed_rows as usize / 2);
                for id in 0..seed_rows {
                    if id % 2 == 0 {
                        table.delete(id).await.expect("pre-fragment");
                        missing.push_back(id);
                    } else {
                        live.push_back(id);
                    }
                }

                let mut manager_handle = None;
                let vacuum_task = if mode.is_running() {
                    let manager = Arc::new(VacuumManager::with_config(VacuumManagerConfig::default()));
                    manager_handle = Some(Arc::clone(&manager));
                    if mode == VacuumMode::Unpaced {
                        let vacuum = EmptyDataVacuum::new(
                            table.name(),
                            Arc::clone(&table.0.data),
                            Arc::clone(&table.0.lock_manager),
                            Arc::clone(&table.0.primary_index),
                            Arc::clone(&table.0.indexes),
                        )
                        .with_pacing(VacuumPacing {
                            batch_pages: 0,
                            ..Default::default()
                        });
                        manager.register(Arc::new(vacuum));
                    } else {
                        manager.register(table.vacuum());
                    }
                    Some(manager.run_vacuum_task())
                } else {
                    None
                };

                let (stop, timer) = stop_after(load);
                let mut upsert_ns: Vec<u64> = Vec::new();
                let mut reinsert_ns: Vec<u64> = Vec::new();
                let mut delete_ns: Vec<u64> = Vec::new();
                let mut upserts = 0u64;
                let mut reinserts = 0u64;
                let mut deletes = 0u64;
                let load_started = Instant::now();

                while !stop.load(Ordering::Acquire) {
                    // One hit exercises the in-place upsert path that the old
                    // space-demand signal could not see.
                    let live_id = *live.front().expect("half the rows stay live");
                    let t = Instant::now();
                    table.upsert(row(live_id)).await.expect("upsert live row");
                    upsert_ns.push(t.elapsed().as_nanos() as u64);
                    upserts += 1;

                    // Reinsert one missing key, then delete one live key. The
                    // table remains half full and both allocation pressure and
                    // delete pressure continue for the entire window.
                    let missing_id = missing.pop_front().expect("half the rows stay missing");
                    let t = Instant::now();
                    table.upsert(row(missing_id)).await.expect("reinsert missing row");
                    reinsert_ns.push(t.elapsed().as_nanos() as u64);
                    reinserts += 1;
                    live.push_back(missing_id);

                    let delete_id = live.pop_front().expect("reinsert precedes delete");
                    let t = Instant::now();
                    table.delete(delete_id).await.expect("delete live row");
                    delete_ns.push(t.elapsed().as_nanos() as u64);
                    deletes += 1;
                    missing.push_back(delete_id);
                }
                let load_elapsed = load_started.elapsed();
                timer.join().expect("timer");

                let sweeps_during_load = manager_handle.as_ref().map(|m| m.stats.snapshot().0).unwrap_or(0);
                // Pages *in use*: a reclaimed page stays in the allocation vec
                // and moves to the reusable free list, so counting allocations
                // alone reports a sweep that worked perfectly as having freed
                // nothing. That mistake cost an hour.
                let pages_after_load = table.0.data.allocated_pages() - table.0.data.reusable_pages();

                // Quiet phase. Nothing else is running, so this is vacuum's
                // turn and the arm stays open until it stops reclaiming.
                let drain_started = Instant::now();
                let mut pages_after_drain = pages_after_load;
                let mut settled = Instant::now();
                let mut saw_sweep = sweeps_during_load != 0;
                while drain_started.elapsed() < DRAIN_LIMIT {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    saw_sweep |= manager_handle
                        .as_ref()
                        .is_some_and(|manager| manager.stats.snapshot().0 != 0);
                    let current = table.0.data.allocated_pages() - table.0.data.reusable_pages();
                    if current < pages_after_drain {
                        pages_after_drain = current;
                        settled = Instant::now();
                    } else if (!mode.is_running() || saw_sweep) && settled.elapsed() >= DRAIN_SETTLED {
                        break;
                    }
                }
                let drain_ns = drain_started.elapsed().as_nanos();
                let sweeps_after_load =
                    manager_handle.as_ref().map(|m| m.stats.snapshot().0).unwrap_or(0) - sweeps_during_load;

                let rows_left = table.count() as u64;

                if let Some(task) = vacuum_task {
                    task.abort();
                }

                YieldArm {
                    schema_version: 3,
                    suite: "vacuum-yield",
                    engine: "worktable",
                    backend: stringify!($module),
                    vacuum_mode: mode.name(),
                    vacuum_running: mode.is_running(),
                    repetition,
                    upserts,
                    reinserts,
                    deletes,
                    upsert_latency: LatencySummary::from_samples(upsert_ns),
                    reinsert_latency: LatencySummary::from_samples(reinsert_ns),
                    delete_latency: LatencySummary::from_samples(delete_ns),
                    load_elapsed_ns: load_elapsed.as_nanos(),
                    sweeps_during_load,
                    sweeps_after_load,
                    pages_after_load,
                    pages_after_drain,
                    ideal_pages,
                    rows_left,
                    drain_ns,
                    target_arch: std::env::consts::ARCH,
                    target_os: std::env::consts::OS,
                }
            }
        }
    };
}

vacuum_yield_backend!(wti, worktables_index);
vacuum_yield_backend!(arctic, arctic);
vacuum_yield_backend!(congee, congee);

pub async fn run_all(config: &Config) {
    // Rotate all six permutations. Over six repetitions each mode owns every
    // ordinal position twice, so host drift cannot consistently favour one.
    const ORDERS: [[VacuumMode; 3]; 6] = [
        [VacuumMode::Off, VacuumMode::Reactive, VacuumMode::Unpaced],
        [VacuumMode::Reactive, VacuumMode::Unpaced, VacuumMode::Off],
        [VacuumMode::Unpaced, VacuumMode::Off, VacuumMode::Reactive],
        [VacuumMode::Off, VacuumMode::Unpaced, VacuumMode::Reactive],
        [VacuumMode::Unpaced, VacuumMode::Reactive, VacuumMode::Off],
        [VacuumMode::Reactive, VacuumMode::Off, VacuumMode::Unpaced],
    ];
    let mut arms = Vec::with_capacity(config.repetitions * 9);
    for repetition in 0..config.repetitions {
        let modes = ORDERS[repetition % ORDERS.len()];
        for mode in modes {
            for arm in [
                wti::arm(config.seed_rows, config.load, mode, repetition as u32).await,
                arctic::arm(config.seed_rows, config.load, mode, repetition as u32).await,
                congee::arm(config.seed_rows, config.load, mode, repetition as u32).await,
            ] {
                emit(&arm);
                arms.push(arm);
            }
        }
    }
    emit_summaries(&arms, config.repetitions);
}
