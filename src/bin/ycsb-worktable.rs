use wt_benchmarks::config::Config;
use wt_benchmarks::ycsb::{Workload, run_repetition};

// nagoya drives this, not tokio. The AFTER arm must not link the runtime it is
// being measured against replacing: the client tasks and the engine's own
// background work would otherwise sit on different schedulers, and the number
// would describe neither stack.
fn main() {
    nagoya::block_on(run());
}

async fn run() {
    let config = Config::from_args().unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    if config.threads > 1
        && config.workload != Workload::C
        && !cfg!(feature = "versioned-row-publication")
    {
        eprintln!(
            "error: concurrent YCSB {} mixes reads with page mutation; rerun with \
             --features versioned-row-publication or use --threads 1",
            config.workload
        );
        std::process::exit(2);
    }

    // Say which pool this is, before any number is printed. A silent env
    // override is a debugging trap, and an arm that fell back to the default
    // looks exactly like an arm that did not.
    eprintln!(
        "runtime: {}, workers: {}",
        worktable::prelude::describe_tuning(worktable::prelude::engine_flavor()),
        std::env::var("WT_RUNTIME_WORKERS")
            .unwrap_or_else(|_| std::thread::available_parallelism()
                .map_or(0, std::num::NonZeroUsize::get)
                .to_string()),
    );

    for repetition in 1..=config.repetitions {
        let result = run_repetition(&config, repetition).await;
        println!(
            "{}",
            serde_json::to_string(&result).expect("result must serialize")
        );
    }
}
