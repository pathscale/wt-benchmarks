//! What vacuum costs the writes and reads happening while it runs.
//!
//! **Consumer profile: correctness and tail latency.** See
//! `docs/BENCHMARK_CATALOG.md`.
//!
//! Vacuum is the one background process in this engine that moves rows and
//! frees pages under live traffic, and nothing measured it. Every statement
//! about its cost, including "`defragment` holds the registry lock for the
//! whole pass, so no insert can reuse a freed link while it runs", was read
//! out of the source rather than observed. That is the gap this closes.
//!
//! The number that matters is not how fast a pass completes. It is what the
//! foreground pays while one is happening, at the tail: a pass that halves
//! fragmentation and doubles p99.9 insert latency is not a good trade for an
//! engine whose callers care about the tail.
//!
//! So each arm runs the same foreground workload twice, once with vacuum
//! stopped and once with it running hard, and reports both. The delta between
//! them is the measurement; the absolute numbers are context.
//!
//! Run across all three primary backends, because they do not have the same
//! shape: arctic boxes every value, congee holds no non-unique index, and the
//! general-purpose one is the default and the slowest on every axis measured
//! so far. A vacuum cost that only appears on one of them is the kind that
//! ships.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::result::LatencySummary;

/// Rows the table starts from.
pub const SEED_ROWS: u64 = 20_000;
/// How long each foreground arm runs. Two arms per cell, so a cell is twice
/// this plus setup.
pub const ARM: Duration = Duration::from_secs(2);
/// Fraction of rows deleted before measuring, as a percentage. Fragmentation
/// is what gives vacuum something to do, and how much of it there is changes
/// how long a pass takes.
pub const FRAGMENTATION: [u64; 2] = [25, 60];

/// How long an arm waits for the sweep to finish reclaiming after the workload
/// stops. Generous on purpose: the question is whether the memory comes back
/// at all and how long it takes, so cutting this short would answer neither.
const DRAIN_LIMIT: Duration = Duration::from_secs(20);

#[derive(Serialize)]
pub struct VacuumArm {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'static str,
    pub backend: &'static str,
    /// Percentage of seeded rows deleted before the arm ran.
    pub fragmentation_pct: u64,
    /// Whether vacuum was running during the arm.
    pub vacuum_running: bool,
    /// How the sweep was triggered: "off" or "reactive".
    ///
    /// There is no polling mode to compare against any more. The interval was
    /// removed from the manager's config, because exposing it invited exactly
    /// what this benchmark used to do: turn it down to 5ms, at which point the
    /// timer wins every wake and neither the fragmentation threshold nor the
    /// settle does anything. The arm now measures what ships.
    pub trigger: &'static str,
    /// Whether the arm deleted while it measured.
    ///
    /// Without this the arm fragments up front and then only inserts, so the
    /// table is never *becoming* fragmented while vacuum decides what to do.
    /// That leaves the reactive path untested: the wake fires once at the
    /// start and the settle, which exists to keep a sweep out of a live delete
    /// burst, is never exercised at all.
    pub churning: bool,
    pub deletes: u64,
    /// Pages the sweep actually reclaimed, measured at the end of the arm.
    ///
    /// Without this the cost columns are unreadable: a sweep that stands down
    /// so hard it never runs reports a penalty of zero, which looks like a win
    /// and is a regression in the thing vacuum exists to do. Cost is only
    /// meaningful next to work done.
    pub empty_pages: usize,
    /// Reclaimable bytes still registered when the arm ended. The lower this
    /// is against the vacuum-off arm, the more the sweep kept up.
    pub reclaimable_bytes_left: u64,
    /// Sweeps the manager actually ran during the arm.
    ///
    /// The question a cost column cannot answer: does a reactive sweep keep
    /// firing, or does it fire once and never again? A penalty of zero means
    /// nothing until you know which.
    pub sweeps: u64,
    pub sweep_pages_freed: u64,
    /// Bytes the table has allocated, at the start of the arm and at its end.
    ///
    /// This is the half that verifies the other half. Vacuum exists to give
    /// memory back, so a cost number without it cannot be checked: a sweep
    /// that never runs is indistinguishable from a sweep that is free. The
    /// comparison that matters is `heap_end` against the same arm with vacuum
    /// off, not against `heap_start`, because both arms grow as they insert.
    pub pages_start: usize,
    pub pages_end: usize,
    /// Pages allocated but on the empty list at the end: reclaimed by a sweep
    /// and reusable without going back to the allocator.
    pub pages_reusable_end: usize,
    /// Pages still held after the workload stopped and the sweep was given as
    /// long as it wanted. This is the number that says whether vacuum returns
    /// everything or only some of it.
    pub pages_after_drain: usize,
    /// Pages a packed table holding `rows_left` would need. `pages_after_drain`
    /// equal to this is a full return; above it is memory never given back.
    pub ideal_pages: usize,
    pub rows_left: u64,
    /// How long the sweep took to stop reclaiming, once nothing else was
    /// running. The answer to "how long until the memory comes back".
    pub drain_ns: u128,
    pub inserts: u64,
    pub selects: u64,
    pub insert_latency: LatencySummary,
    pub select_latency: LatencySummary,
    pub elapsed_ns: u128,
    pub target_arch: &'static str,
    pub target_os: &'static str,
}

