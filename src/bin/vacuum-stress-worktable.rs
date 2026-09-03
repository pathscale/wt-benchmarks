//! What vacuum costs the foreground while it runs.
//!
//! ```sh
//! cargo run --release --bin vacuum-stress-worktable
//! cargo run --release --bin vacuum-stress-worktable -- --rows 50000 --arm-secs 5
//! ```
//!
//! Emits one JSON object per arm. Each cell appears twice, once with vacuum
//! stopped and once with it running, and the delta between them is the
//! measurement.

use wt_benchmarks::vacuum_stress::{Config, run_all};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let config = match Config::from_args() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: debug build. Per-call overhead swamps what vacuum costs. Use --release."
        );
    }
    run_all(&config).await;
}
