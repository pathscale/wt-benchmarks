# Third-party benchmark provenance

This repository reimplements workload semantics rather than copying benchmark
drivers unless a future adapter explicitly says otherwise.

| Workload | Upstream | Upstream license / policy | Local treatment |
|---|---|---|---|
| YCSB A-F | https://github.com/brianfrankcooper/YCSB | Apache-2.0 | Independently implemented operation mixes; attribution retained here and in the catalog. |
| BenchBase TATP | https://github.com/cmu-db/benchbase | Apache-2.0 | Independently implemented four-table schema shape and canonical 2/35/10/35/2/14/2 transaction mix; no upstream driver code copied. Fixed arrays compact repeated subscriber columns, composite SQL keys are packed into typed `u128` primary keys, and host Rust performs the selective join. |
| LinkBench | https://github.com/facebookarchive/linkbench | Apache-2.0 | Independently implemented published request mix with synthetic Zipf graph; no upstream source copied. Empirical degree distribution and concurrency remain pending. |
| SQLite speedtest1 / shared KV | https://sqlite.org/cgi/src/file/test/speedtest1.c | Public domain | Independently implemented nine speedtest1-core shapes and a five-phase shared KV adapter; no upstream source copied. Feature-gated `rusqlite` 0.40.1 embeds SQLite and runs the same deterministic inputs/checksums as WorkTable. Unsupported SQL groups, bulk-transaction semantics, and durable-mode equivalence are not claimed. |
| RocksDB db_bench | https://github.com/facebook/rocksdb/wiki/Benchmarking-tools | Apache-2.0 / GPL-2.0 dual-license repository | Shared WorkTable/redb operation-compatible core exists; RocksDB adapter remains pending. No upstream source copied. |
| redb benchmark | https://github.com/cberner/redb | MIT OR Apache-2.0 | Independently implemented adapter using redb 4.1; no upstream benchmark code copied. |
| TSBS | https://github.com/timescale/tsbs | MIT | Planned deterministic data/query generator adapter; verify every imported asset before use. |
| ClickBench | https://github.com/ClickHouse/ClickBench | CC BY-NC-SA 4.0 | Do not copy its queries or dataset into this MIT repository. Any local event-analytics test must be independently specified and must not be presented as ClickBench. |
| TechEmpower FrameworkBenchmarks | https://github.com/TechEmpower/FrameworkBenchmarks | BSD-3-Clause | Planned end-to-end SaaS routes; no code copied yet. |
| TPC workloads | https://www.tpc.org/ | TPC policies apply | Excluded from published ports unless permissions and reporting requirements are resolved. |

Before importing code, queries, data, or generators, add the exact upstream
commit, asset license, modifications, and required notices to this file.
