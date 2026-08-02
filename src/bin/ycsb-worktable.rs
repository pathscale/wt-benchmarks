use wt_benchmarks::config::Config;
use wt_benchmarks::ycsb::{Workload, run_repetition};

#[tokio::main]
async fn main() {
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

    for repetition in 1..=config.repetitions {
        let result = run_repetition(&config, repetition).await;
        println!(
            "{}",
            serde_json::to_string(&result).expect("result must serialize")
        );
    }
}
