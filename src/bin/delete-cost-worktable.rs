//! Where the time in a single-row delete goes.
//!
//! ```sh
//! cargo run --release --bin delete-cost-worktable
//! cargo run --release --bin delete-cost-worktable -- --rows 50000
//! ```
//!
//! Emits one JSON object per rung, like the other suites. The cumulative rungs
//! are each the one before plus one step, so the cost of a step is the
//! difference between two neighbouring rungs; the rest are whole-call
//! comparisons.

use wt_benchmarks::delete_cost::{Config, run};

fn main() {
    let config = match Config::from_args() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: debug build. Every rung is dominated by unoptimised call overhead. Use --release."
        );
    }
    run(&config);
}
