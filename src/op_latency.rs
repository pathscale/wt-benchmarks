//! Single-operation latency and insert scaling, as a suite.
//!
//! Two questions the throughput suites cannot answer.
//!
//! **What does one call cost.** YCSB and the KV suite report aggregate rates
//! over batches, which is the right shape for throughput and the wrong one for
//! latency: the batching is what a latency question is trying to see through.
//! Every operation here is timed individually and summarised with
//! [`LatencySummary`], so the p99 is a real p99 over calls rather than over
//! batches of calls.
//!
//! **Where insert stops scaling.** Aggregate throughput against writer count.
//! The finding is the shape: flat to four writers, then a cliff, because
//! `DataPages::insert` takes an exclusive write lock on the one page named by
//! `current_page_id` and appends therefore serialise. Before
//! `EmptyLinkRegistry::pop_max` gained a fast path this was 0.25x of
//! single-writer throughput at eight writers; after it, about 0.6x.
//!
//! Both modes are measured because they are different engines. An in-memory
//! write stops at the row and its indexes; a persisted write also queues an
//! operation, and these figures are caller-visible rather than to-durability.
//! `insert_many_bench` in the WorkTable repository is where those two are
//! compared.
//!
//! Feature-gated on `worktable-adapter`.

use std::env;
use std::fmt;

use serde::Serialize;

use crate::result::LatencySummary;

/// Which storage engine an arm ran against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Memory,
    Disk,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Memory => "memory",
            Mode::Disk => "disk",
        })
    }
}

#[derive(Clone, Debug)]
pub struct LatencyConfig {
    /// Operations timed per arm.
    pub operations: u64,
    /// Writer counts the scaling sweep visits.
    pub sweep: Vec<u64>,
    /// Rows inserted per scaling arm, split across its writers.
    pub scaling_rows: u64,
    /// Repetitions of the scaling sweep; the best is reported.
    pub repetitions: usize,
    /// Where the persisted arm keeps its files.
    pub disk_dir: String,
    /// Skip the persisted arms.
    pub memory_only: bool,
}

impl Default for LatencyConfig {
    fn default() -> Self {
        Self {
            operations: 50_000,
            sweep: vec![1, 2, 4, 8, 16],
            scaling_rows: 200_000,
            repetitions: 3,
            disk_dir: "/tmp/wt-bench-op-latency".to_owned(),
            memory_only: false,
        }
    }
}

impl LatencyConfig {
    pub fn from_args() -> Result<Self, String> {
        let mut config = Self::default();
        let mut args = env::args().skip(1);
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                "--operations" => config.operations = parse(&mut args, "--operations")?,
                "--scaling-rows" => config.scaling_rows = parse(&mut args, "--scaling-rows")?,
                "--repetitions" => {
                    config.repetitions = parse::<u64>(&mut args, "--repetitions")? as usize
                }
                "--disk-dir" => {
                    config.disk_dir = args.next().ok_or("--disk-dir needs a value")?;
                }
                "--memory-only" => config.memory_only = true,
                "--sweep" => {
                    let raw = args.next().ok_or("--sweep needs a value")?;
                    config.sweep = raw
                        .split(',')
                        .map(|part| {
                            part.trim()
                                .parse::<u64>()
                                .map_err(|_| format!("bad --sweep value {raw:?}"))
                        })
                        .collect::<Result<_, _>>()?;
                }
                other => return Err(format!("unknown flag {other}")),
            }
        }
        if config.sweep.is_empty() {
            return Err("--sweep needs at least one writer count".to_owned());
        }
        Ok(config)
    }
}

fn parse<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<T, String> {
    args.next()
        .ok_or_else(|| format!("{flag} needs a value"))?
        .parse()
        .map_err(|_| format!("{flag} needs a number"))
}

fn print_help() {
    eprintln!(
        "single-operation latency and insert scaling\n\n\
         --operations N     operations timed per latency arm (default 50000)\n\
         --sweep A,B,C      writer counts for the scaling sweep (default 1,2,4,8,16)\n\
         --scaling-rows N   rows per scaling arm (default 200000)\n\
         --repetitions N    scaling repetitions, best reported (default 3)\n\
         --disk-dir PATH    where the persisted arm writes (default /tmp/wt-bench-op-latency)\n\
         --memory-only      skip the persisted arms\n"
    );
}

