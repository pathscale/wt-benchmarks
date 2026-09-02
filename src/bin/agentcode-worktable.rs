//! The AgentCode write profile: bulk persisted writes, and what `insert_many`
//! removes from them.
//!
//! ```sh
//! cargo run --release --bin agentcode-worktable
//! cargo run --release --bin agentcode-worktable -- --rows 5000
//! ```

use wt_benchmarks::agentcode::{Config, run};

fn main() {
    let config = match Config::from_args() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build. Use --release or the persisted ratio is meaningless.");
    }
    run(&config);
}
