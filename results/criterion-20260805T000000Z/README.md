# Criterion benchmark results — 2026-08-05 (micro scale)

Full clean Criterion 0.8 run across all workloads. See `environment.txt` for machine/commit.

- `results.csv` — consolidated: suite, benchmark, throughput (ops/s), time (ns).
- `raw-kv.txt`, `raw-ycsb.txt`, `raw-ablation.txt` — raw Criterion stdout (with CIs).

## Suites
- **kv** — 5 engines (worktable, sqlite, redb, lmdb, duckdb) × insert/point_read/overwrite/range_scan.
- **ycsb** — two-port (throughput ops/s + read p99 latency ns) over workloads A/B/C/F, 4 threads.
- **ablation** — specialized WorkTable vs naive dynamic baseline (DynTable), per op.

## Scale
Micro (fast, for iteration): KV 10k rows; YCSB 50k records / 200k ops; short Criterion measurement windows.
NOT the load-bearing paper scale — rerun larger on a clean box for final numbers.

## Notes for reading
- DuckDB (columnar) is expectedly slow on single-row point KV — by design, not a defect.
- Ablation: DynTable being faster on raw point ops is intentional (see dynamic.rs); WorkTable is not meant to win the naive baseline.
- LMDB is the strongest KV competitor (wins range_scan).
