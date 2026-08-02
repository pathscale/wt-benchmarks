# Paper and website result tracks

The measurement infrastructure is shared, but the publications have different
jobs. The six-page paper must make a small number of reviewer-relevant claims
unusually convincingly. The website can be a broad, continuously updated
performance reference.

## CIDR paper: dense evidence only

Budget roughly 1.25–1.5 pages for evaluation. Use at most one compact semantics
table and one dense multi-panel figure.

### Compact semantics table

Five rows at most:

1. `Vec<T>`: specialization/lower-bound control;
2. DashMap or `RwLock<HashMap>`: familiar concurrent in-memory control;
3. SQLite `:memory:`: familiar embedded table control;
4. redb or RocksDB: representative embedded persistent control;
5. WorkTable default and stronger publication mode shown together where space
   permits.

Columns should be capability facts, not prose: typed schema, secondary indexes,
concurrent reads/writes, ordered range, cross-table transactions, durability,
and process model. This prevents a fast in-memory number from implying semantic
equivalence to a durable engine.

### One multi-panel figure

- **A — Thesis isolation:** the staged specialization ladder on identical
  pages/indexes: specialized, runtime-schema/fixed-offset, tagged value,
  catalog dispatch, encode/decode, and coarse lock.
- **B — End-to-end utility:** three representative mixes only—read-mostly,
  update-heavy, and RMW—or one production-derived HFT operation if it is more
  compelling. Show throughput and tail latency compactly.
- **C — Concurrency/guarantee cost:** disjoint versus overlapping updates and
  the default versus versioned-publication feature. This answers whether the
  concurrency design scales and what stronger reads cost.

Put compile-time cost, binary size, memory/row, and any remaining workload
numbers into one or two dense sentences. Full YCSB A–F, all thread counts,
machines, and external engines belong in the artifact and website.

### Paper selection rule

A number enters the paper only if it directly supports one of these claims:

1. ahead-of-time specialization materially removes runtime data-management
   overhead;
2. generated field-level coordination preserves useful concurrency;
3. WorkTable occupies a practically valuable point between raw Rust
   collections and general embedded databases.

Everything else is evidence for the website, future work, or rebuttal—not the
six-page body.

## worktable.dev: broad living benchmark

The website should publish:

- all micro operations, schemas, row widths, index counts, and cardinalities;
- YCSB A–F across distributions and thread counts;
- Vec/map/DashMap, SQLite, redb, LMDB/heed, RocksDB, and selected maintained
  Rust embedded engines;
- HFT, desktop, SaaS, persistence, vacuum, recovery, and compile-cost suites;
- default and feature-gated WorkTable modes;
- x86-64 and ARM64 hardware profiles;
- raw reviewed JSONL, environment manifests, exact commits, methodology, and
  confidence intervals.

The website may emphasize navigation and practical engine selection. It must
still keep capability-matched and familiar-alternative comparisons separate.

## Result promotion

Raw local runs remain ignored. Promote a result only after:

1. correctness invariants pass;
2. all contenders use the declared semantic/durability mode;
3. the full repetition set and environment manifest are present;
4. confidence intervals and run-order bias have been checked;
5. the target commits are immutable;
6. the result is labeled `paper-candidate`, `website`, or both.

