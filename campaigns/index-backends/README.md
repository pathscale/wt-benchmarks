> **Status (2026-08-05): research campaign ported to WorkTable 1.0.0-beta.5.**
> It remains outside the v1 release and paper benchmark suite, but now builds against
> the same local WorkTable checkout as the other campaigns.

# WorkTable index-backend campaign

This standalone crate benchmarks the index backends available in the local WorkTable checkout:

```toml
worktable = { path = "../../../WorkTable" }
```

The local WorkTable checkout determines the exact revision used by a run. The
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
