# Measurement and reporting methodology

## Two campaigns, not one compromised campaign

Every workload should have separate throughput and latency runs.

- **Throughput campaign:** sample latency sparsely (for example one operation
  in 1024) so `Instant` calls do not dominate nanosecond-scale operations.
- **Latency campaign:** use an open-loop or coordinated-omission-corrected
  driver, sample every operation, pin workers, and report p50, p95, p99,
  p99.9, maximum, and the offered/completed rate.

Do not derive a latency claim from a saturated closed-loop throughput test.

## Required experiment record

Every curated result must identify:

- repository commit for this suite, WorkTable, and every baseline;
- compiler and flags, target triple, allocator, enabled features, and profile;
- machine/instance type, CPU model, physical/logical cores, SMT, memory, NUMA,
  storage, filesystem, kernel, and power/turbo policy;
- workload, seed, record count, operation count or duration, record width,
  key/value distributions, scan lengths, index count, and thread count;
- warm/cold state, load procedure, persistence/durability settings, and whether
  time includes serialization, HTTP, network, or storage sync;
- every repetition, failures/retries, and correctness checks.

## Standard sweep

Unless a workload justifies a narrower range, sweep:

- threads: 1, 2, 4, 8, 16, 32, then physical-core count;
- cardinality: cache-resident, LLC-exceeding, and RAM-scale data sets;
- record payload: 32 B, 64 B, 256 B, 1 KiB, and 4 KiB;
- key distributions: sequential, uniform, Zipf 0.80/0.90/0.99, latest, and a
  declared hot-set fraction;
- read/write overlap: 0%, 25%, 50%, 75%, and 100%;
- WorkTable default and every performance-relevant feature gate;
- at least 5 measured repetitions for development and 10 for paper-quality
  results, randomized/interleaved across contenders.

Use a sufficiently long steady state, report median and bootstrap 95%
confidence intervals, and retain raw per-repetition JSONL. A 2% difference is
material when the interval excludes zero; do not call it noise by default.

## Fair baseline rules

Maintain two different comparison groups.

1. **Capability-matched:** same in-memory semantics, indexes, concurrency, and
   durability. This is the primary performance comparison.
2. **Familiar alternatives:** `Vec`, `HashMap`, `BTreeMap`, DashMap, SQLite,
   redb, LMDB/heed, RocksDB, and other systems applications actually consider.
   Report their different semantics in a capability table.

Prepared SQL and engine-only WorkTable calls are not the same measurement.
When useful, publish both engine-only and end-to-end results. Never silently
disable durability for one contender, include network transport for only one,
or compare a keyed lookup with a full scan.

## Correctness before speed

Every mixed/concurrent workload needs online or post-run invariants: no ghost
rows, unique-index agreement, row/index cardinality agreement, monotonic order
state, balance conservation where applicable, no resurrection after delete,
and successful persistence/reload. A run with an invariant failure is invalid,
not merely slower.

