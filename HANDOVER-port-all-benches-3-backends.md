# Handover: port EVERY WorkTable bench/adapter to all 3 index backends

## Goal
Make every WorkTable workload in wt-benchmarks runnable across all three
primary-index backends selectable via the `using` keyword — **WorkTablesIndex**
(default), **Congee**, **Arctic** — so the paper can compare backends on each
workload, not just KV.

## The pattern (already applied — copy it)
Two files are already ported and are the reference implementation:
- `src/kv_table.rs` — `kv_backend_table!($module, $driver, $using)` macro emits
  one `mod` per backend (avoids `Row`/`Query`/`WorkTable` ident collisions),
  invoked for `worktables_index`, `congee`, `arctic`.
- `src/kv_json.rs` — same shape via `wt_doc_backend!`, drivers `WtDoc` /
  `WtDocCongee` / `WtDocArctic`.
- `benches/kv.rs` and `benches/kv_json.rs` — a `*_engine!` macro emits one
  Criterion function per backend with labels `worktable` / `worktable-congee` /
  `worktable-arctic`, called once per backend in each `bench_*` group.

Rules that make it compile:
- **congee/arctic REQUIRE an explicit `persist: true|false`** on the table, or
  codegen errors. Use `persist: false` for the in-memory benches.
- Put each backend's `worktable!` in its **own module** — the generated idents
  (`FooRow`, `FooQuery`, `FooWorkTable`, `FooPrimaryKey`) are NOT
  table-name-prefixed and collide otherwise.
- Syntax: `id: u64 primary_key using $using,` where `$using` ∈
  `worktables_index | congee | arctic` (also `indexset` exists but isn't part of
  the 3-backend paper story).

## Already ported — do NOT redo
- `src/kv_table.rs`, `src/kv_json.rs` (+ their benches)
- `campaigns/index-backends/src/main.rs`
- `campaigns/ycsb-index-backends/src/main.rs`,
  `campaigns/ycsb-index-backends/src/bin/shadow-concurrency.rs`
  (these campaigns are backend-parametric by design)

## TODO — single-backend today, port these (priority order)
1. **`src/ycsb/worktable_adapter.rs`** — HIGH. The YCSB A/B/C/F workloads; the
   headline throughput+p99 story. Port so `benches/ycsb.rs` runs each workload on
   all 3 backends (labels worktable / worktable-congee / worktable-arctic).
2. **`src/bin/tatp-worktable.rs`** — MED. TATP telecom workload.
3. **`src/bin/linkbench-worktable.rs`** — MED. LinkBench graph workload.
4. **`src/bin/speedtest1-worktable.rs`** — MED. SQLite speedtest1 port.
5. **`src/bin/micro-layers.rs`** — MED. Layer microbench.
6. **`campaigns/contention/src/lib.rs` + `campaigns/contention/benches/ablation.rs`**
   — MED. Contention/ablation; note ablation compares WT vs the DynTable
   strawman, so add backend variants to the WT side only.
7. **`campaigns/footprint/src/bin/{storage,footprint}-worktable.rs`** — LOW.
   Per-backend storage/memory footprint (interesting but niche).

## Do NOT port (backend-orthogonal)
- `campaigns/persistence-compat/{reader,legacy-writer,modern-writer}.rs` — these
  test on-disk FORMAT compatibility across WT versions; index backend is
  irrelevant to what they measure.
- **Python datatable** (`python/kv_json_datatable.py`) — an external columnar
  engine, NOT a WorkTable backend. There is no `using` keyword to vary; it's one
  adapter (insert + query_field) that participates in the kv_json comparison
  as-is. Nothing to port. (Install: `uv venv --python 3.10 && uv pip install
  datatable` — does not build on 3.11+.)

## Known correctness caveat (affects what you'll SEE, not the port)
The ART backends (Congee/Arctic) have less QA than WTI. A real scan-consistency
bug was already found: under concurrent unsized upserts, `select_all()` on
Congee/Arctic occasionally returned 995–996 of 1000 rows vs WTI's 1000. If a
ported bench's checksum/count assertion fails on congee/arctic but passes on WTI,
that's likely a real backend bug — report it, do NOT weaken the assertion.

## Verify each port
```
cargo build --bench <name> --features external-adapters     # or the bin
cargo bench --bench <name> -- "<op>" --warm-up-time 1 --measurement-time 2
# confirm 3 labels appear: worktable / worktable-congee / worktable-arctic
```

## Repo facts
- wt-benchmarks: `~/code/wt-benchmarks`, direct-to-`main` (no PR). Current tip
  `8554ea8`. WorkTable dep is `path = "../WorkTable"` (beta.5+).
- Constraints: no crates.io publish; leave `Co-Authored-By: Claude` off commits;
  don't hook benches into WorkTable internals.

## Definition of done
Every workload in the TODO list runs on all 3 backends with distinct labels,
builds clean, all-green (or a filed bug where an ART backend genuinely diverges).
