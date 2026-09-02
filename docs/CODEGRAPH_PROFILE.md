# The codegraph profile

`cargo bench --bench codegraph`

Every other workload in this repository is a shape WorkTable might meet. This
one is a shape it has already met. It is `pathscale/agentcode`'s storage
profile, reduced to the operations that decide that consumer's latency, and it
exists to answer two questions without needing agentcode in the loop:

1. **Will WorkTable carry this workload**, at the sizes it actually runs at.
2. **Did a change to WorkTable just make it worse**, before the consumer finds
   out by repinning.

Question 2 is not hypothetical. Three WorkTable releases in a row shipped a
regression this consumer felt: `WorkTablesIndex` 0.0.8 scanned inside a
non-unique insert and cost 21x on a workload with one hot key; beta.15 cost 13
to 30% on plain index insert and lookup; beta.16 shipped resolving a dependency
with a use-after-free window. None of them were caught by a benchmark.

## What the workload is

agentcode is a semantic code index. Every time a repository's source changes it
publishes a **generation**: one indexed state of that repository, written across
several tables, every row tagged with the generation's key. Generations are kept
side by side rather than replacing each other, so the tables accumulate.

Publishing a generation of `F` files writes, per file, 18 symbol postings, 18
symbol lexemes, and 18 dependency edges. At the production fixture of 800 files
that is 14,400 symbols and 14,400 edges, or 43,200 rows.

Four properties of that shape drive everything measured here.

**The rows are persisted.** This is the dominant cost and the reason the profile
exists. agentcode measured 10.24 us marginal per persisted row against 0.46 us
for the same insert into a memory table. That factor of 22 is larger than every
other term in an incremental update combined, and no index choice touches it.

**The generation key is hot.** Every row written in one generation shares one
`snapshot_key`, so the non-unique index over it has a fan-out equal to the whole
generation: 14,400 values under one key at the production size. This is exactly
the distribution that made `WorkTablesIndex` 0.0.8 scan on insert.

**Adjacency is walked on every call.** `dependencies.query` looks an edge up by
source and by target. Both keys were 129-character strings until the graph moved
to a `u128` hash on Arctic, verified against the edge the row already
deserializes, because a hash can collide.

**Row width is a fleet-scale term.** Blob digests were 64-character hex strings
on the two highest-row tables and are now `u128` pairs, which measured a 5%
reduction in on-disk table size across eight real repositories. Width is what
decides whether a 5 MB source tree becomes a 60 MB state directory or a 60 GB
one, so the tables here carry the real widths rather than convenient ones.

## What it runs

Four groups. Sizes are 50, 200 and 800 files; **800 is agentcode's measured
production fixture** and the other two exist to show the shape of the curve.

### `codegraph/publish` — persisted against memory

The number to read first. Identical rows, identical indexes, differing only in
`persist`, so the ratio between the two arms isolates the durable write path and
nothing else.

`wait_for_ops` is inside the timed region deliberately. A publish that has not
reached disk has not happened: agentcode waits for that drain before it calls a
generation published, and a benchmark that skips it measures an enqueue.

### `codegraph/incremental` — one file changes

The real case. A populated store is built first, then one file's rows are
written on top, because the cost of adding a row to an empty index is not the
cost of adding one to a full index. agentcode's whole-update cost is about
790 ms against a 58 ms floor of delta writes alone, so what this isolates is the
marginal write, not the source walk that decides what changed.

### `codegraph/generation_scan` — the hot key

Enumerating a generation through the non-unique `snapshot_key` index, where
fan-out is the whole generation. Sweeping generation size is the point: a sound
index reports roughly flat per-row time, and a rising line is an index scanning
the values already under the key.

### `codegraph/dependency_walk` — the per-call adjacency

Incoming and outgoing edges for one node, through the two `u128` Arctic indexes.
This runs on every `dependencies.query`, so its cost is paid per API call rather
than per publish.

## Baseline

WorkTable `1.0.0-beta.17`, `ps-reclaim` 0.1.1, M4 Max, load average 4 to 6.
Derived per row from Criterion's point estimate; a generation of `F` files is
`F * 54` rows.

| files | rows | `publish/persisted` | per row | `publish/memory` | per row | ratio |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 50 | 2,700 | 9.95 ms | 3.69 us | 1.60 ms | 0.594 us | 6.2x |
| 200 | 10,800 | 37.82 ms | 3.50 us | 6.55 ms | 0.606 us | 5.8x |
| 800 | 43,200 | 151.43 ms | 3.51 us | 27.42 ms | 0.635 us | 5.5x |

