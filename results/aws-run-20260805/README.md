# AWS performance data — 2026-08-05 (size-10 smoke, 4 archs)

**Start here: `ALL-PERF-DATA.csv`** — every measurement, all archs, one file.
Columns: `arch, suite, op, engine, median_ms` (176 rows).

- **arch**: intel (r8i.16xlarge), amd (m8i.16xlarge), arm (r8g.16xlarge),
  budget (t4g.2xlarge). All Ubuntu, WorkTable beta.5.
- **suite**: `kv` (opaque payload), `kv_json` (typed cols vs JSON blob),
  `ycsb` (throughput + latency_p99, workloads A/B/C/F).
- **engine**: `worktable` (WTI), `worktable-congee`, `worktable-arctic`
  (the 3 WT index backends); `redb-json` / `lmdb-json` (KV+JSON, kv_json only);
  `sqlite` / `redb` / `lmdb` (opaque KV, kv suite only).
- **median_ms**: criterion median (lower = better). YCSB throughput values are
  the group timing; latency_p99 rows are the p99.

Scale: 10k rows, `--sample-size 10`, short criterion windows (smoke, not
paper-final precision — the ordering/ratios are stable, absolute numbers will
tighten at higher sample sizes). NOTE: duckdb excluded (built from source, too
slow / disk-bound on these boxes).

## Per-box raw data
`{intel,amd,arm,budget}/`:
- `all.log` — raw criterion stdout (with confidence intervals).
- `crit.tgz` — the criterion estimate JSONs (extract for exact percentiles).

## Headline findings (for the paper)
- **kv_json is the story**: WT/ART crush the durable-JSON pattern —
  insert ~16x faster than redb+JSON, update_field ~3.6x, point_get ~2x.
- **ART (Congee/Arctic) beat WTI** on point_get (~1.6x) and insert everywhere.
- **kv opaque lane**: raw KVs (sqlite/redb/lmdb) beat WT — that's the naive
  bytes baseline, not the story.
- **Known WT issue (see ../HANDOVER-update-vs-insert.md)**: single-column update
  is 3.6-4.7x a fresh insert on ALL archs — flagged for parallel debug.

## CAVEAT on the WT update_field numbers (root-caused; two fixable overheads)
The measured WT `update_field` cost is inflated by two known, self-inflicted
codegen issues — NOT fundamental to WorkTable, and NOT paid by any competitor:

1. **Erroneous full-row reinsert (dominant).** Updating an unindexed fixed-width
   column (e.g. `balance: f64`) on a table that ALSO has a String column is
   routed through full-row clone + delete/reinsert. The safe in-place
   archived-field swap is generated only when the ENTIRE table is fixed-width
   (gated on `self.columns.is_sized` instead of per-updated-column). Fix in
   progress on a branch off PR #54's head. Expected effect: ~4x -> ~1x.
2. **Discarded UUIDv7 per update (secondary).** Every generated update mints
   `OperationId::Single(Uuid::now_v7())` unconditionally, but on a
   `persist: false` table nothing consumes it — pure dead work (clock + RNG per
   op). Competitors generate no UUIDs. Deferred elision.

STATUS: FIXED AND MERGED TO MASTER (PR #54 + #55, = beta.6). The AWS-verified
corrected `update_field` (10k ops, all 4 archs):
  Intel 6.46 ms | AMD 6.41 ms | ARM 9.04 ms | Budget 14.77 ms  (~8x faster)
vs peers: ~28x faster than redb+JSON (~181ms), ~4x faster than lmdb+JSON (~26ms).
update_field now flips from the one loss to a WIN — WorkTable wins EVERY kv_json
op. USE THESE numbers; the pre-fix ALL-PERF-DATA.csv update rows are obsolete.

Implication for the paper: the update regression is gone. C2 is a measured perf
win, not correctness-only.

**FIXED numbers (branch off PR #54 head, 10k ops, decomposed):**
- insert:                 17.99 ms
- update (original):      45.66 ms  (2.54x insert)
- update − unused UUID:   34.20 ms
- explicit delete+insert: 33.97 ms  (confirms delete+insert beat the broken update)
- **update (corrected in-place): 5.71 ms (0.30x insert)** <- USE THIS
Net: 45.66 -> 5.71 ms = ~8x faster; update is now CHEAPER than insert, as an
in-place single-field write should be. Passes on WTI / Congee / Arctic.

The corrected update makes WT dominate the KV+JSON update_field lane outright
(vs redb+JSON ~150-180ms, lmdb+JSON ~25ms). The pre-fix CSV row understates this
massively.
`scripts/aws-run.sh` (in repo root) provisions + runs a box. `FAST=1` for smoke.
