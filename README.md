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

## Runnable workload ports

### YCSB A-F

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

The same A-F streams can run through SQLite `:memory:`:

```bash
cargo run --release --features sqlite-adapter --bin ycsb-sqlite -- \
  --workload A --records 100000 --operations 1000000 \
  --threads 1 --repetitions 5
```

For concurrent SQLite runs, each worker gets a connection to the same
shared-memory database. `retryable_errors` reports every SQLite busy/locked
retry, including successful retries; `errors` remains the final failed
operation count. Workload D resolves reads against an execution-time frontier
that advances only after every preceding insert has succeeded, matching YCSB's
acknowledged-counter semantics instead of treating pre-generated future keys as
readable. SQLite's concurrent D correctness run now completes without misses.
The WorkTable index-backend campaign found two valid concurrent-D classes: the
stabilized WorkTablesIndex candidate confirms provider misses internally, and
memory-only Congee/Arctic produced no transient misses. Vanilla IndexSet is
retained only as an experimental quiescent/single-thread baseline. See
[`docs/YCSB_D_CONCURRENT_INDEX_BACKENDS.md`](docs/YCSB_D_CONCURRENT_INDEX_BACKENDS.md)
for the correctness matrix, paired performance screening, and publication
boundaries.

The runner refuses multi-threaded A/B/E/F runs unless
`versioned-row-publication` is enabled. WorkTable's default page path requires
the application to exclude reads overlapping page mutation, so silently
running those mixes would benchmark outside its documented contract. Workload
C is read-only and may run concurrently in either mode. Concurrent Workload D
also needs a safe index configuration, so the standard versioned sweep keeps D
at one thread until the stabilized dependencies are published and selected.
Use `campaigns/ycsb-index-backends` for stabilized WorkTablesIndex or ART
configurations. `ALLOW_UNSAFE_CONCURRENT_D=true`
exists only to reproduce the acknowledged transient-miss diagnostic on older
compositions.

For a complete local sweep:

```bash
# Default mode: thread 1 for mixed workloads, any requested threads for C.
THREADS="1 2 4 8" scripts/run-ycsb-matrix.sh

# Strong page-publication mode: all threads for A/B/C/E/F; D stays at thread 1.
MODE=versioned THREADS="1 2 4 8" scripts/run-ycsb-matrix.sh

# SQLite shared-memory comparison: all requested thread counts for all A-F.
MODE=sqlite THREADS="1 2 4 8" scripts/run-ycsb-matrix.sh
```

The script stores ignored raw JSONL plus a matching environment capture under
`results/`; review them before force-adding any curated result.

Criterion comparisons from WorkTable itself can be normalized to JSONL with:

```bash
scripts/summarize-criterion-changes.sh /path/to/cargo-target
```

The input target must contain Criterion `change/estimates.json` files produced
by a saved baseline comparison. Negative changes are faster elapsed time.

## Partition routing under churn

The tick loop WorkTable's partitioned tables exist for: readers route to a
partition by key and read from it, while a writer removes and recreates
partitions underneath them.

```bash
cargo bench --bench partition_ticks

# Adds the batched `pinned` strategy. Needs worktable 1.0.0-beta.16 or newer.
cargo bench --features partition-pinned --bench partition_ticks
```

It exists because WorkTable's own `partition_routing` benchmark measures the
routing call in isolation, and isolation hides the two things that decide what
it costs. It does no table work, so a difference worth 15% of the routing call
is worth under 2% of the tick that contains it. And it never retires anything,
so the grace period is never exercised and every reclamation scheme looks
alike, including the ones that stop reclaiming entirely once readers stop
arriving in gaps.

This port does real reads through the routing call and churns partitions while
the readers run. `Outcome::retired_backlog` is the number to watch: a backlog
that tracks the churn count means retirements are piling up behind readers that
keep arriving, which is the failure mode the reclamation scheme was chosen to
avoid.

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

The embedded-store adapters are feature-gated:

```bash
cargo run --release --features redb-adapter --bin kv-redb -- \
  --rows 100000 --operations 100000 \
  --durability relaxed --transaction-scope per-operation

cargo run --release --features sqlite-adapter --bin kv-sqlite -- \
  --rows 100000 --operations 100000 \
  --durability memory --transaction-scope per-operation
```

