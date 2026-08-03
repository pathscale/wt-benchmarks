# Workload port status

This file separates executable code from plans. “Runnable” means the binary
builds, executes a deterministic workload, and emits machine-readable JSONL;
it does not make an official comparability claim.

| Workload | Executable | Current fidelity | Still required |
|---|---|---|---|
| YCSB A-F | `ycsb-worktable` | Runnable operation mixes, ten-field approximately 1 KiB rows, Zipf/latest/scan distributions, pre-generated operations | External-engine adapters and official campaign results |
| Shared embedded KV core | `kv-worktable`, `kv-redb` | Runnable sequential insert, random point read, overwrite, range scan, and random delete with common configuration/result schema | WorkTable/redb semantic alignment audit; SQLite/LMDB/RocksDB adapters; reverse and seek variants |
| SQLite speedtest1 core shape | `speedtest1-worktable`, `speedtest1-sqlite` | Paired WorkTable and SQLite `:memory:` runners execute the same nine integer/text insert, point/range/secondary read, ordered-scan, update, and delete phases with matching deterministic checksums | Transaction-equivalent bulk phase, SQLite WAL/durable mode, and any additional query groups that WorkTable can represent honestly |
| LinkBench | `linkbench-worktable` | Runnable published Facebook request percentages, link/node operations, fan-out index, time filter, synthetic Zipf hot nodes | Empirical graph-degree loader, multithreaded request driver, history/time distribution audit, and external backend adapter |
| TATP | `tatp-worktable` | Runnable four-table load and all seven BenchBase procedures at the canonical 2/35/10/35/2/14/2 mix; deterministic streams, expected-abort accounting, host-Rust join, and gated concurrent mode | SQL/transactional engine adapter, automatic cross-table atomicity (not a WorkTable claim), and official campaign results |
| Production HFT/desktop/SaaS | — | Schemas and workload list drafted | Sanitized generators and executable runners |

All runnable ports are independent Rust implementations. See
[`THIRD_PARTY.md`](../THIRD_PARTY.md) for source and license provenance and
[`METHODOLOGY.md`](METHODOLOGY.md) for publication requirements.