pub fn emit(arm: VacuumArm) {
    println!("{}", serde_json::to_string(&arm).expect("arm serialises"));
}

pub struct Config {
    pub seed_rows: u64,
    pub arm: Duration,
}

impl Config {
    pub fn from_args() -> Result<Self, String> {
        let mut seed_rows = SEED_ROWS;
        let mut arm = ARM;
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--rows" => {
                    let v = args.get(i + 1).ok_or("--rows needs a value")?;
                    seed_rows = v.parse().map_err(|_| format!("--rows: {v} is not a number"))?;
                    i += 2;
                }
                "--arm-secs" => {
                    let v = args.get(i + 1).ok_or("--arm-secs needs a value")?;
                    let secs: u64 = v.parse().map_err(|_| format!("--arm-secs: {v} is not a number"))?;
                    if secs == 0 || secs > 30 {
                        return Err("--arm-secs must be between 1 and 30: a benchmark nobody will wait for is one nobody runs".into());
                    }
                    arm = Duration::from_secs(secs);
                    i += 2;
                }
                "--help" => return Err("usage: vacuum-stress-worktable [--rows N] [--arm-secs S]".into()),
                other => return Err(format!("unrecognised argument {other}")),
            }
        }
        Ok(Self { seed_rows, arm })
    }
}

/// Shared stop flag, so both arms end on a wall clock rather than a count.
/// A count would let the slower arm do less work and then be compared as
/// though it had done the same.
pub struct Stop(pub Arc<AtomicBool>);

impl Stop {
    pub fn armed_for(duration: Duration) -> (Self, std::thread::JoinHandle<()>) {
        let flag = Arc::new(AtomicBool::new(false));
        let handle = {
            let flag = Arc::clone(&flag);
            std::thread::spawn(move || {
                std::thread::sleep(duration);
                flag.store(true, Ordering::Release);
            })
        };
        (Self(flag), handle)
    }

