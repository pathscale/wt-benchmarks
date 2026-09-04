# MoE resident provenance index A/B

This experiment isolates the homegrown resident lookup used by MoE-PGO and
compares it with generated WorkTable tables using Arctic, WorkTablesIndex, and
Congee. All WorkTable results below use the published crates.io
`worktable = 1.0.0-beta.17`.

The logical relation has 1,528 `(source, ordinal)` origin keys and a fixed
five-field payload. The benchmark issues one million deterministic successful
point queries per sample for nine samples. The same query stream is used for
all arms, and execution stops if any checksum differs.

## Results

Two consecutive release runs on 2026-09-04:

| arm | run 1 ns/query | run 2 ns/query |
|---|---:|---:|
| application-style linear Vec scan | 208.17 | 208.49 |
| WorkTable-vec rows + BTreeMap index | 32.34 | 32.18 |
| WorkTable-vec rows + Arctic index | 5.25 | 5.55 |
| generated WorkTable + Arctic | 26.07 | 26.37 |

A later six-arm run measured 206.49 ns/query for linear Vec, 31.80 for
Vec+BTreeMap, 5.28 for Vec+Arctic, 26.14 for WorkTable+Arctic, 63.91 for
WorkTable+WTI, and 33.65 for WorkTable+Congee. Arctic is therefore the fastest
full WorkTable backend for this point-lookup shape.

At this population, replacing BTreeMap with Arctic while keeping the same Vec
rows is 5.80–6.16x faster. Full WorkTable+Arctic is 7.91–7.98x faster than the
linear scan and 1.22–1.24x faster than Vec+BTreeMap. The full table costs
4.75–4.96x over the minimal Vec+Arctic row-offset index; that delta is the
measured price of the generated table path rather than an index-backend delta.

One-population construction in the two runs was:

| arm | run 1 ms | run 2 ms |
|---|---:|---:|
| linear Vec | 0.312 | 0.312 |
| Vec+BTreeMap | 0.115 | 0.116 |
| Vec+Arctic | 0.083 | 0.184 |
| WorkTable+Arctic | 0.497 | 0.524 |

Construction numbers are single observations and are not treated as a stable
benchmark.

## Memory result

An isolated-process counting allocator measured the same 1,528 rows with
`persist:false`:

| arm | retained bytes | peak bytes | bytes after drop |
|---|---:|---:|---:|
| linear Vec | 36,672 | 36,672 | 0 |
| Vec+BTreeMap | 77,088 | 77,088 | 0 |
| Vec+Arctic | 105,824 | 105,824 | 64 |
| WorkTable+Arctic | 396,548 | 396,588 | 73,004 |
| WorkTable+WTI | 359,460 | 359,500 | 67,868 |
| WorkTable+Congee | 378,700 | 378,740 | 44,212 |

The focused beta.17 regression is WorkTable+Arctic retaining 3.75x the bytes
of the stripped Vec+Arctic control, with 73,004 bytes still allocated after
drop. This reproduced identically against the local checkout and the published
crate; the catalogued result is the published-crate run.

## Persisted Arctic result

The `persist:true` runner uses the same relation and published beta.17, closes
it, reopens it, validates all 1,528 rows and the checksum, and then compares
warm point lookup to the `persist:false` generated WorkTable+Arctic arm:

| measurement | result |
|---|---:|
| complete persisted artifact | 128,055 B |
| empty create/load | 0.649 ms |
| insert visible | 1.532 ms |
| insert durable (`wait_for_ops`) | 9.725 ms |
| reopen | 0.646 ms |
| first reopened query | 584 ns |
| warm in-memory | 25.93 ns/query |
| warm reopened persisted | 25.92 ns/query |

Persistence did not degrade the warm lookup path in this run (0.999x). This is
not a durability-parity claim: startup, background drain, and steady-state
lookup are reported as separate operations.

## Control result: dense counters are different

The existing dense profiling-counter test reported 605.4 million updates/s
for a raw atomic array, 544.1 million/s with a ready `.await`, and 2.3
million/s for WorkTable+Arctic. That roughly 263x array advantage is real for a
dense integer address with no query or persistence need. It must not be used to
reject WorkTable for provenance point lookup, where the current baseline is a
search or separately maintained index.

## Reproduction

```sh
env CARGO_BUILD_JOBS=2 RAYON_NUM_THREADS=2 nice -n 15 \
  cargo run --release --bin moe-resident-index-ab

env CARGO_BUILD_JOBS=2 RAYON_NUM_THREADS=2 nice -n 15 \
  cargo run --release --bin moe-resident-memory-ab

env CARGO_BUILD_JOBS=2 RAYON_NUM_THREADS=2 nice -n 15 \
  cargo run --release --bin moe-resident-persist-ab
```
