# MoE resident provenance index A/B

This experiment compares the resident lookup shape used by MoE-PGO with
generated WorkTable tables using Arctic, WorkTablesIndex, and Congee. It uses
1,528 `(source, ordinal)` origin keys and a fixed five-field payload.

## Provenance

These results use the local beta18 candidate working tree, not crates.io. The
candidate is based on WorkTable commit `a046217` plus the beta18 working tree;
its local Cargo package version is `1.0.0-beta.18`.

`cargo tree --workspace --all-features` resolved every WorkTable-family crate
to a local path:

| package | local source |
|---|---|
| WorkTable, codegen, DSL | `/Users/revenge/code/WorkTable` |
| Arctic | `/Users/revenge/code/arctic-wt` |
| ps-reclaim | `/Users/revenge/code/ps-reclaim` |
| WorkTablesIndex | `/Users/revenge/code/WorkTablesIndex` |
| DataBucket | `/Users/revenge/code/DataBucket` |
| Congee | `/Users/revenge/code/congee-wt` |

The stripped Vec controls live in the local `worktable-vec` crate in this
repository. Its Arctic dependency resolves to the same local Arctic checkout.

## Random successful point lookup

Each sample performs eight million deterministic, uniformly distributed,
successful lookups. Nine samples are collected in rotated arm order. Every arm
must produce the same checksum or the benchmark stops.

The audited post-fix release run on 2026-09-05:

| arm | median ns/query | p25–p75 ns/query |
|---|---:|---:|
| Vec linear scan | 205.44 | 204.14–207.17 |
| Vec + BTreeMap | 33.53 | 33.23–33.92 |
| Vec + Arctic | 9.25 | 9.10–9.32 |
| WorkTable + Arctic | 15.65 | 15.41–15.80 |
| WorkTable + WTI | 58.05 | 56.96–58.26 |
| WorkTable + Congee | 24.77 | 24.43–24.82 |

Arctic now defaults to ps-reclaim, so both Arctic arms use the release
reclamation backend. WorkTable+Arctic is 53.3% faster than Vec+BTreeMap and
costs 1.69x the stripped Vec+Arctic control.

ps-reclaim originally treated a second live domain pin as an out-of-line cold
path that scanned atomic participant slots. WorkTable holds its page-domain pin
while entering Arctic's index domain, making that path hot on every select.
The beta18 fix uses an exact per-thread four-bit occupancy mask, preserving
out-of-order-drop and overflow safety while avoiding the scan. The resulting
15.65 ns restores the earlier 15.57 ns WorkTable+Arctic baseline. The
single-thread Seize comparison is intentionally not a release arm;
application-level and scaling results decide the backend.

This benchmark currently covers successful point lookups only. Misses and
range scans require separate validation.

## Construction

Construction is measured separately from lookup. The old lookup runner printed
one cold-ish observation per arm; those values were too noisy to support a
regression claim and have been removed from that runner.

The focused construction runner uses prebuilt logical input, rotated arm order,
31 samples, and eight complete 1,528-row populations per in-process sample:

| arm | median ms/population | p25–p75 ms |
|---|---:|---:|
| Vec + BTreeMap | 0.046 | 0.046–0.047 |
| WorkTable, `block_on` per row | 0.160 | 0.157–0.167 |
| WorkTable, one executor around row loop | 0.160 | 0.157–0.167 |
| WorkTable `insert_many` | 0.168 | 0.164–0.176 |

The same executable also launches a new process for each first-population
sample, avoiding allocator and SMR reuse across samples:

| arm | median ms/population | p25–p75 ms |
|---|---:|---:|
| Vec + BTreeMap | 0.135 | 0.134–0.139 |
| WorkTable, `block_on` per row | 0.209 | 0.207–0.213 |
| WorkTable, one executor around row loop | 0.207 | 0.205–0.214 |
| WorkTable `insert_many` | 0.229 | 0.225–0.245 |

The cold first-population gap is therefore 1.53–1.55x for a WorkTable row loop
versus the stripped Vec+BTreeMap control. The async API wrapper is not the
cause: one executor and per-row `block_on` are within 1% here.

Equivalent local historical in-process runs were:

| version | WorkTable row-loop median ms | WorkTable `insert_many` median ms |
|---|---:|---:|
| beta13 | 0.698 | 0.645 |
| beta15 | 0.662 | 0.618 |
| beta18 candidate | 0.160 | 0.168 |

The candidate does not reproduce a release-to-release construction regression;
it is about 3.9x faster than beta15 on the row-loop construction shape. It
remains slower than the stripped BTree control, which does not serialize rows,
provide table mutation coordination, or provide batch atomicity.

## Memory

The isolated-process counting allocator reproduced the paired 1,528-row result:

| arm | retained bytes | bytes after drop |
|---|---:|---:|
| Vec + Arctic | 105,760 | 0 |
| WorkTable + Arctic | 105,976 | 0 |

The WorkTable delta is 216 bytes, 0.14 bytes per live row, or 1.002x the
stripped Vec+Arctic control. Both arms produced the same checksum.

## Reproduction

```sh
env CARGO_BUILD_JOBS=2 RAYON_NUM_THREADS=2 \
  cargo run --release --bin moe-resident-index-ab

env CARGO_BUILD_JOBS=2 RAYON_NUM_THREADS=2 \
  cargo run --release --bin moe-resident-build-ab

env CARGO_BUILD_JOBS=2 RAYON_NUM_THREADS=2 \
  cargo run --release --bin moe-resident-memory-ab -- worktable-arctic 1528 paired

target/release/moe-resident-memory-ab vec-arctic 1528 paired
```
