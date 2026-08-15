# Handover: universal QA of WorkTable's 3 index backends

## Goal
Systematically QA **all three** WorkTable primary/secondary index backends —
**WorkTablesIndex** (default), **Congee**, **Arctic** — for correctness parity,
not by spot-checking. Every behavior that WorkTablesIndex guarantees must be
proven to hold identically on Congee and Arctic (in-memory *and* persisted where
the backend supports it). Do not assume parity; make the test suite force it.

## The `using` keyword (how backends are selected)
In a `worktable!` declaration:
```rust
worktable!(
    name: Foo,
    persist: false,                 // congee/arctic REQUIRE explicit persist: true|false
    columns: {
        id: u64 primary_key using arctic,   // or: congee | worktables_index | indexset
        val: String,
    },
    indexes: {
        v_idx: val unique using congee,     // secondary indexes can use a different backend
    },
);
```
- Valid backend idents: `worktables_index` (default), `indexset` (upstream),
  `congee`, `arctic`.
- **congee and arctic require an explicit `persist: true|false`** on the table,
  or codegen errors: `primary index 'id' uses 'arctic', which requires an
  explicit persist: true or persist: false`.
- Reference for full syntax + persisted variants:
  `WorkTable/tests/worktable/index_backends.rs` (already a strong multi-backend
  suite: CRUD, ranges, unique-conflict rollback, persist/reload, concurrent
  same-row updates across wti/indexset/congee/arctic).

## What's already done (do not redo)
1. **`WorkTable/tests/worktable/update_in_place_unsized.rs`** — refactored to a
   `unsized_in_place_suite!($module, $using)` macro invoked for
   `worktables_index`, `congee`, `arctic`. 3 tests × 3 backends = 9, all pass.
   Confirms the overwrite in-place fix (PR #48) is NOT WTI-specific.
   - Pattern to copy: one `mod` per backend (avoids `Row`/`Query`/`WorkTable`
     ident collisions), each with its own `worktable!(... using $using)` and
     `persist: false`. `link_of()` uses the `TableIndex::get_value` trait method,
     which every backend's pk_map implements, so link-identity checks are
     backend-agnostic.
2. **`index_backends.rs`** — pre-existing, covers cross-backend CRUD / ranges /
   unique-conflict / persist-reload / concurrent updates. Baseline to build on.
3. **wt-benchmarks** now runs the KV workload on all 3 backends:
   - Bench: `cargo bench --bench kv -- worktable` → labels `worktable`,
     `worktable-congee`, `worktable-arctic` in every group.
   - Binary: `kv-worktable --index-backend worktables_index|congee|arctic`.

## Smoke comparison already captured (10k rows, 64B, single-thread, smoke timings)
| op | WTI | Congee | Arctic |
|---|---|---|---|
| insert | 4.90ms | 4.07ms | 3.98ms |
| point_read | 1.51ms | 0.95ms | 0.94ms |
| overwrite | 19.73ms | 18.46ms | 18.20ms |
| range_scan | 1.48ms | 1.53ms | 1.60ms |
Directional only — Congee/Arctic ~1.6x faster on point_read; WTI edges scans;
overwrite within ~8% (index-agnostic path). Re-run full for paper-grade numbers.

## The actual QA work (build this out)
Parametrize the index-touching test suites over all 3 backends, using the same
per-module macro pattern. Priority order:

1. **`unsized_.rs`** — string full-row + field updates WITH secondary indexes
   (`test unique`, `exchange`, `another`). NOTE: it reads links via
   `pk_map.get(&pk).unwrap().get().value` (WTI-specific node access) — rewrite
   link probes to the backend-agnostic `get_value` trait method, or drop the
   link-identity assert on non-WTI variants and keep the value/round-trip asserts.
2. **`in_place.rs`** — `test_update_in_place_and_update_unsized_multithread` and
   the sized multithread variants. Concurrency + in-place across backends.
3. **`upsert.rs`, `base.rs`, `float.rs`** — the other files that touch
   `primary_index`/`pk_map` (grep: `grep -rln 'primary_index\|pk_map'
   tests/worktable/`). Same link-probe caveat applies.
4. **`delete.rs`, `count.rs`, `vacuum.rs`, `vacuum_no_row_loss.rs`** — deletion,
   vacuum/grace-period, and row-loss guarantees per backend. Vacuum reuses links
   after a grace period; verify no backend reuses early (see PR #48 P1 fix).
5. **Persisted variants** — for congee/arctic with `persist: true`, exercise
   WAL reload + further mutation (copy the shape from
   `index_backends.rs::native_art_backends_survive_wal_reload_and_further_mutation`).

**Skip / low-value:** pure encoding tests where the index backend is irrelevant
(`uuid.rs`, `with_enum.rs`, `array.rs`, `option.rs`, `nid.rs`) — parametrizing
them is noise. Judge by whether the test's assertion depends on index behavior.

## How to run
```
cd ~/code/WorkTable
cargo test --test mod <name_filter>          # e.g. update_in_place_unsized
cargo test --test mod                         # full suite (currently 365 pass, 2 ignored)
```
Persisted tests write under `tests/data/...` and clean up via
`remove_dir_if_exists`; if a run is killed, stale dirs there are safe to delete.

## Repo / branch state (IMPORTANT)
- WorkTable repo: `~/code/WorkTable` (NOT `~/AgencyZero`).
- The in-place backend test landed on **PR #48 branch
  `fix/overwrite-inplace-v2`** (tip `e14cf65`, on top of `d48e1ab`).
  **This is a SHARED branch — others push to it.** Always `git fetch` +
  fast-forward, never force-push, keep commits focused.
- wt-benchmarks: `~/code/wt-benchmarks`, direct-to-`main` (no PR). The
  3-backend KV wiring is commit `3f57d27`.
- Constraints: no `crates.io` publish without explicit owner OK; do not hook
  benchmarks into WorkTable internals; leave `Co-Authored-By: Claude` OFF commits.

## Definition of done
Every index-behavior test in `tests/worktable/` that can vary by backend runs on
all 3 (in-memory; + persisted for congee/arctic), all green, with the link-probe
divergence handled cleanly (not by silently dropping asserts). Then a full,
paper-grade `cargo bench --bench kv` re-run for the numbers table above.
