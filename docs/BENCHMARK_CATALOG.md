# Benchmark catalog and port order

This is the executable backlog. “Port” means preserving the workload's
operation semantics and distributions while mapping them honestly to
WorkTable's typed accessors and ordinary compiled Rust. It does not imply SQL
support: the macro specializes storage, indexes, and common access paths;
compiled Rust is the compositional query and procedure language.

Priorities are:

- **P0:** required for the CIDR evaluation and first public benchmark release.
- **P1:** required for credible coverage of a major production use.
- **P2:** valuable breadth after the core claims are settled.
- **Exclude:** misleading, legally encumbered, or outside WorkTable's contract.

## Portable external workload families

| Priority | Port | Exact local workload | Why / caveat |
|---|---|---|---|
| P0 | YCSB A | 50% point read, 50% field update, Zipf keys | Update-heavy serving baseline. Runnable now. |
| P0 | YCSB B | 95% point read, 5% field update, Zipf keys | Read-mostly serving baseline. Runnable now. |
| P0 | YCSB C | 100% point read, Zipf keys | Pure indexed-read ceiling. Runnable now. |
| P0 | YCSB D | 95% latest-biased read, 5% insert | Append plus recent-item reads. Runnable now. |
| P0 | YCSB E | 95% short range scan, 5% insert | Index/range behavior. Runnable now. |
| P0 | YCSB F | 50% read, 50% read-then-update | Read/modify/write behavior. Runnable now. |
| P0 | db_bench core | fillseq, fillrandom, overwrite, readrandom, readseq, readreverse, seekrandom, delete; fixed key/value widths | WorkTable, SQLite `:memory:`, and redb now run five shared phases with matching checksums. Reverse/seek variants and LMDB/RocksDB adapters remain. Report that WorkTable is typed rows, not an LSM KV store. |
| P0 | redb embedded suite | bulk/sequential/random writes, random and multithreaded reads, range reads, removals, memory/storage size | Directly useful Rust embedded comparison; run the upstream adapter as well as a shared driver. |
| P0 | SQLite speedtest1 core | transactional bulk insert, indexed point/range reads, ordered scans, updates, deletes, text and integer keys | Paired WorkTable and SQLite `:memory:` runners now execute nine matching core phases. Bulk-transaction, WAL/durable, and unsupported SQL groups are explicitly not claimed. |
| P1 | BenchBase TATP | subscriber lookup, access-data lookup, call-forwarding lookup/insert/delete, subscriber/location updates | WorkTable's four-table runner and all seven canonical transaction types are runnable now. Expected aborts and the absence of automatic cross-table atomicity are explicit; a transactional-engine adapter remains. |
| P1 | BenchBase SmallBank shape | balance, deposit, checking/savings transfer, account amalgamation | Host-Rust procedure/coordination test. WorkTable does not promise automatic multi-table atomicity, so conservation failures and the locking protocol must be explicit. |
| P1 | BenchBase SEATS subset | customer/flight/reservation lookups, reservation create/update/delete using secondary indexes | Strong host-Rust indexed-join/procedure workload; disclose per-table rather than cross-table transaction semantics. |
| P1 | LinkBench | link add/delete/update/count/get, node get/update, ID and time range queries with social-graph skew | Published operation mix is runnable now over a synthetic Zipf graph. Empirical degree distribution, trace-compatible loading, and concurrency remain. Apache-2.0. |
| P1 | TechEmpower DB tests | single query, 1–20 queries, 1–20 read-then-updates, fortunes-style load/sort/render, cache route | End-to-end SaaS requests/second with HTTP and serialization. Keep a separate engine-only companion. |
| P1 | TSBS subset | deterministic DevOps/IoT ingest; latest point, time-window range, filtered window, host-Rust min/max/mean | Time-series/range use. Full TSBS SQL aggregates are not macro-dialect operations. |
| P1 | CacheBench shape | cache get/set/delete, variable object sizes, hit ratios, hot-set/trace replay, eviction pressure | WorkTable as an application cache; use only traces whose redistribution terms are recorded. |
| P2 | BenchBase Twitter | synthetic user/tweet/follow graph, timeline fan-out, profile/tweet lookup and insert | Host-Rust index nested-loop joins; do not redistribute questionable historical Twitter data. |
| P2 | BenchBase Epinions shape | synthetic users/items/reviews/ratings/trust edges with point lookup, fan-out, and host-Rust joins | Useful many-to-many schema, but do not import or redistribute a dataset until its separate terms are verified. |
| P2 | BenchBase Voter | short hot-key vote updates plus periodic totals | Contention and host-Rust aggregation. |
| P2 | BenchBase AuctionMark | selected auction browse/bid/close procedures | Broad OLTP but large port and cross-table semantic mismatch. |
| P2 | BenchBase Wikipedia | synthetic or license-cleared MediaWiki read/edit/watchlist subset | Real SaaS shape, but complex schema and data licensing/provenance work. |
| P2 | LDBC SNB short reads | synthetic graph short reads and update stream mapped to tables/indexes | Good multi-hop host-Rust joins; full audited graph claims are inappropriate. |
| P2 | Independent event analytics | load, filtered scan, range, group/count/sum/min/max in host Rust on a synthetic event table | Inspired by the application category, not a ClickBench port: ClickBench is CC BY-NC-SA 4.0 and its queries/data should not enter this MIT repository. |
| Exclude | TPC-C/H/DS and CH-benCHmark | None in public results absent written permission and compliant derived-work labeling | TPC policies govern derivative names/code/results and official comparability. Build an independently named workload only if its application question is otherwise missing. |
| Exclude | SIBench as a speed claim | Correctness/contract test only | It tests snapshot isolation; WorkTable range reads are not snapshot reads. |
| Exclude | sysbench code port | External driver or independently specified generic operations only | GPL-2.0 code should not be incorporated into this MIT repository. |
| Exclude | STAC-branded HFT tests | Independently named HFT workloads below | Specifications/results are controlled and comparability would be misleading. |

