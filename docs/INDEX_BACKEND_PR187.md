# PR #187 expanded ARM result

This is a local screening result for WorkTable PR #187, not a paper-ready
hardware claim. It identifies which providers deserve controlled AWS ARM runs.

## Run identity

- WorkTable revision: `ba6a8d0a3c864207201be16fed06bfce3d0629d1`
- Dependency branch: `feat/index-backend-using`
- Host: Apple ARM64, macOS 26.5
- Repetitions: 7, with rotating provider order and one warmup per provider
- Initial rows: 250,000
- Point operations per read phase: 2,000,000
- Range queries per width: 200
- Steady inserts and deletes: 100,000 each
- Schema: sequential autoincrement `u64` primary key, sequential unique `u64`
  secondary key, fixed 32-byte payload
- Persistence: disabled for all providers
- Process isolation: one provider per process

The values below are medians across seven repetitions. Throughput includes all
operations; p99 comes from one latency sample per 64 operations.

## Throughput and p99

| Provider | Initial insert ops/s (p99) | PK hit ops/s (p99) | Unique hit ops/s (p99) | Steady insert ops/s (p99) | Delete ops/s (p99) |
|---|---:|---:|---:|---:|---:|
| WorkTablesIndex | 1.516M (5.92 us) | 20.34M (750 ns) | 18.61M (125 ns) | 1.774M (1.83 us) | 0.594M (2.71 us) |
| IndexSet | 1.478M (6.33 us) | 16.77M (541 ns) | 18.59M (125 ns) | 1.806M (1.75 us) | 0.584M (2.75 us) |
| Congee | 2.876M (2.50 us) | 40.31M (125 ns) | 40.37M (125 ns) | 2.735M (1.50 us) | 0.641M (2.50 us) |
| Arctic | 2.986M (2.50 us) | 60.54M (125 ns) | 61.22M (84 ns) | 2.869M (1.92 us) | 0.578M (3.25 us) |

IndexSet is effectively in the WorkTablesIndex performance class in this
single-thread point campaign: slightly faster initial/unique operations and
slightly slower primary reads, steady inserts, and deletes. Congee and Arctic
are the clear point-operation candidates for deeper testing. Their range
behavior must not be inferred from this table.

## Allocations and resident memory

All four providers performed zero allocator calls in the primary-hit,
unique-hit, and primary-miss phases.

| Provider | Initial-insert allocation calls/op | Initial-insert bytes/op | RSS after 250K-row load | Allocator live-byte load delta |
|---|---:|---:|---:|---:|
| WorkTablesIndex | 4.083 | 604 B | 58.22 MiB | 47.93 MiB |
| IndexSet | 6.087 | 1,195 B | 57.33 MiB | 47.87 MiB |
| Congee | 4.062 | 370 B | 53.70 MiB | 44.09 MiB |
| Arctic | 4.062 | 349 B | 48.83 MiB | 38.41 MiB |

RSS is current resident size from Mach task info, not the adapters'
`system_info()` estimate. Allocation counters wrap the process global allocator
and therefore include common WorkTable insertion machinery as well as the
selected indexes; the relative result is the useful part.

## Range-shape crossover

The current Congee and Arctic adapters materialize a full index snapshot before
applying a range. That cost is almost independent of the requested width and is
especially visible for selective ranges:

| Provider | One-row range ops/s | Bytes allocated/query | Full range ops/s | Bytes allocated/query |
|---|---:|---:|---:|---:|
| WorkTablesIndex | 1.661M | 376 B | 172 | 24.00 MiB |
| IndexSet | 1.145M | 376 B | 228 | 24.00 MiB |
| Congee | 151 | 17.91 MiB | 107 | 41.91 MiB |
| Arctic | 751 | 12.00 MiB | 247 | 36.00 MiB |

This is the strongest evidence for the `using` design: Arctic and Congee win
the point-heavy phases, while WorkTablesIndex and IndexSet are orders of
magnitude better for selective ordered ranges. These are adapter results, not
an inherent limit of ART indexes; a cursor-based range adapter could change
the result.

## Boundaries before publication

- This campaign is single-threaded. Contended point operations remain required.
- Congee and Arctic require `persist: false`; this run therefore does not make a
  durability comparison.
- Range scans are included to expose the current snapshot-allocation boundary;
  they must not be blended into a point-operation score.
- Vanilla IndexSet persistence and WorkTablesIndex -> IndexSet ->
  WorkTablesIndex switching were tested by PR #187's integration suite, not
  benchmark-timed here.
- Sub-microsecond sampled p99 values approach the host clock's useful
  resolution. Throughput is the more stable discriminator for the fastest read
  phases.
- Re-run the shortlist on controlled AWS ARM hardware, with pinned CPU policy
  and captured environment metadata, before using any number in the paper or
  on worktable.dev.