/// One timed operation arm.
#[derive(Debug, Serialize)]
pub struct LatencyResult<'a> {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'static str,
    pub mode: String,
    pub operation: &'a str,
    pub operations: u64,
    pub elapsed_ns: u128,
    pub ops_per_second: f64,
    pub latency: LatencySummary,
    pub target_arch: &'static str,
    pub target_os: &'static str,
}

/// One arm of the scaling sweep.
#[derive(Debug, Serialize)]
pub struct ScalingResult {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'static str,
    pub mode: String,
    pub writers: u64,
    pub rows: u64,
    pub repetitions: usize,
    pub elapsed_ns: u128,
    pub ops_per_second: f64,
    pub ns_per_insert: f64,
    /// Throughput as a share of the single-writer arm. Below 1.0 means adding
    /// writers made the table slower in aggregate than one thread.
    pub relative_to_single_writer: f64,
    pub target_arch: &'static str,
    pub target_os: &'static str,
}

pub fn emit_latency(mode: Mode, operation: &str, samples: Vec<u64>, elapsed_ns: u128) {
    let operations = samples.len() as u64;
    let result = LatencyResult {
        schema_version: 1,
        suite: "op-latency",
        engine: "worktable",
        mode: mode.to_string(),
        operation,
        operations,
        elapsed_ns,
        ops_per_second: operations as f64 / (elapsed_ns as f64 / 1_000_000_000.0),
        latency: LatencySummary::from_samples(samples),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    };
    println!(
        "{}",
        serde_json::to_string(&result).expect("result must serialize")
    );
}

pub fn emit_scaling(
    writers: u64,
    rows: u64,
    repetitions: usize,
    elapsed_ns: u128,
    single_writer_rate: f64,
) {
    let rate = rows as f64 / (elapsed_ns as f64 / 1_000_000_000.0);
    let result = ScalingResult {
        schema_version: 1,
        suite: "insert-scaling",
        engine: "worktable",
        mode: Mode::Memory.to_string(),
        writers,
        rows,
        repetitions,
        elapsed_ns,
        ops_per_second: rate,
        ns_per_insert: elapsed_ns as f64 / rows as f64,
        relative_to_single_writer: if single_writer_rate > 0.0 {
            rate / single_writer_rate
        } else {
            1.0
        },
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    };
    println!(
        "{}",
        serde_json::to_string(&result).expect("result must serialize")
    );
}

pub mod memory {
    use worktable::prelude::*;
    use worktable::worktable;

    worktable!(
        name: Lat,
        persist: false,
        columns: { id: u64 primary_key, payload: u64, bucket: u32 },
        indexes: { payload_idx: payload unique, bucket_idx: bucket },
    );

    pub fn table() -> LatWorkTable {
        LatWorkTable::default()
    }

    pub fn row(id: u64, payload_base: u64) -> LatRow {
        LatRow {
            id,
            payload: payload_base + id,
            bucket: (id % 16) as u32,
        }
    }
}

pub mod disk {
    use worktable::prelude::*;
    use worktable::worktable;

    worktable!(
        name: OnDisk,
        persist: true,
        columns: { id: u64 primary_key, payload: u64, bucket: u32 },
        indexes: { payload_idx: payload unique, bucket_idx: bucket },
    );

    pub async fn table(dir: &str) -> OnDiskWorkTable {
        let _ = std::fs::remove_dir_all(dir);
        let config = DiskConfig::new_with_table_name(
            dir,
            OnDiskWorkTable::name_snake_case(),
            OnDiskWorkTable::version(),
        );
        let engine = OnDiskPersistenceEngine::new(config).await.expect("engine");
        OnDiskWorkTable::load(engine).await.expect("load")
    }

    pub fn row(id: u64, payload_base: u64) -> OnDiskRow {
        OnDiskRow {
            id,
            payload: payload_base + id,
            bucket: (id % 16) as u32,
        }
    }
}