## Consumer profiles

Shapes taken from a real WorkTable consumer rather than from a published
benchmark. They are not comparative: no other engine is run, because the
question is whether a WorkTable change breaks a dependant, not how WorkTable
ranks. A profile earns its place by naming a regression that reached a consumer
and was not caught here.

| Profile | Consumer | Operations | Guards |
|---|---|---|---|
| codegraph | `pathscale/agentcode` | generational publish (persisted and memory), one-file incremental update, hot-key generation scan, per-call graph adjacency walk | The 22x durable-write ratio that dominates this consumer; the one-hot-key fan-out that `WorkTablesIndex` 0.0.8 turned into a 21x regression; index insert and lookup, which beta.15 cost 13 to 30%. Documented in [CODEGRAPH_PROFILE.md](CODEGRAPH_PROFILE.md). |

Not covered by this profile, and tracked elsewhere: eviction and vacuum reclaim
(blocked on `pathscale/WorkTable#78`), on-disk footprint (`campaigns/footprint`),
and concurrency, which this consumer does not yet exercise.

## WorkTable microbenchmark matrix

These are not redundant with YCSB. They isolate the mechanisms behind the
paper and make regressions diagnosable.

| Area | Required cases |
|---|---|
| Primary key | hit and miss; sequential/random; integer/string/UUID/composite; point and ranges of 1/10/100/1K/all rows |
| Secondary indexes | unique hit/miss/range; non-unique fan-out 1/2/10/100/1K; 0/1/2/4/8 indexes; first/middle/last unique conflict; stale-candidate revalidation |
| Mutation | insert into empty/steady/full-page table; full-row and one-field update; indexed/non-indexed update; fixed-size and same/growing/shrinking variable-size update; in-place RMW; hit/miss delete; upsert hit/miss; delete/reinsert slot reuse |
| Batch | insert/select/update/delete batches of 1/10/100/1K/10K; ordered and random keys |
| Shape | rows 32 B–4 KiB; 2/8/32/128 columns; fixed-only versus unsized; optional-field null rates; cardinality from cache-resident to RAM-scale |
| Concurrency | 1/2/4/8/16/32/core-count workers; disjoint rows, same row, disjoint fields, overlapping fields; uniform/Zipf/latest/hot-set; open-loop offered load sweeps |
| Publication | default versus `versioned-row-publication`; read/update, read/insert, read/delete, read/vacuum; ghost-row and torn-row invariants; feature-off overhead control |
| Vacuum | fragmentation 0/10/25/50/75%; reclaim-only latency; concurrent foreground throughput/p99; progress with continuously arriving readers; bytes reclaimed per CPU-second |
| Persistence | operation-log enqueue, batch drain, checkpoint, reload, migration, local disk and S3 adapter; sync policy and fsync semantics labeled; injected failure and torn shutdown |
| Resources | allocated bytes/op, allocations/op, resident bytes/row, peak RSS, page/index overhead, binary size, clean/incremental compile time, macro expansion versus schema width |
| Specialization | same pages/indexes through a staged ladder: specialized row; runtime schema/fixed offsets; tagged values; runtime catalog dispatch; encode/decode; coarse locking |