WorkTable, SQLite, and redb execute the same five logical phases and produce
matching checksums. Durability (`memory`, `relaxed`, or `durable`) and
transaction scope (`per-operation` versus `batch`) are emitted with every
result. They are different semantic experiments and must remain separate in
charts.

## Embedded and application-shaped ports

Four additional WorkTable runners are executable now:

```bash
# Shared embedded key/value shape: insert, point read, overwrite, range, delete.
cargo run --release --bin kv-worktable -- \
  --rows 100000 --operations 100000 --scan-operations 10000 \
  --scan-length 100 --repetitions 5

# Public-domain SQLite speedtest1-inspired core operation shapes.
cargo run --release --bin speedtest1-worktable -- \
  --rows 100000 --operations 100000 --repetitions 5

# The same nine phases and deterministic inputs through SQLite :memory:.
cargo run --release --features sqlite-adapter --bin speedtest1-sqlite -- \
  --rows 100000 --operations 100000 --repetitions 5

# Apache-2.0 LinkBench operation mix over a synthetic Zipf graph.
cargo run --release --bin linkbench-worktable -- \
  --nodes 100000 --links-per-node 20 --operations 1000000 \
  --repetitions 5

# BenchBase TATP's four-table telecom workload and seven-transaction mix.
cargo run --release --bin tatp-worktable -- \
  --subscribers 100000 --operations 1000000 --threads 1 \
  --repetitions 5
```

These are independent Rust implementations; no upstream driver code is copied.
The paired speedtest1 runners use the same inputs and nine phase names; their
checksums must match. SQLite is explicitly `:memory:` with one autocommit
statement per operation. Neither runner claims unsupported SQL groups,
transactional bulk inserts, or durable-mode equivalence. The LinkBench runner
preserves the published request mix, but its current graph uses a synthetic
Zipf source distribution rather than LinkBench's empirical degree
distribution. Those limitations are embedded in every JSONL result and tracked
in [PORT_STATUS.md](docs/PORT_STATUS.md).

The TATP runner implements subscriber, access-info, special-facility, and
call-forwarding tables plus all seven canonical procedures at BenchBase's
2/35/10/35/2/14/2 weights. Duplicate inserts, missing facilities, and missing
deletes are reported as expected aborts rather than benchmark errors. Its
multi-table procedures are compiled application Rust and explicitly do not
claim automatic cross-table atomicity. Concurrent read/write runs require the
`versioned-row-publication` feature, just like mixed YCSB runs.

Run the complete KV, speedtest1-shape, LinkBench, and TATP application matrix
with matching environment capture and cross-engine checksum validation using:

```bash
scripts/run-application-matrix.sh
```

The default matrix keeps in-memory WorkTable and SQLite separate from relaxed
redb, records both per-operation and batch redb transactions, and runs TATP at
1, 4, and 8 threads. Every dimension can be overridden with the
`CAMPAIGN_*` variables at the top of the script.

## Consumer profiles

Workload shapes taken from a real WorkTable consumer rather than from a
published benchmark. They answer a narrower question than the ports above: not
how WorkTable compares to another engine, but whether a change to WorkTable is
about to break something that already depends on it.

### codegraph, the agentcode storage profile

```bash
cargo bench --bench codegraph
```

`pathscale/agentcode` is a semantic code index that republishes a whole
generation of facts about a repository on every source change: 43,200 persisted
rows across three tables at its production fixture, every row tagged with one
hot generation key, plus a graph adjacency walked on every query.

Four groups: `publish` (persisted against memory, the 22x durable-write ratio
that dominates this consumer), `incremental` (one file changing against a
populated store), `generation_scan` (the hot non-unique key, whose fan-out is
the whole generation), and `dependency_walk` (the per-call adjacency).

Read `publish` as a ratio and `generation_scan` as a slope, not as absolutes.
Full documentation, including what it deliberately does not cover, is in
[CODEGRAPH_PROFILE.md](docs/CODEGRAPH_PROFILE.md).

Three WorkTable releases in a row shipped a regression this consumer felt and no
benchmark caught. That is what this profile is for.

## Repository rules

- Never publish an unreviewed one-shot number.
- Preserve per-repetition results; summarize with medians and confidence
  intervals rather than selecting a favorable run.
- Compare like semantics and clearly label durability, concurrency, caching,
  and feature modes.
- Do not use TPC names, implementations, metrics, or comparability language in
  published results without satisfying the TPC fair-use policy and obtaining
  any required permission.