| files | `incremental` (54 rows) | `generation_scan` | `dependency_walk` |
| ---: | ---: | ---: | ---: |
| 50 | 760 us, interval [477 us, 1.16 ms] | 45.3 us, 19.9 Melem/s | 2.17 us |
| 200 | 394 us | 176.6 us, 20.4 Melem/s | 2.15 us |
| 800 | 408 us | 732.6 us, 19.7 Melem/s | 2.16 us |

Three things in that table are worth stating rather than leaving to be noticed.

**The ratio is 5.5x here, not the 22x agentcode measured.** Both numbers are
real and they are not measuring the same thing. agentcode's 10.24 us per
persisted row is its whole publish path: eight tables rather than three, plus
content-addressed blob files written to disk outside WorkTable entirely. This
bench isolates WorkTable's durable insert and gets 3.51 us per row. The memory
arms agree closely (0.635 us here against 0.46 us there), which is the evidence
that the shape is right and the difference is scope, not error. **Do not quote
5.5x as a refutation of 22x, or as WorkTable's share of it**; establishing that
split needs agentcode instrumented per table, which has not been done.

**`incremental/50` is noise, not a data point.** Its interval spans 2.4x, its
neighbours at 200 and 800 sit within 4% of each other, and a smaller base store
has no mechanism to be slower than a larger one. Left in the table rather than
dropped, because a reader needs to know what a bad cell looks like here.

**Incremental costs more per row than bulk**, 7.4 us against 3.5 us, and that is
expected rather than a defect: a publish has a fixed cost that 54 rows amortise
badly and 43,200 rows amortise well. agentcode measured the same thing
directly, at 9.7 ms fixed per publish against 10.24 us marginal per row. Any
change that lowers the marginal cost without touching the fixed cost will barely
move the one-file update that this consumer actually runs.

## How to read the output

**`publish` is a ratio, not an absolute.** Compare `persisted/800` against
`memory/800`. The baseline above is 5.5x. Materially wider means the durable
path regressed; materially narrower is good news that should be confirmed before
it is believed, because the usual cause is a drain that stopped waiting rather
than a write that got faster.

**`generation_scan` is a slope, not an absolute.** Read across 50, 200, 800 for
one arm and compare the `Melem/s` column, not the time. Flat passes: the
baseline is 19.9, 20.4, 19.7 Melem/s over a 16x range of fan-out. A line that
falls as size rises is the 0.0.8 defect returning, and it is the single
highest-value signal in this file.

**`incremental` and `dependency_walk` are absolutes**, and therefore the two
worth trusting least on a loaded machine. Criterion's own confidence interval is
the guide: an interval wider than a few percent of the estimate means the
machine moved, not the code.

**Check the load average before believing any of it.** Ratios and slopes survive
a noisy box because both arms move together. Absolutes do not. Every historical
number quoted in this document was taken under load and is labelled as
approximate for that reason.

## Budget

Criterion's sample count and measurement time are pinned rather than left to
default. Publishing builds a store from empty every iteration, so samples are
expensive; more importantly, the comparisons here are between arms at the same
size, which does not need tight confidence intervals, and a default budget would
make a persisted regression take minutes to report instead of seconds.

Do not raise the budget to make an interval look better. Shrink the fixture, or
run on a quiet machine.

## What it does not cover

- **Eviction and reclaim.** agentcode's generational eviction is blocked on
  `pathscale/WorkTable#78`, `delete_many` plus a confirmed vacuum reclaim. Until
  that lands there is nothing to measure, and this file deliberately does not
  fake it with per-row deletes.
- **On-disk footprint.** Criterion measures time. The size question, which is
  the other half of what agentcode cares about, belongs with the footprint
  campaign under `campaigns/footprint`.
- **Concurrency.** agentcode publishes from one writer. If that changes, this
  profile needs a concurrent arm and does not have one.
- **The source walk.** Capture, parse and hash are agentcode's problem, not
  WorkTable's, and they are 2 to 11% of an update. Only the storage half is
  here.

## Provenance

Every number quoted above comes from agentcode's own measurements, recorded in
that repository under `docs/benchmarks/`: `state-growth.md` for the size and
width figures, `incremental-update.md` for the 790 ms update and the 58 ms
floor, `index-backends.md` for the 10.24 us against 0.46 us persisted-row cost.
They were measured on a real repository fixture, not on this synthetic one, so
treat them as the shape to match rather than as targets this bench should
reproduce exactly.
