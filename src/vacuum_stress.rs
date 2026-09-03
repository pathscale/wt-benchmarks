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
            async fn fragmented(seed_rows: u64, fragmentation_pct: u64) -> (Arc<VacStressWorkTable>, u64) {
                let table = Arc::new(VacStressWorkTable::default());
                for id in 0..seed_rows {
                    table.insert(row(id)).await.expect("seed");
                }
                if fragmentation_pct > 0 {
                    let step = (100 / fragmentation_pct).max(1);
                    let mut id = 0;
                    while id < seed_rows {
                        let _ = table.delete(id).await;
                        id += step;
                    }
                }
                (table, seed_rows)
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
                let (table, mut next_id) = fragmented(seed_rows, fragmentation_pct).await;

                let vacuum_task = if vacuum_running {
                    // Shipping defaults, on purpose: woken by freed space,
                    // with the delete burst allowed to settle first.
                    let manager = Arc::new(VacuumManager::with_config(VacuumManagerConfig::default()));
                    manager.register(table.vacuum());
                    Some(manager.run_vacuum_task())
                } else {
                    None
                };

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

                    if churning && insert_ns.len() % 500 == 0 {
                        for _ in 0..200 {
                            delete_cursor += 1;
                            if table.delete(delete_cursor).await.is_ok() {
                                deletes += 1;
                            }
                        }
                    }
                }
                let elapsed = started.elapsed();

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
