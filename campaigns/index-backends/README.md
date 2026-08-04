> **Status (2026-08-04): archived research campaign — does not currently build.**
> It targets the retired `feat/index-backend-using` WorkTable branch (PR #187), which
> no longer exists on the remote. Kept for provenance of the index-backend experiment;
> not part of the v1 release or paper benchmark suite. To revive, repoint the
> `worktable` dependency at a live branch/rev.

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

The phases are initial insert, primary-key and unique-key point hit/miss,
unique-field update, steady insert/delete, and unique ranges of 1, 8, 64,
1,024, and all rows. Latency is sampled at a fixed interval for point and
mutation phases and on every range operation; throughput counts all
operations. Allocation counters wrap the process global allocator. On macOS,
current RSS comes from Mach task info.

All four tables use `persist: false`, which is required by Congee and Arctic.
The range phases deliberately quantify the current Congee/Arctic full-snapshot
cost; they must not be blended into a point-operation score. Persistence and
provider switching are covered by PR #187's integration tests instead.

Run the balanced seven-repetition ARM campaign from the repository root:

```sh
campaigns/index-backends/run-arm.sh
```

The defaults are 250,000 initial rows, 2,000,000 reads per point phase, 200
queries per range width, 100,000 mutations per phase, and one latency sample
per 64 point/mutation operations. Override these with `CAMPAIGN_ROWS`,
`CAMPAIGN_OPERATIONS`, `CAMPAIGN_RANGE_OPERATIONS`, `CAMPAIGN_MUTATIONS`,
`CAMPAIGN_SAMPLE_EVERY`, `CAMPAIGN_REPETITIONS`, or `CAMPAIGN_RESULTS`.
