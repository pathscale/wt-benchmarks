# Concurrent YCSB D index-backend resolution

This is a local ARM64 correctness and performance screening result, not a
paper-ready hardware claim. It tests the acknowledged-insert Workload D
generator against the index providers proposed by WorkTable PR #187 and the
`versioned-row-publication` integration.

## Result

There are two viable configurations for concurrent WorkTable D:

1. For memory-only tables, use `persist: false` with `using congee` or
   `using arctic`. Neither ART provider produced a first-attempt miss in the
   tested matrix, so no retry workaround is required.
2. For WorkTablesIndex or IndexSet, enable the separately gated
   `stable-index-read-retry` feature. It confirms an apparently stable point
   miss with one bounded additional probe. The feature implies
   `versioned-row-publication` and remains off by default.

The normal HFT path is therefore unchanged. Successful point hits still use
one index probe. A true negative lookup uses two probes only when the retry
feature is enabled.

## Evidence

Feature-off tests covered 102 million combined WorkTablesIndex and IndexSet
operations. They observed 501 transient first-attempt misses. Every one was
found by exactly one immediate retry; no insert failed and no retry remained a
final miss. The failures occurred only at 8 or more workers in the thread
sweep.

Feature-off Congee and Arctic covered 42 million combined operations across
the same 1/2/4/8/16/32-thread sweep, 16-byte diagnostic rows, and approximately
1 KiB YCSB rows. They produced zero first-attempt misses, final read errors, or
insert errors.

The current gated retry build covered 95 million operations across all four
providers. WorkTablesIndex and IndexSet were additionally tested with
approximately 1 KiB rows at 8, 16, and 32 threads. It produced zero externally
visible first-attempt misses, final read errors, or insert errors.

These operation totals include the 5% inserts in Workload D; they are not
counts of point reads alone.

## Paired performance check

The longer performance confirmation interleaved feature-off and feature-on
binaries for 15 pairs per B-tree provider, with 2 million operations per run,
8 threads, 10,000 initial rows, and 16-byte fields.

| Provider | Feature-off median | Feature-on median | Median paired delta | Read p50 off/on | Read p99 off/on |
|---|---:|---:|---:|---:|---:|
| WorkTablesIndex | 2.990M ops/s | 3.013M ops/s | +0.30% | 1.125/1.125 us | 5.375/4.709 us |
| IndexSet | 3.027M ops/s | 3.091M ops/s | +1.58% | 1.125/1.125 us | 4.875/4.041 us |

This local run found no measurable hot-path regression. Individual paired
deltas were noisy, ranging from -2.53% to +4.00% for WorkTablesIndex and
-2.30% to +6.46% for IndexSet. Controlled AWS ARM runs with confidence
intervals remain required before making a performance claim.

## Publication boundary

Concurrent WorkTable D no longer needs to be categorically excluded. Publish
it only with the exact provider and feature mode in the configuration:

- ART result: `persist: false`, `using congee` or `using arctic`, and
  `versioned-row-publication`.
- Persistent-capable B-tree result: WorkTablesIndex or IndexSet with
  `stable-index-read-retry`.

Do not present feature-off WorkTablesIndex or IndexSet concurrent D as a valid
correctness run. Do not infer range-scan behavior from these point-heavy
results: the current Congee and Arctic range adapters allocate snapshots, and
those providers do not support WorkTable persistence.

The raw JSONL files under `results/` remain ignored until a controlled run is
reviewed and selected for curation.
