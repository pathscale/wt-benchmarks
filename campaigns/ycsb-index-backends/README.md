# Concurrent YCSB D index-backend diagnostic

This standalone crate combines the acknowledged-key YCSB D generator with a
local integration of WorkTable's `using` backends and
`versioned-row-publication`. It is intentionally a diagnostic campaign until
the two WorkTable feature branches share a merged upstream base.

Select one primary-index provider with `WT_INDEX_BACKEND`:

```sh
WT_INDEX_BACKEND=worktables_index cargo run --release --manifest-path \
  campaigns/ycsb-index-backends/Cargo.toml -- \
  --workload D --records 10000 --operations 100000 --threads 8 \
  --repetitions 10 --sample-every 256 --field-bytes 16
```

Valid providers are `worktables_index`, `indexset`, `congee`, and `arctic`.
Every table uses `persist: false`. Results distinguish first-attempt point
misses, misses recovered by one immediate retry, final read errors, and insert
errors. A backend resolves the anomaly only when `first_read_misses` is zero;
zero final errors with nonzero recovered misses is merely a retry workaround.

WorkTablesIndex now uses a specialized stable-read algorithm by default: the
successful hit remains on the original one-node path and only an apparent miss
enters three-node structural confirmation. The old bounded-retry feature is no
longer part of the campaign. Vanilla IndexSet remains available as a
single-thread/quiescent baseline, but it is experimental and does not gate the
concurrent matrix or published claims.

## Shadow-state concurrency

The companion `shadow-concurrency` binary runs mixed full-row updates,
delete/reinsert cycles, primary and secondary reads, and vacuum against an
independent per-key shadow state. Its seqlock-style markers distinguish reads
that overlap a write without serializing operations through the oracle. Every
returned row is checked for cross-field and payload consistency; after the
workers quiesce, primary, current and stale unique, non-unique, and cardinality
state must exactly match the model.

```sh
WT_INDEX_BACKEND=congee cargo run --release --manifest-path \
  campaigns/ycsb-index-backends/Cargo.toml \
  --bin shadow-concurrency -- \
  --records 2000 --operations 20000 --threads 8 \
  --repetitions 3 --payload-bytes 1024 --vacuum true
```

Run the complete stable-read correctness matrix with a matching environment
capture:

```sh
campaigns/ycsb-index-backends/run-shadow-matrix.sh
```

The defaults cover WorkTablesIndex, Congee, and Arctic at 1/2/4/8/16/32
threads, five fresh tables per cell, 1 KiB payloads, delete/reinsert churn, and
concurrent vacuum. Set `CAMPAIGN_BACKENDS` explicitly to include experimental
IndexSet for diagnostic comparisons.
Override any dimension with the `CAMPAIGN_*` variables defined at the top of
the script. The runner records every JSON line even when a cell fails, runs the
remaining cells, and returns a failing exit status at the end.
