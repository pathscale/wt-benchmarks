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

The sections above port *external* workloads. These are the workloads of our own
consumers, and they are the ones that decide whether a release is an improvement
in practice. A benchmark belongs here when its shape was taken from a real
consumer's measured profile rather than from a published spec.

**Label every such benchmark with the profile it serves**, in the module's doc
header and in this table. Several agents add to this list, and a benchmark whose
consumer is not written down cannot be prioritised against the rest.

### AgentCode

A semantic workspace for coding agents. Indexes a repository into immutable
content-addressed generations: 11 tables, 8 of them `persist: true`. One
generation of the fact-dense fixture is 800 files, 14,400 symbols and 14,400
dependency edges, and every generation currently rewrites all of them. Its
recorded phase profile puts **74% of an incremental update in three bulk write
phases**. Source: `agentcode-worktable-asks.md`, measured on beta.12.

| Benchmark | Files | What it guards |
|---|---|---|
| Codegraph | `src/codegraph.rs`, `benches/codegraph.rs` | Generational publish (persisted and memory), one-file incremental update, hot-key generation scan, per-call graph adjacency walk. The 22x durable-write ratio that dominates this consumer, and index insert and lookup, which beta.15 cost 13 to 30%. See [CODEGRAPH_PROFILE.md](CODEGRAPH_PROFILE.md). |
| Write profile | `src/agentcode.rs`, `src/bin/agentcode-worktable.rs` | One 14,400-row generation across WTI, Arctic and Congee, with both the primary and dedup indexes on the selected backend. Compares one-at-a-time with `insert_many`, in-memory with persisted, and reports persistence acceptance separately from drain. |
| Key fan-out | `src/fanout.rs`, `benches/fanout.rs` | A non-unique index under a shared generation id, where every row in a generation carries the same value. WorkTablesIndex 0.0.8 turned this into a linear scan inside insert and reached AgentCode as a 21x regression. |
| Pinned text search | External: `agentcode` commit `62a97b7`, `crates/agentcoder-server/tests/rg_comparison.rs`; evidence in `docs/benchmarks/rg-exact-token-comparison.md` there | Consumer-level exact-token lookup against real `rg` on this repository as the irregular fixture. All 45 samples had identical coordinates. Warm median was 2.533 ms versus 7.949 ms for `rg` (3.14x); cold open plus snapshot/index was 871.125 ms, so the measured mixed-query break-even was 165 queries. Its isolated production-width backend grid builds 20,000 posting-shaped rows in 12.718 ms on Arctic versus 17.384 ms on WTI (1.37x), while the 66 versus 63 ns hot lookup medians overlap in range. |
| Text-index build | not written | `ensure_text_index` is 37% of an update, the single largest indexing phase, and the external search benchmark above does not measure its incremental build cost. |
| Incremental reuse | not written | One file changed 18 of 14,400 symbols, 0.125%, and the store wrote all of them. The floor is 13.7x below current. |

**Codegraph and the write profile overlap and should be merged into one
benchmark.** They were written independently and cover different halves: the
first has the profile document and the graph shapes, the second has the backend
grid and the acceptance-versus-durability split. Do not extend both.

Not covered by either, and tracked elsewhere: eviction and vacuum reclaim
(blocked on `pathscale/WorkTable#78`), on-disk footprint
(`campaigns/footprint`), and concurrency, which this consumer does not yet
exercise.

### EKOPathRS

A resident compiler session resolves structural paths shaped like
`fn:unit000042/loop:1`. The measured working set is 163 distinct regions and
is capped at 512, while 8,192 and 131,072 show where cache and structure
crossovers occur. String prefix scans and point gets are separate questions.

| Benchmark | Files | What it guards |
|---|---|---|
| Structural paths | `benches/arctic_paths.rs` | Prefix scans and shuffled point gets on the identical string population across Arctic, `std::BTreeMap`, and WorkTablesIndex. Prefix arms assert equal, non-zero results so an empty scan cannot masquerade as speed. Integer-key controls isolate key handling. |
| Probe order | `benches/probe_order.rs`, `src/rng.rs`, `src/ycsb/generator.rs` | Ordered, seeded-shuffled, fixed-hot, and YCSB Zipf probes across the three backends. This guards against reporting an ordered cache walk or one permanently hot key as general random lookup performance. |
| Concurrent structural paths | `benches/arctic_concurrent.rs` | Pure point-get aggregate throughput at 1/2/4/8 readers, including unlocked and shared-`RwLock` `std::BTreeMap` controls, plus a separately named 95% read/5% in-place-update interference group. Worker creation is outside the timed window and a start gate prevents workers from running before the clock. Congee is absent because its public key API cannot represent variable-width structural strings. |

