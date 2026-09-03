//! The AgentCode write profile.
//!
//! **Consumer profile: AgentCode.** See `docs/BENCHMARK_CATALOG.md`.
//!
//! Every other suite here measures point operations. AgentCode does not do
//! point operations: it indexes a repository into immutable generations, and
//! one generation of its fact-dense fixture is 800 files, 14,400 symbols and
//! 14,400 dependency edges, all rewritten together. Its recorded phase profile
//! puts **74% of an incremental update in three bulk write phases**, and 147 ms
//! of `put_symbols`'s 156 ms is marginal per-row cost.
//!
//! So the numbers that decide whether WorkTable is getting better for AgentCode
//! are not YCSB's. They are: what does a persisted row cost in bulk, how much
//! of that does `insert_many` remove, and how much does persistence add over
//! the same work in memory. That last ratio was measured at **7.9x** on
//! beta.12, like for like, and is the figure to design against.
//!
//! Shapes taken from `agentcode-worktable-asks.md`, including the row: the
//! dedup lookup before each insert is in the real `put_symbols` loop and is
//! part of the cost, so it is here too.
//!
//! Feature-gated on `worktable-adapter`.

use std::time::Instant;

use serde::Serialize;
use worktable::prelude::*;
use worktable::worktable;

/// Symbols in one generation of the fact-dense fixture. Their number, not a
/// round one: a benchmark that does not write a generation is not measuring
/// the thing that costs 74% of an update.
pub const SYMBOLS_PER_GENERATION: u64 = 14_400;

// `SymbolPostingRow`, as AgentCode declares it. Written out twice rather than
// generated from a `macro_rules!`: substituted metavariables reach a proc macro
// wrapped in invisible groups, and `worktable!` rejects them as not an
// identifier. Two copies that differ in one word are clearer than that fight.

pub mod memory {
    use super::*;

    worktable!(
        name: MemPosting,
        persist: false,
        columns: {
            id: u64 primary_key,
            posting_hash: u64,
            snapshot_id: String,
            normalized_name: String,
            records_blob: String,
        },
        indexes: {
            posting_hash_idx: posting_hash unique,
        },
    );

    pub fn row(i: u64, snapshot: u64) -> MemPostingRow {
        MemPostingRow {
            id: i,
            posting_hash: snapshot * 1_000_000 + i,
            snapshot_id: format!("snapshot-{snapshot}"),
            normalized_name: format!("crate::module::symbol_name_{i}"),
            records_blob: format!("blob-{:016x}", i.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        }
    }
}

pub mod disk {
    use super::*;

    worktable!(
        name: DiskPosting,
        persist: true,
        columns: {
            id: u64 primary_key,
            posting_hash: u64,
            snapshot_id: String,
            normalized_name: String,
            records_blob: String,
        },
        indexes: {
            posting_hash_idx: posting_hash unique,
        },
    );

    pub async fn table(dir: &str) -> DiskPostingWorkTable {
        let _ = std::fs::remove_dir_all(dir);
        let config = DiskConfig::new_with_table_name(
            dir,
            DiskPostingWorkTable::name_snake_case(),
            DiskPostingWorkTable::version(),
        );
        let engine = DiskPostingPersistenceEngine::new(config)
            .await
            .expect("engine");
        DiskPostingWorkTable::load(engine).await.expect("load")
    }

    pub fn row(i: u64, snapshot: u64) -> DiskPostingRow {
        DiskPostingRow {
            id: i,
            posting_hash: snapshot * 1_000_000 + i,
            snapshot_id: format!("snapshot-{snapshot}"),
            normalized_name: format!("crate::module::symbol_name_{i}"),
            records_blob: format!("blob-{:016x}", i.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        }
    }
}

#[derive(Serialize)]
pub struct PhaseResult {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'static str,
    /// "memory" or "disk". The ratio between them is the headline.
    pub mode: &'static str,
    /// The AgentCode phase this stands for.
    pub phase: &'static str,
    pub rows: u64,
    pub elapsed_ns: u128,
    pub nanos_per_row: f64,
    pub target_arch: &'static str,
    pub target_os: &'static str,
}

pub fn emit(mode: &'static str, phase: &'static str, rows: u64, elapsed_ns: u128) {
    let result = PhaseResult {
        schema_version: 1,
        suite: "agentcode",
        engine: "worktable",
        mode,
        phase,
        rows,
        elapsed_ns,
        nanos_per_row: elapsed_ns as f64 / rows as f64,
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    };
    println!("{}", serde_json::to_string(&result).expect("serialize"));
}

pub struct Config {
    pub rows: u64,
    pub dir: String,
}

impl Config {
    pub fn from_args() -> Result<Self, String> {
        let mut rows = SYMBOLS_PER_GENERATION;
        let mut dir = std::env::temp_dir()
            .join("wt-bench-agentcode")
            .to_string_lossy()
            .into_owned();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--rows" => {
                    let v = args.get(i + 1).ok_or("--rows needs a value")?;
                    rows = v
                        .parse()
                        .map_err(|_| format!("--rows: {v} is not a number"))?;
                    if rows == 0 {
                        return Err("--rows must be at least 1".into());
                    }
                    i += 2;
                }
                "--dir" => {
                    dir = args.get(i + 1).ok_or("--dir needs a value")?.clone();
                    i += 2;
                }
                "--help" => return Err("usage: agentcode-worktable [--rows N] [--dir PATH]".into()),
                other => return Err(format!("unrecognised argument {other}")),
            }
        }
        Ok(Self { rows, dir })
    }
}

