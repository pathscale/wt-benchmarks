//! One row of a benchmark grid, and the JSON it prints.
//!
//! # Why this exists
//!
//! Every dimension anybody has asked for in this study, page size, thread
//! count, runtime, flavor, dispatch mode, was added to one benchmark at a
//! time and then extracted from its stdout with `awk`. That went wrong
//! repeatedly and in ways that inverted conclusions: a column index off by one
//! compared uniform's *upserts* against split's *total* and produced a table
//! that was wrong in every row.
//!
//! So a benchmark emits a [`GridRow`] per cell, as one JSON object per line,
//! and nothing downstream parses columns. Set `WT_JSON=1` to get it.
//!
//! **Every dimension is a field on every row, always**, even when a benchmark
//! does not vary it. A grid where some rows lack `page_size` cannot be pivoted
//! without special cases, and the special cases are where the mistakes live.

use core::sync::atomic::{AtomicU32, Ordering};

use serde::Serialize;

use crate::result::LatencySummary;

/// Which repetition the benchmark is on.
///
/// A process-wide counter rather than a parameter because rows are built
/// inside macros that expand where the round index is not in scope. Set it
/// once per round; every row emitted until the next call carries it.
static REPETITION: AtomicU32 = AtomicU32::new(1);

/// Call once at the top of each round, with the round number from 1.
pub fn set_repetition(round: u32) {
    REPETITION.store(round, Ordering::Relaxed);
}

/// The round rows are currently being emitted for.
#[must_use]
pub fn repetition() -> u32 {
    REPETITION.load(Ordering::Relaxed)
}

/// One measured cell.
#[derive(Debug, Clone, Serialize)]
pub struct GridRow {
    pub schema_version: u32,
    /// Which benchmark produced this, so rows from several can share a file.
    pub suite: &'static str,
    /// `nagoya` or `tokio`.
    pub runtime: String,
    /// The full tuning, not the flavor name: a run that set `WT_BACKOFF` is
    /// not running the flavor it would otherwise be filed under.
    pub tuning: String,
    /// `inline` or `pool`, where the benchmark has both.
    pub dispatch: &'static str,
    /// Which index backend the table's secondary index uses.
    ///
    /// A dimension because it is a choice made per table and the backends are
    /// not interchangeable in performance: a comparison that leaves it out is
    /// really a comparison of whichever one happened to be the default. It
    /// also decides which crate the numbers belong to, which matters when a
    /// change lands in `WorkTablesIndex` rather than in `worktable`.
    pub index_backend: &'static str,
    /// Which repetition of this cell, from 1.
    ///
    /// **A cell measured once has no error bar and must not be compared to
    /// anything.** Every wrong conclusion this study has produced came from a
    /// single sample or a median of three: a page-size effect reported at +31%
    /// that collapsed to noise at eleven rounds, and a flavor read as 1.85x
    /// better on seven repetitions that was 1.002x on sixteen. So the
    /// repetition is a field on the row, and the summariser refuses to name a
    /// winner between cells whose ranges overlap.
    pub repetition: u32,
    pub page_size: Option<u32>,
    pub worker_threads: usize,
    pub readers: usize,
    pub writers: usize,
    pub ops_per_task: u64,
    pub elapsed_ns: u128,
    pub ops_per_second: f64,
    /// Reads and writes separately, because a combined figure cannot answer a
    /// release question. A table doing 3M ops/s that is 95% selects is a very
    /// different proposition from one that is half upserts, and the two sides
    /// have different costs and different tails.
    pub read_ops_per_second: f64,
    pub write_ops_per_second: f64,
    /// Average cores busy: `(user + system) / real`. Without it a low number
    /// cannot be told apart from an idle one.
    pub cpu_x: f64,
    pub read_latency: LatencySummary,
    pub write_latency: LatencySummary,
    pub target_arch: &'static str,
    pub target_os: &'static str,
}

impl GridRow {
    /// Whether the process was asked for JSON rather than a table.
    #[must_use]
    pub fn wanted() -> bool {
        std::env::var("WT_JSON").is_ok_and(|value| value != "0")
    }

    /// Print this row as one JSON object, if JSON was asked for.
    pub fn emit(&self) {
        if Self::wanted() {
            println!("{}", serde_json::to_string(self).expect("a grid row must serialize"));
        }
    }
}