    pub fn hit(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Counts the vacuum passes that actually ran, so an arm that reports no cost
/// can be distinguished from one where vacuum never got going.
#[derive(Default)]
pub struct PassCounter(pub AtomicU64);

pub fn now() -> Instant {
    Instant::now()
}

macro_rules! vacuum_stress_backend {
    ($module:ident, $backend:ident) => {
        pub mod $module {
            use super::*;
            use worktable::prelude::*;
            use worktable::vacuum::{VacuumManager, VacuumManagerConfig};
            use worktable::worktable;

            worktable!(
                name: VacStress,
                persist: false,
                columns: {
                    id: u64 primary_key,
                    uniq: u64,
                    tag: u32,
                    payload: String
                },
                indexes: {
                    uniq_idx: uniq unique using $backend,
                    tag_idx: tag,
                }
            );

            fn row(id: u64) -> VacStressRow {
                VacStressRow {
                    id,
                    uniq: 1_000_000 + id,
                    tag: (id % 8) as u32,
                    payload: format!("payload-{id}"),
                }
            }

            /// Seed, then delete `fragmentation_pct` of the rows so vacuum has
            /// work. Returns the table and the next free id.
            /// Returns the table, the next free id, and the pages a *packed*
            /// table of `seed_rows` occupies. That last number is the yardstick:
            /// a fully reclaimed table should need the same pages per row, so
            /// anything above it is memory the sweep has not given back yet.
            async fn fragmented(seed_rows: u64, fragmentation_pct: u64) -> (Arc<VacStressWorkTable>, u64, usize) {
                let table = Arc::new(VacStressWorkTable::default());
                for id in 0..seed_rows {
                    table.insert(row(id)).await.expect("seed");
                }
                let packed_pages = table.0.data.allocated_pages();
                if fragmentation_pct > 0 {
                    let step = (100 / fragmentation_pct).max(1);
                    let mut id = 0;
                    while id < seed_rows {
                        let _ = table.delete(id).await;
                        id += step;
                    }
                }
                (table, seed_rows, packed_pages)
            }

            /// One arm: run inserts and selects against the table for the
            /// duration, timing each call, optionally with vacuum running.
            pub async fn arm(
                seed_rows: u64,
                fragmentation_pct: u64,
                duration: Duration,
                trigger: &'static str,
                churning: bool,
            ) {
                let vacuum_running = trigger != "off";
                let (table, mut next_id, packed_pages) = fragmented(seed_rows, fragmentation_pct).await;

                let mut manager_handle = None;
                let vacuum_task = if vacuum_running {
                    // Shipping defaults, on purpose: woken by freed space,
                    // with the delete burst allowed to settle first.
                    let manager = Arc::new(VacuumManager::with_config(VacuumManagerConfig::default()));
                    manager_handle = Some(Arc::clone(&manager));
                    manager.register(table.vacuum());
                    Some(manager.run_vacuum_task())
                } else {
                    None
                };

                let pages_start = table.0.data.allocated_pages();
                let (stop, timer) = Stop::armed_for(duration);
                let mut insert_ns: Vec<u64> = Vec::new();
                let mut select_ns: Vec<u64> = Vec::new();
                let mut deletes = 0u64;
                // Deleted in runs rather than one per turn: a ranged delete is
                // what produces garbage in bursts, and bursts are what the
                // settle is for. One-at-a-time would be a steady trickle and
                // would not exercise it either.
                let mut delete_cursor = 0u64;
                let started = now();

                while !stop.hit() {
                    // A write and a read per turn, so one arm cannot be
                    // dominated by whichever is cheaper on that backend.
                    let t = now();
                    let _ = table.insert(row(next_id)).await;
                    insert_ns.push(t.elapsed().as_nanos() as u64);
                    next_id += 1;

                    let probe = next_id % seed_rows.max(1);
                    let t = now();
                    std::hint::black_box(table.select(probe));
                    select_ns.push(t.elapsed().as_nanos() as u64);

                    // Delete more than the turn inserted, so fragmentation is
                    // *sustained* rather than consumed. The previous ratio
                    // deleted 200 per 500 inserts, which the inserts recycled
                    // immediately: the registry ended every arm at roughly zero
                    // reclaimable bytes and the sweep correctly never fired,
                    // which made every cost number here a measurement of
                    // nothing.
                    if churning && insert_ns.len() % 100 == 0 {
                        for _ in 0..300 {
                            delete_cursor += 1;
                            if table.delete(delete_cursor).await.is_ok() {
                                deletes += 1;
                            }
                        }
                    }
                }
                let elapsed = started.elapsed();

                let pages_end = table.0.data.allocated_pages();
                let pages_reusable_end = table.0.data.reusable_pages();
                let (sweeps, sweep_pages_freed, _) = manager_handle
                    .as_ref()
                    .map(|m| m.stats.snapshot())
                    .unwrap_or((0, 0, 0));
                let empty_pages = table.0.data.get_empty_pages().len();
                let reclaimable_bytes_left = table.0.data.empty_links_registry().reclaimable_bytes();

                // The workload has stopped. Hold the arm open and let the
                // sweep finish, because a partial return is not a result: a
                // vacuum that hands back 37% and stops has not done its job,
                // and closing here would record that as success. Poll until
                // the page count stops falling, then report where it landed
                // and how long it took.
                let drain_started = now();
                let mut pages_after_drain = table.0.data.allocated_pages();
                let mut stable_for = 0u32;
                while drain_started.elapsed() < DRAIN_LIMIT {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let current = table.0.data.allocated_pages();
                    if current < pages_after_drain {
                        pages_after_drain = current;
                        stable_for = 0;
                    } else {
                        stable_for += 1;
                        // Half a second with no further reclamation is as
                        // finished as it is going to get.
                        if stable_for >= 10 {
                            break;
                        }
                    }
                }
                let drain_ns = drain_started.elapsed().as_nanos();
                let rows_left = table.count() as u64;
                // What a packed table holding this many rows would need.
                let ideal_pages = if seed_rows == 0 {
                    0
                } else {
                    ((rows_left as f64 / seed_rows as f64) * packed_pages as f64).ceil() as usize
                };

                if let Some(task) = vacuum_task {
                    task.abort();
                }
                timer.join().expect("timer");

                emit(VacuumArm {
                    schema_version: 1,
                    suite: "vacuum-stress",
                    engine: "worktable",
                    backend: stringify!($module),
                    fragmentation_pct,
                    vacuum_running,
                    trigger,
                    churning,
                    deletes,
                    empty_pages,
                    reclaimable_bytes_left,
                    sweeps,
                    sweep_pages_freed,
                    pages_start,
                    pages_end,
                    pages_reusable_end,
                    pages_after_drain,
                    ideal_pages,
                    rows_left,
                    drain_ns,
                    inserts: insert_ns.len() as u64,
                    selects: select_ns.len() as u64,
                    insert_latency: LatencySummary::from_samples(insert_ns),
                    select_latency: LatencySummary::from_samples(select_ns),
                    elapsed_ns: elapsed.as_nanos(),
                    target_arch: std::env::consts::ARCH,
                    target_os: std::env::consts::OS,
                });

                drop(table);
            }
        }
    };
}

vacuum_stress_backend!(wti, worktables_index);
vacuum_stress_backend!(arctic, arctic);
vacuum_stress_backend!(congee, congee);

/// Every cell: three backends, two fragmentation levels, vacuum off then on.
pub async fn run_all(config: &Config) {
    for pct in FRAGMENTATION {
        for churning in [false, true] {
            for trigger in ["off", "reactive"] {
                wti::arm(config.seed_rows, pct, config.arm, trigger, churning).await;
                arctic::arm(config.seed_rows, pct, config.arm, trigger, churning).await;
                congee::arm(config.seed_rows, pct, config.arm, trigger, churning).await;
            }
        }
    }
}
