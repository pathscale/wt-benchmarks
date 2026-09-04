# AgentCode local WorkTable version/backend grid

Validated 2026-09-05. The benchmark writes one real 14,400-symbol generation.
Every WorkTable-family crate was resolved from a local checkout. Beta13 and
beta15 used their matching local DataBucket and WorkTablesIndex versions; all
three versions used the local Arctic and Congee checkouts.

The primary key and dedup secondary index both use the backend named in each
arm. Each cell is the median of three warm process runs and is reported in
nanoseconds per row.

## In-memory phases

| version | backend | one at a time | `insert_many` | generation readback |
|---|---|---:|---:|---:|
| beta13 | WTI | 1,212.14 | 1,041.73 | 122.02 |
| beta13 | Arctic | 910.42 | 728.90 | 97.65 |
| beta13 | Congee | 904.04 | 735.36 | 103.01 |
| beta15 | WTI | 1,193.00 | 987.45 | 127.92 |
| beta15 | Arctic | 899.16 | 721.78 | 92.14 |
| beta15 | Congee | 921.64 | 726.67 | 96.08 |
| beta18 | WTI | 835.06 | 728.80 | 110.25 |
| beta18 | Arctic | 506.03 | 379.56 | 80.12 |
| beta18 | Congee | 538.04 | 395.23 | 73.84 |

## Persisted phases

The write columns include acceptance plus `wait_for_ops`, added within each
individual run before taking the median. They therefore measure durable work,
not merely queue admission.

| version | backend | durable one at a time | durable `insert_many` | generation readback |
|---|---|---:|---:|---:|
| beta13 | WTI | 6,598.21 | 7,817.54 | 130.91 |
| beta13 | Arctic | 6,159.13 | 7,243.97 | 103.27 |
| beta13 | Congee | 6,058.45 | 7,202.81 | 110.91 |
| beta15 | WTI | 6,634.12 | 6,475.49 | 129.73 |
| beta15 | Arctic | 6,129.71 | 5,822.36 | 107.42 |
| beta15 | Congee | 6,008.33 | 5,883.92 | 106.45 |
| beta18 | WTI | 4,416.68 | 3,971.17 | 104.06 |
| beta18 | Arctic | 3,753.34 | 3,377.61 | 80.30 |
| beta18 | Congee | 3,899.32 | 3,497.77 | 76.72 |

Against beta15, beta18 durable one-at-a-time improves 33.4% on WTI, 38.8% on
Arctic and 35.1% on Congee. Durable batch improves 38.7%, 42.0% and 40.6%.
Arctic is the fastest beta18 write backend; Congee has the fastest generation
readback.

## Reproduction

The normal candidate run is:

```sh
cargo run --offline --release --bin agentcode-worktable -- --rows 14400
```

Historical runs use `tools/version-grid/Cargo.toml`, `--features
historical-grid`, and explicit local Cargo patch paths for WorkTable,
DataBucket and WorkTablesIndex. The source contains compatibility adapters for
the synchronous pre-beta18 insert APIs; the logical workload is identical.