The micro baselines are `Vec<T>`, `Vec<RwLock<T>>`, `HashMap`, `BTreeMap`,
`RwLock<HashMap>`, DashMap, and the WorkTablesIndex primitives. External
embedded baselines are SQLite `:memory:`, redb, LMDB/heed, RocksDB, and sled
where it still builds. Every chart gets a capability/semantics table.

## HFT production-derived suite

Use schemas and distributions derived from the local trading application, with
synthetic values or sanitized traces. Report throughput, p50/p99/p99.9/max,
deadline misses, allocations, and CPU per event. Run steady, microburst, and
market-open burst profiles.

1. **Top-of-book update:** one row per exchange/symbol; update bid/ask price and
   size while strategy readers sample all venues. Sweep same-row and disjoint
   symbol contention.
2. **Depth replacement:** update fixed top levels plus variable rest-of-book
   arrays; sweep 1/5/10/50 levels and payload growth/relocation.
3. **Order lifecycle:** insert an order, unique lookup by client order ID,
   partial-fill updates, status changes, terminal read, and retention delete.
4. **Order/fill append stream:** monotonic inserts plus timestamp-window reads
   and periodic retention/vacuum.
5. **Position lifecycle:** insert by event ID, lookup by symbol/event, close
   price/fee update, and open-position scan.
6. **Funding-rate window:** append rates, index by exchange/symbol, latest and
   time-window queries, rolling calculation in compiled Rust.
7. **Signal pipeline:** append signal rows, lookup the corresponding market
   state, compute strategy values in Rust, and append decisions/events.
8. **Pre-trade risk procedure:** indexed account/position/limit reads, compiled
   risk calculation, and conditional order insert. Label its coordination and
   atomicity protocol.
9. **Recovery interference:** foreground book/order traffic while persistence,
   checkpoint, reload validation, and vacuum run.
10. **Feature-gate A/B:** every latency-critical workload with publication,
    unique revalidation, or reclamation gates off/on; regressions above 2% are
    investigated rather than dismissed.

## Desktop production-derived suite

The AgencyZero schemas supply realistic wide strings, append logs, persisted
startup, and non-unique project fan-out.

1. Cold start: open/reload 10/100/1K projects with associated messages, task
   logs, tool I/O, PRs, and usage rows; report startup wall time and peak RSS.
2. Project dashboard: point-read project plus fan-out reads of items/messages/
   tasks/PRs, sort in Rust, and render-ready materialization.
3. Streaming transcript: concurrent message, task-log, and agent-I/O appends
   while the UI pages the active project.
4. Project edits: name/status/pin/activity/position one-field updates with small
   cardinality and human-paced idle gaps.
5. Cascading project delete: delete all non-unique-index children and reclaim;
   report foreground pause and eventual memory recovery.
6. KV settings: string-key get/set/upsert with 10–10K rows and cold/warm reads.
7. Retention: timestamp-cursor trim of agent I/O and task logs during foreground
   appends and reads.
8. Migration/upgrade: schema-version load and migration time, disk footprint,
   and failure recovery across realistic installed-database sizes.
9. Resource floor: empty-table/static binary cost, idle RSS, clean compile time,
   install size, and battery/CPU during an idle-plus-burst script.

## SaaS production-derived suite

The support, identity, payments, and live-video applications provide the
request mix. Measure engine-only operations and full endpoint TPS separately.

1. Support chat: create session; append messages; list by session and sort;
   read app/user/member metadata; close session.
