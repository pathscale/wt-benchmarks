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

To test the separately gated bounded-retry workaround, add
`--features stable-index-read-retry`. This feature implies
`versioned-row-publication`; it is off by default so the normal HFT path is
unchanged. `WT_CAMPAIGN_PAIR` is copied into each result and can identify
interleaved feature-off/feature-on performance pairs.
