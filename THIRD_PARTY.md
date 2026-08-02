# Third-party benchmark provenance

This repository reimplements workload semantics rather than copying benchmark
drivers unless a future adapter explicitly says otherwise.

| Workload | Upstream | Upstream license / policy | Local treatment |
|---|---|---|---|
| YCSB A-F | https://github.com/brianfrankcooper/YCSB | Apache-2.0 | Independently implemented operation mixes; attribution retained here and in the catalog. |
| BenchBase catalog | https://github.com/cmu-db/benchbase | Apache-2.0 | Candidate workload definitions only; each port must record deviations and data provenance. |
| LinkBench | https://github.com/facebookarchive/linkbench | Apache-2.0 | Planned adapter/reimplementation; no code copied yet. |
| SQLite speedtest1 | https://sqlite.org/cgi/src/file/test/speedtest1.c | Public domain | Planned embedded/desktop workload; no code copied yet. |
| RocksDB db_bench | https://github.com/facebook/rocksdb/wiki/Benchmarking-tools | Apache-2.0 / GPL-2.0 dual-license repository | Planned operation-compatible runner; no code copied yet. |
| redb benchmark | https://github.com/cberner/redb | MIT OR Apache-2.0 | Planned external baseline and operation-compatible runner; no code copied yet. |
| TSBS | https://github.com/timescale/tsbs | MIT | Planned deterministic data/query generator adapter; verify every imported asset before use. |
| ClickBench | https://github.com/ClickHouse/ClickBench | CC BY-NC-SA 4.0 | Do not copy its queries or dataset into this MIT repository. Any local event-analytics test must be independently specified and must not be presented as ClickBench. |
| TechEmpower FrameworkBenchmarks | https://github.com/TechEmpower/FrameworkBenchmarks | BSD-3-Clause | Planned end-to-end SaaS routes; no code copied yet. |
| TPC workloads | https://www.tpc.org/ | TPC policies apply | Excluded from published ports unless permissions and reporting requirements are resolved. |

Before importing code, queries, data, or generators, add the exact upstream
commit, asset license, modifications, and required notices to this file.