2. Tenant skew: many tenants with Zipf traffic, per-tenant hot sessions, and
   one noisy tenant; verify isolation and p99 for cold tenants.
3. Persistence toggle: move a tenant's messages between memory and persisted
   tables under the application's lock protocol while reads/appends continue.
4. Retention purge: timestamp range select, revalidation, delete, vacuum, and
   foreground p99 impact.
5. Authentication: user/app/token unique lookups, membership fan-out, TOTP/
   session insert-expire-delete, and rate-limited hot identities.
6. Payments: merchant/config/wallet lookups, idempotency-key insert, balance
   update, payment-state transition, and history range query. Add explicit
   conservation/idempotency invariants; do not imply cross-table ACID.
7. Live sessions: studio/session/participant/heartbeat insert-update-expire,
   list by studio, and reconnect bursts.
8. TechEmpower-compatible HTTP routes: single/multi lookup, read-then-update,
   fortune-style fan-out/sort/render, JSON and plaintext controls.
9. Scale and tenancy: 1/10/100/1K tenants; 1/10/100 concurrent requests per
   tenant; small/medium/large rows; database fits/exceeds LLC.
10. Persistence/restart/S3: accepted-write latency, background drain, restart
    recovery, sync lag, failure injection, and cost per million requests.

## Host-Rust joins and procedure coverage

For every multi-table port, implement and label these execution strategies:

- index nested-loop lookup for selective joins;
- hash join for larger unsorted inputs;
- merge join where both inputs have compatible ordered access;
- application procedure with branching, aggregation, and external computation.

Measure macro-native accessors separately from the full compiled-Rust
procedure. This demonstrates the intended seamless language-level composition
without claiming a SQL optimizer, declarative joins, snapshot reads, or
automatic multi-table transactions.

## Implementation order

1. Finish the YCSB runner and add the shared JSONL/result manifest.
2. Move the existing paper micro/contention/ablation/compile-cost programs into
   adapters under this repository without changing their semantics.
3. Add the remaining LMDB/RocksDB adapters and SQLite WAL/durable mode behind
   features; the `Vec`/map/DashMap, redb, and SQLite `:memory:` targets run now.
4. Implement HFT order-book and order-lifecycle workloads from the local
   production schemas.
5. Implement desktop startup/project-view and SaaS support-chat/TechEmpower
   workloads.
6. Finish TATP/LinkBench comparator and concurrency coverage, add
   SEATS/SmallBank-shaped host-Rust procedures, then the deferred analytical
   and graph boundary tests.

## Primary research sources

- YCSB repository, workload files, and Apache-2.0 license:
  https://github.com/brianfrankcooper/YCSB
- BenchBase workload catalog and Apache-2.0 framework:
  https://db.cs.cmu.edu/projects/benchbase/ and
  https://github.com/cmu-db/benchbase
- RocksDB `db_bench` documentation:
  https://github.com/facebook/rocksdb/wiki/Benchmarking-tools
- redb benchmark and source license:
  https://github.com/cberner/redb
- SQLite test strategy, `speedtest1`, and public-domain status:
  https://www.sqlite.org/testing.html and
  https://sqlite.org/cgi/src/file/test/speedtest1.c
- LinkBench workload and Apache-2.0 source:
  https://github.com/facebookarchive/linkbench
- TSBS deterministic load/query model and MIT source:
  https://github.com/timescale/tsbs
- TechEmpower test requirements and BSD-3-Clause source (the repository was
  archived in 2026, but the test definitions remain useful):
  https://github.com/TechEmpower/FrameworkBenchmarks
- CacheBench and Apache-2.0 source:
  https://cachelib.org/ and https://github.com/facebook/CacheLib
- LDBC SNB workload definitions and Apache-2.0 driver:
  https://ldbcouncil.org/benchmarks/snb/ and
  https://github.com/ldbc/ldbc_snb_interactive_v2_driver
- ClickBench scope and CC BY-NC-SA 4.0 license:
  https://github.com/ClickHouse/ClickBench
- TPC fair-use quick reference and policies:
  https://www.tpc.org/TPC_Documents_Current_Versions/pdf/Fair_Use_Quick_Reference_v1.0.0.pdf
  and https://www.tpc.org/TPC_Documents_Current_Versions/pdf/TPC-Policies_v6.19.pdf
