//! contention-paper — paper-grade lock-granularity contention (C2).
//!
//! Runs ONE cell per process (a single lock mode x write-set x thread count),
//! emitting one JSONL record to stdout. Fresh-process-per-cell isolation and
//! the full sweep are the runner's job (scripts/run-contention-matrix.sh),
//! which also captures the environment manifest.
//!
//! The load-bearing invariant: every mode drives the *same* logical operation
//! — a fixed number of u64-field increments against one hot row — and differs
//! ONLY in the lock discipline. Because the work is identical, cross-mode
//! throughput ratios attribute to the lock structure and nothing else.
//!
//! Correctness: each increment is +1 to a target field, so after the run the
//! sum of the touched fields must equal the number of committed operations.
//! Any shortfall is a lost update. `lost_updates` records it; `passed`
//! reflects it. A throughput number from a cell that lost updates is not a
//! valid comparison point, so the runner can drop it.
//!
//! Modes (see lib.rs for the schema):
//!   field_granular — worker i updates field (i % CONTENTION_FIELDS): the
//!                    generated per-column lock; distinct workers never collide.
//!   overlap        — every worker updates field f0: identical write sets, so
//!                    the field locks fully serialize (upper bound on the
//!                    field-granular structure's own cost).
//!   whole_row      — every worker takes the all-columns lock (UpdAll): the
//!                    honest coarse baseline, WorkTable's own machinery
//!                    serializing the whole field set.
//!   single_mutex   — the field_granular call wrapped in one external
//!                    tokio::Mutex: reference line for a naive coarse lock,
//!                    labeled as such (NOT presented as WorkTable).
//!
//! Usage:
//!   contention-paper --mode <field_granular|overlap|whole_row|single_mutex>
//!                    --threads N [--ops-per-thread M] [--repetition R]
//!                    [--warmup-ops W]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use wt_contention_campaign::*;

const SCHEMA_VERSION: u32 = 1;
const SUITE: &str = "contention";

#[derive(Clone, Copy)]
enum Mode {
    FieldGranular,
    Overlap,
    WholeRow,
    SingleMutex,
}

impl Mode {
    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "field_granular" => Mode::FieldGranular,
            "overlap" => Mode::Overlap,
            "whole_row" => Mode::WholeRow,
            "single_mutex" => Mode::SingleMutex,
            other => return Err(format!("unknown --mode {other}")),
        })
    }
    fn label(self) -> &'static str {
        match self {
            Mode::FieldGranular => "field_granular",
            Mode::Overlap => "overlap",
            Mode::WholeRow => "whole_row",
            Mode::SingleMutex => "single_mutex",
        }
    }
    /// The write-set shape this mode exercises, recorded for the paper axis.
    fn write_set(self) -> &'static str {
        match self {
            Mode::FieldGranular => "disjoint",
            Mode::Overlap => "overlapping",
            Mode::WholeRow => "overlapping",
            Mode::SingleMutex => "disjoint",
        }
    }
    fn lock_discipline(self) -> &'static str {
        match self {
            Mode::FieldGranular => "generated_per_column",
            Mode::Overlap => "generated_per_column",
            Mode::WholeRow => "generated_all_columns",
            Mode::SingleMutex => "external_mutex",
        }
    }
}

