# Concurrent YCSB D index-backend resolution

This is a local ARM64 correctness and performance screening result, not a
paper-ready hardware claim. It tests the acknowledged-insert Workload D
generator against the index providers proposed by WorkTable PR #187 and the
`versioned-row-publication` integration.

## Result

There are two viable configurations for concurrent WorkTable D:

1. Use the stabilized WorkTablesIndex candidate, whose optimistic point-read
   path confirms only apparent misses against a stable three-node boundary
   window. This is the default provider contract, not a retry feature.
2. For memory-only tables, use `persist: false` with `using congee` or
   `using arctic`. Neither ART provider produced a first-attempt miss in the
   tested matrix, so no retry workaround is required.
The normal WorkTablesIndex HFT hit path is unchanged: successful point hits use
one index probe. A true negative lookup enters cold structural confirmation.
Upstream IndexSet remains available only as an experimental quiescent or
single-thread baseline; it does not gate concurrent correctness or published
claims.

## Evidence

Before the provider-level WorkTablesIndex stabilization, feature-off tests
covered 102 million combined WorkTablesIndex and IndexSet operations. They
observed 501 transient first-attempt misses. Every one was found by exactly one
immediate retry; no insert failed and no retry remained a final miss. The
failures occurred only at 8 or more workers in the thread sweep.

Feature-off Congee and Arctic covered 42 million combined operations across
the same 1/2/4/8/16/32-thread sweep, 16-byte diagnostic rows, and approximately
1 KiB YCSB rows. They produced zero first-attempt misses, final read errors, or
insert errors.

The historical gated retry build covered 95 million operations across all four
providers. WorkTablesIndex and IndexSet were additionally tested with
approximately 1 KiB rows at 8, 16, and 32 threads. It produced zero externally
visible first-attempt misses, final read errors, or insert errors.

These operation totals include the 5% inserts in Workload D; they are not
counts of point reads alone.

The earlier combined shadow-state matrix exercised WorkTablesIndex, IndexSet,
Congee, and Arctic with WorkTable retry off and on, 1/2/4/8/16/32
threads, five fresh tables per cell, 1 KiB rows, delete/reinsert churn, and
concurrent vacuum. The final rerun directly against the consolidated PR #187
worktree passed all 240 cells: 12 million writes, 10 million requested reads,
37,489,481 independently validated online reads, 32,991 vacuumed pages, and
zero error counters. All 30 feature-off WorkTablesIndex cells passed. The
specialized default algorithm is now validated separately without the retry
feature. See
`SHADOW_CONCURRENCY_WORKTABLES_INDEX.md` for the root cause, regressions, and
exact result artifact.

The specialized default then passed 90 additional WorkTablesIndex-only cells
across 1/2/4/8/16/32 threads: 4.5 million requested writes, 3.75 million
requested reads, 14,108,737 validated online reads, 12,245 vacuumed pages, and
zero error counters. The second half ran after the final hot-path inlining
change.

## Paired performance check

The longer performance confirmation interleaved feature-off and feature-on
binaries for 15 pairs per B-tree provider, with 2 million operations per run,
8 threads, 10,000 initial rows, and 16-byte fields.

| Provider | Feature-off median | Feature-on median | Median paired delta (bootstrap 95% CI) | Read p50 off/on | Read p99 off/on |
|---|---:|---:|---:|---:|---:|
| WorkTablesIndex | 2.911M ops/s | 2.940M ops/s | +1.18% (-0.39%, +1.62%) | 1.500/1.458 us | 10.208/9.292 us |
| IndexSet | 2.955M ops/s | 2.953M ops/s | +0.38% (-0.23%, +0.78%) | 1.417/1.417 us | 8.917/8.750 us |

Both confidence intervals cross zero, so this local run found no measurable
hot-path regression. Individual paired deltas were noisy, ranging from -0.87%
to +5.48% for WorkTablesIndex and -3.55% to +4.47% for IndexSet. Controlled AWS
ARM runs remain required before making a paper performance claim.

The final provider-level miss confirmation received a separate six-pair frozen
binary screen on 1,000 successful primary-key selects. Mean and median paired
deltas were +0.48% and +0.46%, with individual pairs spanning -2.48% to +3.64%.
This is inside local run noise; true negative lookups deliberately do more
work.

## Publication boundary

Concurrent WorkTable D no longer needs to be categorically excluded. Publish
it only with the exact provider in the configuration:

- ART result: `persist: false`, `using congee` or `using arctic`, and
  `versioned-row-publication`.
- Persistent-capable B-tree result: the stabilized WorkTablesIndex candidate.

Do not present an older WorkTablesIndex build or upstream IndexSet concurrent D
as a valid correctness run. IndexSet may still be used as a quiescent or
single-thread baseline where relevant. Do not infer range-scan behavior from
these point-heavy results: the current Congee and Arctic range adapters
allocate snapshots, and those providers do not support WorkTable persistence.

The raw JSONL files under `results/` remain ignored until a controlled run is
reviewed and selected for curation.
