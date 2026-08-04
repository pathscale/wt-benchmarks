# WorkTablesIndex shadow-concurrency validation

Date: 2026-08-03

This note records development-machine correctness and performance evidence for
the WorkTable shadow-state concurrency harness. These runs are diagnostic and
are not final paper benchmark numbers.

## Oracle and workload

`campaigns/ycsb-index-backends/src/bin/shadow-concurrency.rs` keeps an
independent per-row generation marker outside WorkTable. Writers own disjoint
primary keys, so the oracle does not serialize WorkTable operations. Readers
validate only observations bracketed by an unchanged, non-writing generation.

The harness checks:

- primary-key and current unique-key reads;
- absence through the previous unique key;
- non-unique predicate membership and duplicate entries;
- cross-field generation, payload, and checksum consistency;
- final cardinality and exact primary, unique, stale-unique, and non-unique
  membership;
- optional delete/reinsert and concurrent vacuum behavior.

## Failure reproduced in the published 0.0.1 source

At 32 threads, 1,000 rows, 10,000 writer operations, 10,000 requested reader
operations, 16-byte payloads, updates only, and no vacuum, 3 of 20 fresh-table
runs failed. The failures were stale duplicate entries in the non-unique
index; primary rows and unique indexes were exact in that run.

A focused WorkTablesIndex multimap test made the defect much easier to
observe. Concurrent workers updating disjoint values could ask to remove
`(bucket, row_id)` and receive a different row from the same bucket, or receive
`None`. In WorkTable an ensuing insert can then leave the old non-unique entry
published.

The multipair-removal refactor included in WorkTablesIndex PR #3 is the first
tested good history boundary for the dominant wrong-value-removal failure:
the commit immediately before it failed 8 of 50 shadow runs, while the
refactor passed 100 of 100 under the same update-only configuration.

## Rare residual miss and WorkTablesIndex PR #4

The PR #3 head passed the broad WorkTable shadow matrix, but the focused test
found a rarer case under heavier scheduling: the range lookup located the
exact pair and the subsequent point removal returned `None`.

WorkTablesIndex PR #4 preserves the existing point-removal fast path. Only
after an observed pair unexpectedly misses does it recover under the tree's
structural write guard, making predicate lookup, deletion, CDC generation,
and node reindexing one critical section.

Validation of the final fallback design:

- 20 of 20 focused runs passed at 32 writer threads and 100,000 operations per
  run: 2 million exact remove/reinsert operations total;
- 100 of 100 WorkTable shadow runs passed at 32 threads, about 1 KiB per row,
  and delete/reinsert every tenth write;
- 50 of those WorkTable runs overlapped vacuum and processed 5,012 pages;
- all primary, unique, stale-key, predicate, torn-row, cardinality, duplicate,
  and vacuum error counters were zero.

Repository validation for PR #4 passed 83 unit/stress tests, the all-target and
all-feature build, and Clippy with warnings denied.

## Empty-tree publication and point-read stabilization

A later disjoint-writer test exposed a separate first-publication race. The
empty-tree path tested `try_write().is_ok()` but discarded the acquired guard
before publishing the first node and did not recheck that the index was still
empty. Concurrent first writers could therefore publish overlapping root
nodes. The visible signature was especially dangerous: `len()` still reported
the expected cardinality while both point lookup and iteration were missing a
committed key.

The regression uses eight synchronized first writers with disjoint 1,000-key
ranges. It failed on the second repeated run against the old path. Retaining
the structural write guard through an emptiness recheck and first-node
publication passed 100 native repetitions and 100 downstream WorkTable
repetitions.

Point reads now preserve and inline the original optimistic hit path. Only an
apparent miss clones the selected node and adjacent structural boundaries,
checks them without holding the structural lock, and then verifies that the
three-node routing window did not move. This closes the window where a split
or stale maximum can route a committed value outside the optimistic node while
preserving the documented no-overlapping-lock rule. Whole nodes detached by
`remove_range` are cleared before publication is removed so an already-cloned
node cannot return a successful ghost read.

## Combined WorkTable matrix

The combined PR #187 plus versioned-publication candidate was also exercised
across WorkTablesIndex, IndexSet, Congee, and Arctic; retry off and on; 1, 2, 4,
8, 16, and 32 threads; and five fresh tables per cell. Each table began with
2,000 rows and used 1 KiB payloads, delete/reinsert every seventh write, and
concurrent vacuum. All 240 intended cells completed.

After consolidating the implementation into the actual PR #187 worktree, the
matrix was rerun from scratch in
`results/shadow-concurrency-consolidated-20260803.jsonl`. All 240 cells passed.
The matrix completed 12 million requested writer operations, 10 million
requested reader operations, 37,489,481 validated online reads, and 32,991
vacuumed pages. Every online and final primary, unique, stale-unique, non-unique,
torn-row, cardinality, duplicate, writer, and vacuum error counter was zero.

All 120 cells with the WorkTable-level retry feature disabled passed, including
all 30 WorkTablesIndex cells. WorkTablesIndex therefore no longer depends on
the WorkTable retry feature for this campaign; its provider-level confirmation
runs only after an optimistic miss. The retry-enabled half remains as an
independent composition check.

After removing the retry feature entirely and making the specialized provider
contract the default, two WorkTablesIndex-only matrices ran on the final path:
60 cells with ten repetitions per thread count, followed by 30 cells on the
final inlined code with five repetitions. Across the two runs, all 90 cells
passed, covering 4.5 million requested writes, 3.75 million requested reads,
14,108,737 independently validated online reads, and 12,245 vacuumed pages.
Every error counter was zero. Vanilla IndexSet is no longer a concurrency gate;
it remains available only as an experimental quiescent/single-thread baseline.

## Performance gate

PR #3 and PR #3 plus the fallback were compared in 15 interleaved pairs at 32
threads and 50,000 operations. Median paired throughput changed by -0.33%.
The individual pair range was -2.79% to +2.98%, so the median observation was
inside local run noise. The successful hot path is unchanged; the structural
write guard and scan execute only after an unexpected point-removal miss.

The final specialized point-read design was compared against frozen binaries.
Successful direct index hits changed by a median -0.55% across five alternating
pairs. Generated WorkTable primary-key hits changed by -1.46% across five
pairs, and unique-secondary hits by -1.96% across ten pairs. These are local
speedups, not claims of a general improvement. A 99%-hit/1%-miss randomized
index mix changed by +0.69% across five pairs. A true negative lookup increased
from about 32 ns to about 161 ns because it deliberately enters structural
confirmation. The tradeoff is therefore isolated miss latency; no successful
HFT read regression was measured locally.

## Interpretation and limits

This evidence is strong enough to reject published WorkTablesIndex 0.0.1 for
concurrent WorkTable benchmark publication and to require PR #3, PR #4, and
the first-publication/confirmed-miss stabilization (or their merged
descendants). The final WorkTablesIndex repository gates passed 88 tests, the
all-target/all-feature build, formatting, and Clippy with warnings denied. It
supports a paper claim that the implementation has an independent concurrent
shadow-state validation campaign across primary, unique, non-unique,
deletion, range removal, and vacuum behavior.

It is not a proof of linearizability, a model-checking result, or a final
cross-machine performance result. The runs were on a local ARM64 macOS
development machine without CPU pinning. Final paper numbers need the planned
controlled AWS campaign, repeated trials, disclosed instance details, and raw
result artifacts.