/// Runs the profile and emits one JSON object per phase.
///
/// In this module rather than the binary because `worktable!` expands its
/// tables here, and the generated types are what the phases are written
/// against.
pub fn run(config: &Config) {
    let rows = config.rows;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    // ---- in memory, the floor ----

    // `put_symbols` as written today: a dedup lookup on the unique index, then
    // a single insert, once per symbol.
    {
        let table = memory::MemPostingWorkTable::default();
        let started = Instant::now();
        for i in 0..rows {
            if table.select_by_posting_hash(1_000_000 + i).is_none() {
                futures::executor::block_on(table.insert(memory::row(i, 1))).expect("insert");
            }
        }
        emit(
            "memory",
            "put_symbols_one_at_a_time",
            rows,
            started.elapsed().as_nanos(),
        );
    }

    // The same work through `insert_many`, which is what Ask 1 asks for. Rows
    // are built before the clock starts: AgentCode already has the complete
    // set in hand, and building it is not the cost under discussion.
    {
        let table = memory::MemPostingWorkTable::default();
        let batch: Vec<_> = (0..rows).map(|i| memory::row(i, 1)).collect();
        let started = Instant::now();
        futures::executor::block_on(table.insert_many(batch)).expect("insert_many");
        emit(
            "memory",
            "put_symbols_insert_many",
            rows,
            started.elapsed().as_nanos(),
        );
    }

    // `load base symbols`: the whole generation read back, which is 12% of an
    // update on its own.
    {
        let table = memory::MemPostingWorkTable::default();
        futures::executor::block_on(
            table.insert_many((0..rows).map(|i| memory::row(i, 1)).collect()),
        )
        .expect("fixture");
        let started = Instant::now();
        let mut seen = 0u64;
        for i in 0..rows {
            if table.select(i).is_some() {
                seen += 1;
            }
        }
        let elapsed = started.elapsed();
        assert_eq!(seen, rows, "the generation must read back whole");
        emit("memory", "load_base_symbols", rows, elapsed.as_nanos());
    }

    // ---- persisted, which is what AgentCode actually runs ----

    runtime.block_on(async {
        {
            let table = disk::table(&format!("{}/one-at-a-time", config.dir)).await;
            let started = Instant::now();
            for i in 0..rows {
                if table.select_by_posting_hash(1_000_000 + i).is_none() {
                    table.insert(disk::row(i, 1)).await.expect("insert");
                }
            }
            // Caller-visible cost, matching the memory arm.
            emit(
                "disk",
                "put_symbols_one_at_a_time",
                rows,
                started.elapsed().as_nanos(),
            );
            // And the same arm drained, so it can be compared against the
            // batch arm on equal terms. Timing one to acceptance and the other
            // to durability would flatter whichever was drained.
            let started = Instant::now();
            table.wait_for_ops().await.expect("drain");
            emit(
                "disk",
                "wait_for_ops_one_at_a_time",
                rows,
                started.elapsed().as_nanos(),
            );
            table.close().await.expect("close");
        }

        {
            let table = disk::table(&format!("{}/insert-many", config.dir)).await;
            let batch: Vec<_> = (0..rows).map(|i| disk::row(i, 1)).collect();
            let started = Instant::now();
            table.insert_many(batch).await.expect("insert_many");
            emit(
                "disk",
                "put_symbols_insert_many",
                rows,
                started.elapsed().as_nanos(),
            );

            // And again including the drain, because "accepted by the queue" is
            // not the number AgentCode feels at the end of a generation.
            let started = Instant::now();
            table.wait_for_ops().await.expect("drain");
            emit(
                "disk",
                "wait_for_ops_after_batch",
                rows,
                started.elapsed().as_nanos(),
            );
            table.close().await.expect("close");
        }

        {
            let table = disk::table(&format!("{}/load", config.dir)).await;
            table
                .insert_many((0..rows).map(|i| disk::row(i, 1)).collect())
                .await
                .expect("fixture");
            table.wait_for_ops().await.expect("drain");
            let started = Instant::now();
            let mut seen = 0u64;
            for i in 0..rows {
                if table.select(i).is_some() {
                    seen += 1;
                }
            }
            let elapsed = started.elapsed();
            assert_eq!(seen, rows, "the generation must read back whole");
            emit("disk", "load_base_symbols", rows, elapsed.as_nanos());
            table.close().await.expect("close");
        }
    });

    let _ = std::fs::remove_dir_all(&config.dir);
}
