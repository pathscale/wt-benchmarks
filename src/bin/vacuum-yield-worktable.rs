//! Does vacuum stay out of the way, and does it still return everything?
//!
//! ```sh
//! cargo run --release --bin vacuum-yield-worktable
//! cargo run --release --bin vacuum-yield-worktable -- --rows 100000 --load-secs 3
//! ```
//!
//! Two phases per arm. Under sustained upsert and delete pressure, vacuum
//! should defer: the foreground's latency has to match the identical arm with
//! vacuum stopped, and any daylight between them is vacuum interfering. Then
//! the load stops and the arm stays open until vacuum has finished reclaiming,
//! because a sweep that waits politely and then never returns the memory has
//! failed just as surely as one that got in the way.

use wt_benchmarks::vacuum_yield::{Config, run_all};

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
        eprintln!("warning: debug build. Per-call overhead swamps what vacuum costs. Use --release.");
    }
    run_all(&config).await;
}