All groups are bounded. The concurrency suite uses 10 samples, a 300 ms warmup,
and a 1 s measurement window per cell; filter by population when a full grid is
not needed, for example `cargo bench --bench arctic_concurrent --
arctic_concurrent_get/.*/131072`.

**Findings, 5 September 2026.** At 131,072 structural-string keys, two
back-to-back pure-read passes produced the following median aggregate
throughput. Parentheses are scaling over the backend's own one-reader cell.

| Readers | Arctic pass 1 | Arctic pass 2 | std pass 1 | std pass 2 | WTI pass 1 | WTI pass 2 |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 11.145 (1.00x) | 10.119 (1.00x) | 6.206 (1.00x) | 5.754 (1.00x) | 4.088 (1.00x) | 3.727 (1.00x) |
| 2 | 22.745 (2.04x) | 22.223 (2.20x) | 13.158 (2.12x) | 12.725 (2.21x) | 6.536 (1.60x) | 6.072 (1.63x) |
| 4 | 50.667 (4.55x) | 47.814 (4.73x) | 26.550 (4.28x) | 25.538 (4.44x) | 6.194 (1.52x) | 6.007 (1.61x) |
| 8 | 98.609 (8.85x) | 94.924 (9.38x) | 52.646 (8.48x) | 50.986 (8.86x) | 4.576 (1.12x) | 4.430 (1.19x) |

Values are millions of gets per second. Arctic and immutable `std::BTreeMap`
scale through eight readers. WTI peaks at two, remains flat at four, and falls
at eight. The shared-`parking_lot::RwLock<std::BTreeMap>` control produced
3.125 / 4.655 / 6.083 / 9.022 Mgets/s: the shared reader-count cache line is
the first ceiling, while WTI's additional Arc refcount and per-node mutex make
the eight-reader result worse.

The single-thread string result was Arctic 69.8 ns, std 152.7 ns, and WTI
348.8 ns. The fixed-width integer control on the same 131,072-row population
was 12.866 / 18.152 / 20.413 ns. WTI's sequential structure is therefore only
1.59x Arctic and 1.12x std on integers; its string gap comes primarily from
repeating long shared-prefix comparisons across the outer node-maxima search,
the inner up-to-1,024-entry node search, and final equality check.

### MoE-PGO

Profile-guided re-partitioning of mixture-of-experts boundaries. This is
**not** derived from a recorded phase profile: MoE-PGO does not use WorkTable
at run time yet, so `src/moe_pgo.rs` encodes what its pipeline is designed to
ask for. Read the shapes as a specification and the numbers as a baseline to
design against.

Parameters live in memory-mapped safetensors and never move; an expert is a
*view* over that fixed block store, so re-partitioning writes a new map rather
than relocating anything. A map is one `u16` per neuron, about 864 KB at donor
scale. That leaves WorkTable holding exactly two things.

