# Binary and persistent-storage footprint research

## Decision

There is no broadly adopted database benchmark whose single metric is “binary
size.” The defensible CIDR experiment is a small, reproducible footprint
campaign modeled on established embedded-database papers:

- Gaffney et al., *SQLite: Past, Present, and Future* (PVLDB 2022) measure
  SQLite and DuckDB compilation time, peak compilation memory, resulting
  library size under `-Os` and `-O3`, and storage occupied by TATP and SSB
  datasets: <https://www.vldb.org/pvldb/vol15/p3535-gaffney.pdf>.
- SQLite's official footprint page reports that library size depends on the
  compiler, target, optimization, and enabled features, and gives `-Os`
  measurements rather than presenting one universal number:
  <https://www.sqlite.org/footprint.html>.
- Seltzer and Bostic's Berkeley DB embedded-systems paper treats code, memory,
  and disk footprint as first-class embedded DBMS criteria and reports a
  per-module library breakdown: <https://www.usenix.org/publications/library/proceedings/es99/full_papers/seltzer/seltzer_html/index.html>.
- redb's maintained benchmark reports both uncompacted and compacted database
  sizes, demonstrating that steady-state and reclaimed footprint must be
  separate rows: <https://github.com/cberner/redb>.

WorkTable differs from SQLite and DuckDB in one crucial way: its specialized
engine code is materialized by each schema macro in the final application.
There is no meaningful standalone WorkTable library size that captures this
cost. Therefore the primary deployment artifact must be a linked, runnable
application, not an `.rlib`, static archive, crate download, or Cargo `target/`
directory.

## Research questions

The campaign answers four narrowly worded questions:

1. What is the absolute stripped size of the smallest runnable application
   exercising a typed table, primary key, one secondary index, insert, primary
   lookup, and indexed lookup?
2. How much does the application grow as the number of distinct declared
   schemas grows from 1 to 2, 4, and 8?
3. How many logical and allocated bytes are required to reload the same rows
   and index after initial load, churn, and backend compaction?
4. Which part of a difference is fixed engine/runtime cost, and which part is
   per-schema or per-row growth?

These support a modest paper claim: WorkTable trades generated code and a
page-aligned persisted representation for removal of a runtime catalog and SQL
engine. They do not establish general superiority over transactional stores.

## Executable experiment

### Common application contract

Each engine's executable must actually execute its path so LTO cannot discard
it. The representative schema is:

| Field | Type | Role |
|---|---|---|
| `id` | 64-bit integer | primary key |
| `account_id` | 64-bit integer | non-unique secondary index |
| `sequence` | 64-bit integer | ordinary fixed field |
| `score` | 64-bit float | ordinary fixed field |
| `payload` | string | variable-size field |

The binary creates the table, inserts one row, performs a primary lookup and
performs a secondary-index lookup. redb uses a multimap table for the secondary
index, so the comparison does not quietly omit the index bytes or API path.

### Matrix

| Dimension | Values |
|---|---|
| Engine | no-DB Rust control, WorkTable, bundled SQLite through rusqlite, redb |
| Schemas | 1, 2, 4, 8 |
| Build | speed (`opt-level=3`) and size (`opt-level=s`) |
| Common flags | fat LTO, one codegen unit, no debug info, stripped symbols, panic abort, incremental off |
| Targets | official Linux ARM64; repeat Linux x86-64; macOS ARM64 is development evidence |

Rust documents that `s` and `z` optimize for size but are not guaranteed to
produce a smaller executable than every other optimization level. Using `s`
and `3` mirrors the PVLDB paper's `-Os`/`-O3` comparison and avoids selecting a
profile after seeing favorable results:
<https://doc.rust-lang.org/cargo/reference/profiles.html>.

### Metrics

Primary metrics:

- raw stripped executable bytes;
- executable file bytes plus non-system dynamic database-library closure;
- text, data, and BSS/load-size sections from `llvm-size`;
- slope of executable bytes over schema count.

Diagnostic metrics, not headline numbers:

- crate/symbol attribution from `cargo-bloat` or Bloaty;
- compressed artifact bytes;
- control-subtracted executable delta.

`cargo-bloat` explicitly describes its attribution as guesswork, so it is
useful for explaining growth but not as the primary measurement:
<https://github.com/RazrFalcon/cargo-bloat>. `llvm-size` provides documented
text/data/BSS section accounting: <https://llvm.org/docs/CommandGuide/llvm-size.html>.

Subtracting the control binary is also not exact attribution: LTO and linker
garbage collection can change the entire program when a dependency is added.
Report the absolute artifacts first and call the subtraction a delta, not
“bytes belonging to WorkTable.”

### Linkage trap

Bundled SQLite is the primary Rust-application comparison because its engine is
inside the shipped artifact. A system-SQLite track is still useful, but the
main executable alone is misleading: it moves the implementation into
`libsqlite3.so`/`.dylib`. A system-linked result must report both:

1. executable bytes; and
2. the complete non-system deployment closure, with a separate clearly labeled
   OS-provided case.

The same rule applies to LMDB/heed and RocksDB. Static and dynamic builds may
not share a bar.

## Persistent-storage experiment

### Common logical dataset

Use the same five-column row and one non-unique `account_id` index. Values are
deterministic. Record counts should include cache-resident development runs and
at least one million rows for the paper. Payload-width sweeps expose whether
fixed page overhead dominates tiny rows and whether payload dominates at
realistic widths.

Recommended paper sweep:

