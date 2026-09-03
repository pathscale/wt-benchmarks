//! Does vacuum stay out of the way, and does it still return everything?
//!
//! ```sh
//! cargo run --release --bin vacuum-yield-worktable
//! cargo run --release --bin vacuum-yield-worktable -- --rows 100000 --load-secs 3 --repetitions 6
//! ```
//!
//! Three modes run repeatedly in rotated order: vacuum stopped measures the
//! machine's null spread, reactive vacuum is the shipping policy, and unpaced
//! vacuum is the deliberately interfering positive control. Unless the
//! positive control is detectably worse than vacuum stopped, the workload
//! cannot validate the reactive result. After load stops, each vacuum arm
//! stays open until it has finished reclaiming memory.

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
        eprintln!(
            "warning: debug build. Per-call overhead swamps what vacuum costs. Use --release."
        );
    }
    run_all(&config).await;
}