| Benchmark | Files | What it guards | State |
|---|---|---|---|
| Control | `src/moe_pgo.rs`, `benches/moe_pgo.rs` | Nothing. Contains no WorkTable, so its result cannot legitimately move between arms or runs. **Read it first**: if it moved, the machine moved and the rest of the run is void. No other suite here has one, which is how a session once reported a 3.6x spread on a pure dereference and treated the surrounding numbers as real. | Runnable |
| Accumulate | same | Profiling's read-modify-write stream over a dense key set with no locality and no Zipf tail: the working set is the whole table. Nothing else in this suite measures that shape. Retains ~0 bytes per update, measured at the allocator. | Runnable |
| Publish | same | Building a new map version. Insert-dominated and two orders of magnitude larger than retiring one, so it is the cost of in-place retraining. | Runnable |
| Retire | same | Dropping the version readers left behind while they keep arriving. Readers are continuous, so there is never a quiet instant, which is the one place epoch reclamation is load-bearing for this consumer and a quiescence-based scheme would fail. | Runnable |
| Switch window | not written | A reader loads the current version, then looks it up. If a version can be retired between those two steps the reader gets nothing. Observed `missed 0` across every run so far, which is not proof the window is closed, only that it was not hit. Needs deliberate widening to settle. | Missing |
| Resident provenance point lookup | `src/moe_resident.rs`, `src/bin/moe-resident-index-ab.rs` | A measured resident-IR shape: 1,528 unique `(source, ordinal)` keys, five-field owned rows, and eight million deterministic successful lookups per rotated sample. Compares linear `Vec`, `Vec+BTreeMap`, `Vec+Arctic`, and generated WorkTable with Arctic, WTI, and Congee. Local beta18 WorkTable+Arctic is 15.65 ns/query; the same local naked Arctic+Vec control is 9.25 ns/query. | Runnable; equality checked |
| Resident-memory overhead | `src/moe_resident.rs`, `src/bin/moe-resident-memory-ab.rs` | Isolated-process allocation census for the same 1,528 rows and checksum. Local beta18 WorkTable+Arctic retains 105,976 B versus 105,760 B for `Vec+Arctic` (1.002x), and both return to zero after drop. WTI and Congee full-table controls distinguish table-layer overhead from the index backend. | Runnable; beta18 release gate |
| Resident persistence/reload | `src/moe_resident.rs`, `src/bin/moe-resident-persist-ab.rs` | The identical Arctic relation with `persist:true`: measures empty load, visible insert, durability drain, complete artifact bytes, close/reopen, checksum, first query, and warm reopened lookup against the in-memory arm. Published beta.17 produced a 128,055 B artifact, 9.725 ms insert-to-drain, 0.646 ms reopen, and 25.92 ns/query warm after reopen versus 25.93 ns/query in-memory (0.999x). Never mix startup/durability with the steady-state lookup comparison. | Runnable; reload and checksum verified |
| Multiple persisted instances | `campaigns/multi-cluster-instance` | Defines one persisted table schema once, loads two independent instances concurrently through separate engines, proves key isolation, gracefully closes/reloads both, then unloads one while the other remains queryable. Published WorkTable `1.0.0-beta.17` passes. This establishes table-template multiplicity; it does not establish multiple named table spaces inside one DataBucket file. | Runnable; published-crate lifecycle verified |

Every group runs on all three primary-index backends. The key is a dense `u32`,
which is the shape ART indexes exist for, and the gap is not small, so a
single-backend number would mislead rather than merely be incomplete. Adding a
fourth backend is one `moe_backend!` line and one `Backend` variant.

**The axis this suite is really for is WorkTable versions, not backends.**
Backend is a choice made once. A local build slower than the published crate is
a regression, and it cannot appear in a run that only ever builds one of them.
`scripts/compare-worktable-versions.sh <version> [bench]` runs both sides
through a shared `CRITERION_HOME` so Criterion's own baseline machinery does
the comparison.

**Findings, 3 September 2026.** Congee and arctic insert about 25 to 30 percent
faster than WorkTablesIndex, reproducibly: the ordering held across three
passes with the order alternated, across published beta.16 and local beta.17,
and across two dependency sets. Release build, roughly 260 to 380 ns per row
depending on backend; a full 442k map version is about 0.15 s.

An earlier figure of 6.3 us per row in this section was a debug build and was
wrong by twenty times.

Published beta.16 measured no slower than local beta.17, with beta.17's first
pass elevated and its later passes matching. That is **unresolved, not
absent**, and it is exactly the comparison the script above exists to settle.

`moe_pgo2` is the bulk-mutation companion. It builds the same dense map with a
loop of generated `insert` calls and with one `insert_many`, then clears it
with a loop of `delete`, one `delete_many`, or one `delete_range`. Batch widths
are 1, 64, 1,024, and 12,288 rows, and every case runs against WorkTablesIndex,
Congee, and Arctic. Fixture construction and row/key allocation happen outside
the measured interval. WorkTable does not currently expose `update_many`, so
the benchmark does not claim to measure one. Delete timing ends when the
generated API returns; this is an operation-latency benchmark, not proof that
vacuum has subsequently reclaimed every byte.

**Deliberately not measured.** Per-token neuron routing: the partition is drawn
so a request's needs are known before compute starts, and if per-token routing
were needed that would be evidence the partition failed. Weight paging:
WorkTable does not hold parameters. Generational churn at scale: versions are
864 KB and a resident block stays valid across a re-partition, because experts
are views. The neuron-pair co-activation matrix is not a table and must never
become one: `F * (F + 1) / 2` u32 counters, 302 MB per layer at F = 12288, in a
memory-mapped file.

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