- rows: 10,000 development; 1,000,000 paper;
- payload: 16, 64, 256, and 1,024 bytes;
- cardinality of indexed `account_id`: 10,000;
- dense sequential initial load;
- churn: delete every fourth row and same-length-update every fourth remaining
  row;
- report loaded, churned, compacted/vacuumed, and reloaded states.

The harness reports a logical dataset denominator of the four fixed 8-byte
fields plus payload bytes. This deliberately excludes engine metadata and the
secondary index; their overhead is what the numerator is meant to reveal.
Both `bytes/row` and `stored bytes/logical byte` should be plotted.

### Count every required file

For each phase, recursively sum every file needed for successful reopen:

- WorkTable data, primary-index, secondary-index, table-of-contents, and any
  persistence metadata beneath its configured root;
- SQLite main file plus WAL, shared-memory, or journal files if present;
- redb database file and any sidecar files;
- equivalent files for future LMDB/RocksDB adapters.

Report both logical file length and allocated filesystem blocks. Sparse files,
copy-on-write filesystems, compression, and block size can make them differ.
Record filesystem and mount options in the official environment manifest.

SQLite's `dbstat` interface can additionally report payload and unused bytes by
B-tree, while `page_count`, `freelist_count`, and `page_size` explain file-level
changes: <https://www.sqlite.org/dbstat.html>. Those are explanatory metrics;
the recursive file sum remains the cross-engine measurement.

### Compaction is a phase, not an assumption

Measure at least:

1. `loaded`: after the initial load and a clean persistence drain/commit;
2. `churned`: after deletes and same-length updates, before reclamation;
3. `vacuumed`/`compacted`: after the documented backend operation;
4. `reloaded`: after reopen and row-count/sample validation.
5. `closed-after-reload`: after the validating handle is cleanly closed.

SQLite receives `VACUUM`; redb receives repeated `Database::compact()` until it
reports no further work; WorkTable receives its generated vacuum operation and
then drains persistence. If WorkTable vacuum frees in-memory pages but does not
truncate persisted files, the unchanged file length is the correct result and
identifies a concrete optimization opportunity.

Open and closed states both matter. Some engines reserve, repair, or rewrite
pages while opening a compacted database. The compacted-but-closed artifact and
the post-reopen operational artifact must remain separate rather than selecting
the smaller of the two.

### Capability boundary

The file-size bars are not durability-equivalent:

- SQLite and redb provide crash-consistency/transactional guarantees that the
  current WorkTable persisted path does not.
- WorkTable currently targets asynchronous persistence and warm restart; its
  queue must be drained before measurement and shutdown.
- SQLite stores a runtime schema and SQL-capable record format; WorkTable stores
  generated page/index images; redb stores byte values plus an explicitly
  maintained multimap index.

The paper must call this a representation/footprint comparison and include a
capability note. A smaller WorkTable file would not imply equivalent durability
or transaction semantics.

## Where WorkTable may gain or lose

Likely WorkTable advantages:

- no SQL parser, bytecode VM, planner, runtime catalog machinery, or general
  type system in an in-memory executable;
- dead-code elimination of persistence when the table is memory-only;
- fixed schema permits direct archived layouts and omitted per-value type tags.

Likely WorkTable disadvantages:

- monomorphized generated engine code grows with every distinct schema;
- page-sized data and index allocation creates a high floor for small datasets;
- primary and secondary indexes occupy separate page files;
- variable-size archived strings require offsets/alignment;
- vacuum may reclaim reusable pages without reducing physical file length;
- persistence machinery can materially enlarge a persistence-enabled binary.

File-size-specific improvements worth testing only after baseline evidence:

1. truncate trailing fully free data/index pages during an explicit offline
   compaction;
2. pack partially occupied pages during vacuum before truncation;
3. compact table-of-contents pages and avoid permanently retaining empty index
   pages;
4. store narrow keys/links at the narrowest safe widths;
5. separate transient operation-log capacity from the restart-required image;
6. audit page headers and alignment padding with fixed-only and string schemas;
7. offer a size-oriented page geometry if current page size causes poor packing
   for small embedded datasets.

Do not optimize against one friendly schema. A fixed-only numeric schema may
favor WorkTable, while SQLite's varints can strongly favor small-magnitude
integers and DuckDB-style compression favors repeated column values. Publish a
small schema matrix if space allows; otherwise retain it in the artifact.

## Implementation status

The isolated campaign under `campaigns/footprint/` currently provides:

- executable controls for WorkTable, bundled SQLite, redb, and no-DB Rust;
- 1/2/4/8-schema builds under speed and size profiles;
- raw binary retention, hashes, dependency listings, and section reports;
- persistent WorkTable, SQLite, and redb runners;
- deterministic rows with a capability-matched secondary index;
- loaded/churned/compacted/reloaded JSONL phases;
- recursive logical and allocated file bytes;
- row-count and WorkTable reload validation.

Still required before a CIDR number is publishable:

- inspect the generated artifacts with Bloaty/cargo-bloat to explain rather
  than merely report growth;
- add clean compile time and peak compiler RSS using isolated target directories
  without running another benchmark concurrently;
- add system-SQLite as a separately labeled deployment-closure track;
- run the full matrix on the dedicated Linux ARM64 paper machine;
- repeat at least ten times where compile-time variation is reported;
- pin exact WorkTable, Rust, linker, SQLite, redb, libc, and target commits or
  versions;
- independently verify every output directory contains all reload-required
  files before promoting results.