struct Args {
    mode: Mode,
    threads: usize,
    ops_per_thread: u64,
    warmup_ops: u64,
    repetition: usize,
    /// Number of distinct hot rows workers spread across. Defaults to the thread
    /// count so field_granular has one row per worker (nothing to serialize on),
    /// while overlap/single_mutex still contend. A single hot row (the old
    /// behavior, --hot-rows 1) cannot show granular lock scaling.
    hot_rows: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut mode = None;
    let mut threads = None;
    let mut ops_per_thread = 200_000u64;
    let mut warmup_ops = 20_000u64;
    let mut repetition = 1usize;
    let mut hot_rows = None;

    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        if flag == "-h" || flag == "--help" {
            eprintln!(
                "contention-paper --mode <field_granular|overlap|whole_row|single_mutex> \
                 --threads N [--ops-per-thread M] [--warmup-ops W] [--repetition R] \
                 [--hot-rows H]"
            );
            std::process::exit(0);
        }
        let val = it.next().ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--mode" => mode = Some(Mode::parse(&val)?),
            "--threads" => threads = Some(val.parse().map_err(|_| "bad --threads")?),
            "--ops-per-thread" => ops_per_thread = val.parse().map_err(|_| "bad --ops-per-thread")?,
            "--warmup-ops" => warmup_ops = val.parse().map_err(|_| "bad --warmup-ops")?,
            "--repetition" => repetition = val.parse().map_err(|_| "bad --repetition")?,
            "--hot-rows" => hot_rows = Some(val.parse().map_err(|_| "bad --hot-rows")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let threads = threads.ok_or("--threads is required")?;
    Ok(Args {
        mode: mode.ok_or("--mode is required")?,
        threads,
        ops_per_thread,
        warmup_ops,
        repetition,
        hot_rows: hot_rows.unwrap_or(threads).max(1),
    })
}

#[derive(Serialize)]
struct CellResult {
    schema_version: u32,
    suite: &'static str,
    // paper axes
    lock_mode: &'static str,
    write_set: &'static str,
    lock_discipline: &'static str,
    threads: usize,
    repetition: usize,
    // workload
    hot_rows: usize,
    ops_per_thread: u64,
    warmup_ops: u64,
    operations_completed: u64,
    // outcome
    elapsed_ns: u128,
    ops_per_second: f64,
    // correctness: sum of touched fields must equal committed ops
    expected_sum: u64,
    observed_sum: u64,
    lost_updates: u64,
    errors: u64,
    passed: bool,
    // provenance
    feature_versioned_row_publication: bool,
    target_arch: &'static str,
    target_os: &'static str,
}

/// Atomically increment worker `worker`'s owned field by one, via the
/// generated `in_place` closure for that field. This is the SAME atomic RMW
/// for every field; only which field (hence which per-column lock) differs.
async fn inc_field(
    table: &BenchWorkTable,
    worker: usize,
    pk: u64,
) -> eyre::Result<()> {
    match disjoint_field(worker) {
        0 => table.update_inc_f_0_in_place(|f| *f += 1, pk).await.map(|_| ()),
        1 => table.update_inc_f_1_in_place(|f| *f += 1, pk).await.map(|_| ()),
        2 => table.update_inc_f_2_in_place(|f| *f += 1, pk).await.map(|_| ()),
        3 => table.update_inc_f_3_in_place(|f| *f += 1, pk).await.map(|_| ()),
        4 => table.update_inc_f_4_in_place(|f| *f += 1, pk).await.map(|_| ()),
        5 => table.update_inc_f_5_in_place(|f| *f += 1, pk).await.map(|_| ()),
        6 => table.update_inc_f_6_in_place(|f| *f += 1, pk).await.map(|_| ()),
        _ => table.update_inc_f_7_in_place(|f| *f += 1, pk).await.map(|_| ()),
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };

    let feature_versioned = cfg!(feature = "versioned-row-publication");

    let table = Arc::new(BenchWorkTable::default());
    // Insert `hot_rows` distinct rows. Workers spread across them so
    // field_granular has genuine row-level parallelism; a single row would make
    // every worker contend on one row's pages/publication regardless of mode.
    let pk_vals: Arc<Vec<u64>> = Arc::new(
        (0..args.hot_rows)
            .map(|r| table.insert(mk_row(&table, r as u64)).unwrap().into())
            .collect(),
    );

    // Warmup: touch every hot row so pages/locks/publication maps are hot. Not
    // timed, not counted toward the correctness sum (we zero the fields after).
    for i in 0..args.warmup_ops {
        let pk = pk_vals[(i as usize) % args.hot_rows];
        let _ = inc_field(&table, (i as usize) % args.threads.max(1), pk).await;
    }
    // Reset all fields on every hot row to 0 so the post-run sum starts from a
    // known baseline, using the all-columns in_place closure (set, not inc).
    for &pk in pk_vals.iter() {
        table
            .update_inc_all_in_place(
                |(f0, f1, f2, f3, f4, f5, f6, f7)| {
                    *f0 = 0u64.into(); *f1 = 0u64.into();
                    *f2 = 0u64.into(); *f3 = 0u64.into();
                    *f4 = 0u64.into(); *f5 = 0u64.into();
                    *f6 = 0u64.into(); *f7 = 0u64.into();
                },
                pk,
            )
            .await
            .unwrap();
    }

    let completed = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let start = Arc::new(AtomicBool::new(false));
    let big_lock = Arc::new(tokio::sync::Mutex::new(()));

    let mut handles = Vec::with_capacity(args.threads);
    for i in 0..args.threads {
        let table = table.clone();
        let completed = completed.clone();
        let errors = errors.clone();
        let start = start.clone();
        let big_lock = big_lock.clone();
        let mode = args.mode;
        let ops = args.ops_per_thread;
        let pk_vals = pk_vals.clone();
        // Worker i owns hot row (i % hot_rows). With the default hot_rows =
        // threads, each worker gets a distinct row.
        let pk_val = pk_vals[i % pk_vals.len()];
        handles.push(tokio::spawn(async move {
            while !start.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
            let mut local = 0u64;
            for _ in 0..ops {
                // Each op increments a target field by 1. We express "+1" as an
                // absolute write of (current+1) is NOT safe across modes, so we
                // use the in_place closure for the granular/overlap paths where
                // available, and rely on the sum invariant below.
                // Shared row for the collapse modes: overlap, whole_row, and
                // single_mutex must contend on ONE row to show coarse locking
                // collapsing. field_granular uses the worker's own row so the
                // per-column locks give genuine row-level parallelism.
                let shared_pk = pk_vals[0];
                let op = match mode {
                    // worker i increments field (i % CONTENTION_FIELDS) on its
                    // OWN row: disjoint rows x disjoint fields -> scales.
                    Mode::FieldGranular => inc_field(&table, i, pk_val).await,
                    // every worker increments the SAME field (f0) of the SAME
                    // row: identical write sets fully serialize on one field lock
                    Mode::Overlap => {
                        table.update_inc_f_0_in_place(|f| *f += 1, shared_pk).await.map(|_| ())
                    }
                    // every worker takes the all-columns lock of the shared row,
                    // atomically incrementing all eight fields (8 increments/op)
                    Mode::WholeRow => inc_all_by_one(&table, shared_pk).await,
                    // field-granular work on the shared row wrapped in one
                    // external mutex: everything serializes globally.
                    Mode::SingleMutex => {
                        let _g = big_lock.lock().await;
                        inc_field(&table, i, shared_pk).await
                    }
                };
                if op.is_err() {
                    errors.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                local += 1;
            }
            completed.fetch_add(local, Ordering::Relaxed);
        }));
    }

    let t0 = Instant::now();
    start.store(true, Ordering::Release);
    for h in handles {
        h.await.unwrap();
    }
    let elapsed = t0.elapsed();

    let ops_completed = completed.load(Ordering::Relaxed);
    let err_count = errors.load(Ordering::Relaxed);

    // Correctness: sum the touched fields across ALL hot rows. field_granular
    // spreads increments across every worker's row; the collapse modes land
    // entirely on row 0. Summing all rows covers both.
    let observed_sum: u64 = pk_vals
        .iter()
        .map(|&pk| {
            let row = table.select(pk).expect("hot row must exist");
            row.f0 + row.f1 + row.f2 + row.f3 + row.f4 + row.f5 + row.f6 + row.f7
        })
        .sum();
    // whole_row increments all 8 fields per op; the others increment exactly 1.
    let per_op_increments: u64 = match args.mode {
        Mode::WholeRow => CONTENTION_FIELDS as u64,
        _ => 1,
    };
    let expected_sum = ops_completed * per_op_increments;
    let lost_updates = expected_sum.saturating_sub(observed_sum);

    let ops_per_second = ops_completed as f64 / elapsed.as_secs_f64();
    let passed = lost_updates == 0 && err_count == 0;

    let result = CellResult {
        schema_version: SCHEMA_VERSION,
        suite: SUITE,
        lock_mode: args.mode.label(),
        write_set: args.mode.write_set(),
        lock_discipline: args.mode.lock_discipline(),
        threads: args.threads,
        repetition: args.repetition,
        hot_rows: args.hot_rows,
        ops_per_thread: args.ops_per_thread,
        warmup_ops: args.warmup_ops,
        operations_completed: ops_completed,
        elapsed_ns: elapsed.as_nanos(),
        ops_per_second,
        expected_sum,
        observed_sum,
        lost_updates,
        errors: err_count,
        passed,
        feature_versioned_row_publication: feature_versioned,
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    };

    println!(
        "{}",
        serde_json::to_string(&result).expect("result must serialize")
    );
}

/// Atomically increment all eight contention fields by one under the
/// all-columns lock, via the multi-field `IncAll` in_place closure. The
/// closure receives a tuple of archived field refs `(&mut f0, .., &mut f7)`.
async fn inc_all_by_one(table: &BenchWorkTable, pk: u64) -> eyre::Result<()> {
    table
        .update_inc_all_in_place(
            |(f0, f1, f2, f3, f4, f5, f6, f7)| {
                *f0 += 1;
                *f1 += 1;
                *f2 += 1;
                *f3 += 1;
                *f4 += 1;
                *f5 += 1;
                *f6 += 1;
                *f7 += 1;
            },
            pk,
        )
        .await
        .map(|_| ())
}
