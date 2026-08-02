# wt-benchmarks

Reproducible performance, scalability, correctness, and resource testing for
[WorkTable](https://github.com/pathscale/WorkTable). This repository is
deliberately separate from WorkTable so benchmark code, external adapters,
large workload generators, environment manifests, and reviewed results can
evolve without adding weight to the library.

The suite covers four different questions:

1. What does WorkTable cost relative to the Rust data structure an application
   would otherwise use?
2. How does it behave under recognized portable workload shapes?
3. Does it meet the latency, throughput, startup, and memory needs of real HFT,
   desktop, and SaaS applications?
4. Do stronger feature-gated guarantees change performance or tail latency?

See [the benchmark catalog](docs/BENCHMARK_CATALOG.md) for the port queue and
[the methodology](docs/METHODOLOGY.md) for reporting rules.

The [comparison layers](docs/COMPARISON_LAYERS.md) keep raw Rust collections,
concurrent maps, in-process tables, and durable stores from being presented as
if they offered identical semantics. [Result tracks](docs/RESULT_TRACKS.md)
define the deliberately narrow paper figures and the broader worktable.dev
benchmark publication.

## Current runnable port: YCSB A-F

The first port implements the operation mixes from the Apache-licensed Yahoo!
Cloud Serving Benchmark. It uses a ten-field, approximately 1 KiB record and
pre-generates each operation stream outside the timed region.

```bash
# Development-sized run
cargo run --release --bin ycsb-worktable -- \
  --workload A --records 100000 --operations 1000000 \
  --threads 1 --repetitions 5

# Strong concurrent-read mode
cargo run --release --features versioned-row-publication \
  --bin ycsb-worktable -- \
  --workload B --records 1000000 --operations 5000000 \
  --threads 8 --repetitions 5
```

Results are emitted as JSON Lines, one record per repetition. Run `--help` for
all parameters. The path dependency in `Cargo.toml` intentionally targets the
local sibling WorkTable checkout while the benchmark APIs are still changing;
replace it with a released version or pinned Git revision before publishing.

The runner refuses multi-threaded A/B/D/E/F runs unless
`versioned-row-publication` is enabled. WorkTable's default page path requires
the application to exclude reads overlapping page mutation, so silently
running those mixes would benchmark outside its documented contract. Workload
C is read-only and may run concurrently in either mode.

For a complete local sweep:

```bash
# Default mode: thread 1 for mixed workloads, any requested threads for C.
THREADS="1 2 4 8" scripts/run-ycsb-matrix.sh

# Strong publication mode: all requested thread counts for all A-F workloads.
MODE=versioned THREADS="1 2 4 8" scripts/run-ycsb-matrix.sh
```

The script stores ignored raw JSONL plus a matching environment capture under
`results/`; review them before force-adding any curated result.

## Runnable comparison ladder

```bash
cargo run --release --bin micro-layers -- \
  --rows 1000000 --operations 1000000 \
  --scan-operations 10000 --scan-length 100 --repetitions 5
```

The initial ladder includes `Vec<T>`, `Vec<RwLock<T>>`, `HashMap`, `BTreeMap`,
`RwLock<HashMap>`, DashMap, and WorkTable. Reads are explicitly labeled
borrowed or materialized in JSONL; do not combine those bars without explaining
the semantic difference.

`scripts/run-micro-matrix.sh` sweeps payload and range widths with an attached
environment manifest. The complete official-run ordering and time budget are in
[RUN_CAMPAIGN.md](docs/RUN_CAMPAIGN.md).

The first external-store adapter is feature-gated:

```bash
cargo run --release --features redb-adapter --bin kv-redb -- \
  --rows 100000 --operations 100000 \
  --durability relaxed --transaction-scope per-operation
```

Both durability (`relaxed` versus `durable`) and transaction scope
(`per-operation` versus `batch`) are emitted with every result. They are
different semantic experiments and must remain separate in charts.

## Repository rules

- Never publish an unreviewed one-shot number.
- Preserve per-repetition results; summarize with medians and confidence
  intervals rather than selecting a favorable run.
- Compare like semantics and clearly label durability, concurrency, caching,
  and feature modes.
- Do not use TPC names, implementations, metrics, or comparability language in
  published results without satisfying the TPC fair-use policy and obtaining
  any required permission.
