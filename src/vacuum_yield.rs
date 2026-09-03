//! Does vacuum stay out of the way, and does it still return everything?
//!
//! The other suite asks what a sweep costs while it runs. This one asks the
//! question that decides whether the pacing design works at all: under
//! sustained upsert and delete pressure, does vacuum quietly wait its turn?
//!
//! A sweep that runs anyway and takes its 30% is failing, however much memory
//! it hands back. So is one that waits politely and then never returns the
//! memory. Both halves are measured here, in two phases:
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
//! The load window is deliberately short. A couple of seconds should not come
//! near the bound on consecutive stand-downs, so a sweep that fires during it
//! is not being forced through by the cap: it simply is not deferring.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::result::LatencySummary;

/// How long the sweep is given to finish reclaiming once the load stops.
const DRAIN_LIMIT: Duration = Duration::from_secs(20);

/// No further reclamation for this long counts as finished.
const DRAIN_SETTLED: Duration = Duration::from_millis(500);

#[derive(Serialize)]
pub struct YieldArm {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'static str,
    pub backend: &'static str,
    /// Whether vacuum was running during the load phase.
    pub vacuum_running: bool,
    pub upserts: u64,
    pub deletes: u64,
    /// Foreground latency during the load. The comparison against the
    /// vacuum-off arm is the whole point: equal means vacuum stayed out of the
    /// way, worse means it did not.
    pub upsert_latency: LatencySummary,
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

pub fn emit(arm: YieldArm) {
    println!("{}", serde_json::to_string(&arm).expect("arm serialises"));
}

pub struct Config {
    pub seed_rows: u64,
    pub load: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed_rows: 200_000,
            load: Duration::from_secs(2),
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
                    config.seed_rows = v.parse().map_err(|_| format!("--rows: {v} is not a number"))?;
                    i += 2;
                }
                "--load-secs" => {
                    let v = args.get(i + 1).ok_or("--load-secs needs a value")?;
                    let secs: u64 = v.parse().map_err(|_| format!("--load-secs: {v} is not a number"))?;
                    if secs == 0 || secs > 30 {
                        return Err("--load-secs must be between 1 and 30".into());
                    }
                    config.load = Duration::from_secs(secs);
                    i += 2;
                }
                "--help" => return Err("usage: vacuum-yield-worktable [--rows N] [--load-secs S]".into()),
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
            use worktable::vacuum::{VacuumManager, VacuumManagerConfig};
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

            pub async fn arm(seed_rows: u64, load: Duration, vacuum_running: bool) {
                let table = Arc::new(VacYieldWorkTable::default());
                for id in 0..seed_rows {
                    table.insert(row(id)).await.expect("seed");
                }
                let packed_pages = table.0.data.allocated_pages() - table.0.data.reusable_pages();

                let mut manager_handle = None;
                let vacuum_task = if vacuum_running {
                    let manager = Arc::new(VacuumManager::with_config(VacuumManagerConfig::default()));
                    manager_handle = Some(Arc::clone(&manager));
                    manager.register(table.vacuum());
                    Some(manager.run_vacuum_task())
                } else {
                    None
                };

                let (stop, timer) = stop_after(load);
                let mut upsert_ns: Vec<u64> = Vec::new();
                let mut delete_ns: Vec<u64> = Vec::new();
                let mut upserts = 0u64;
                let mut deletes = 0u64;
                // Deletes walk forward from the bottom; upserts churn what is
                // left above them. So the table shrinks the whole time and the
                // freed pages are real, which is what gives vacuum a reason to
                // want to run during the load.
                let mut delete_cursor = 0u64;
                let mut upsert_cursor = seed_rows / 2;
                let load_started = Instant::now();

                while !stop.load(Ordering::Acquire) {
                    let t = Instant::now();
                    let _ = table.upsert(row(upsert_cursor)).await;
                    upsert_ns.push(t.elapsed().as_nanos() as u64);
                    upserts += 1;
                    upsert_cursor += 1;
                    if upsert_cursor >= seed_rows {
                        upsert_cursor = seed_rows / 2;
                    }

                    if delete_cursor < seed_rows / 2 {
                        let t = Instant::now();
                        let _ = table.delete(delete_cursor).await;
                        delete_ns.push(t.elapsed().as_nanos() as u64);
                        deletes += 1;
                        delete_cursor += 1;
                    }
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
                while drain_started.elapsed() < DRAIN_LIMIT {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    let current = table.0.data.allocated_pages() - table.0.data.reusable_pages();
                    if current < pages_after_drain {
                        pages_after_drain = current;
                        settled = Instant::now();
                    } else if settled.elapsed() >= DRAIN_SETTLED {
                        break;
                    }
                }
                let drain_ns = drain_started.elapsed().as_nanos();
                let sweeps_after_load =
                    manager_handle.as_ref().map(|m| m.stats.snapshot().0).unwrap_or(0) - sweeps_during_load;

                let rows_left = table.count() as u64;
                let ideal_pages = if seed_rows == 0 {
                    0
                } else {
                    ((rows_left as f64 / seed_rows as f64) * packed_pages as f64).ceil() as usize
                };

                if let Some(task) = vacuum_task {
                    task.abort();
                }

                emit(YieldArm {
                    schema_version: 1,
                    suite: "vacuum-yield",
                    engine: "worktable",
                    backend: stringify!($module),
                    vacuum_running,
                    upserts,
                    deletes,
                    upsert_latency: LatencySummary::from_samples(upsert_ns),
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
                });
            }
        }
    };
}

vacuum_yield_backend!(wti, worktables_index);
vacuum_yield_backend!(arctic, arctic);
vacuum_yield_backend!(congee, congee);

pub async fn run_all(config: &Config) {
    for running in [false, true] {
        wti::arm(config.seed_rows, config.load, running).await;
        arctic::arm(config.seed_rows, config.load, running).await;
        congee::arm(config.seed_rows, config.load, running).await;
    }
}
