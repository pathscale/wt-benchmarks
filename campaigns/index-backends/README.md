# WorkTable index-backend campaign

This standalone crate benchmarks PR #187 through its downstream Git dependency:

```toml
worktable = {
    git = "https://github.com/pathscale/WorkTable",
    branch = "feat/index-backend-using",
}
```

`Cargo.lock` pins the exact WorkTable revision used by a captured run. The
campaign runs each provider in a separate process so allocator and resident-set
measurements do not leak across providers. The tested schema has an
autoincrement sequential `u64` primary key, a sequential unique `u64` secondary
key, and a fixed 32-byte payload.

The phases are initial insert, primary-key point hit, unique-key point hit,
primary-key point miss, steady insert, and steady delete. Latency is sampled at
a fixed interval; throughput counts all operations. Allocation counters wrap
the process global allocator. On macOS, current RSS comes from Mach task info.

All four tables use `persist: false`, which is required by Congee and Arctic.
Range scans are deliberately outside this campaign because those two adapters
currently allocate full snapshots for range/iteration. Persistence and provider
switching are covered by PR #187's integration tests instead.

Run the balanced seven-repetition ARM campaign from the repository root:

```sh
campaigns/index-backends/run-arm.sh
```

The defaults are 250,000 initial rows, 2,000,000 reads per point phase, 100,000
steady inserts/deletes, and one latency sample per 64 operations. Override these
with `CAMPAIGN_ROWS`, `CAMPAIGN_OPERATIONS`, `CAMPAIGN_MUTATIONS`,
`CAMPAIGN_SAMPLE_EVERY`, `CAMPAIGN_REPETITIONS`, or `CAMPAIGN_RESULTS`.
