# PR #187 preliminary ARM result

This is a local screening result for WorkTable PR #187, not a paper-ready
hardware claim. It identifies which providers deserve controlled AWS ARM runs.

## Run identity

- WorkTable revision: `114574f0f005560712a738655857d39818c2c262`
- Dependency branch: `feat/index-backend-using`
- Host: Apple ARM64, macOS 26.5
- Repetitions: 7, with rotating provider order and one warmup per provider
- Initial rows: 250,000
- Point operations per read phase: 2,000,000
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
| WorkTablesIndex | 1.459M (6.54 us) | 19.22M (459 ns) | 18.06M (125 ns) | 1.405M (1.96 us) | 0.509M (3.13 us) |
| IndexSet | 1.478M (6.67 us) | 18.81M (500 ns) | 19.41M (125 ns) | 1.385M (1.96 us) | 0.496M (3.13 us) |
| Congee | 2.853M (2.71 us) | 43.86M (125 ns) | 44.13M (125 ns) | 2.749M (1.58 us) | 0.546M (2.88 us) |
| Arctic | 2.967M (2.54 us) | 74.26M (125 ns) | 74.57M (125 ns) | 2.895M (1.46 us) | 0.499M (3.13 us) |

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
| WorkTablesIndex | 7.090 | 1,469 B | 58.20 MiB | 47.93 MiB |
| IndexSet | 7.089 | 1,484 B | 57.27 MiB | 47.87 MiB |
| Congee | 5.064 | 658 B | 53.62 MiB | 44.09 MiB |
| Arctic | 5.064 | 637 B | 48.72 MiB | 38.41 MiB |

RSS is current resident size from Mach task info, not the adapters'
`system_info()` estimate. Allocation counters wrap the process global allocator
and therefore include common WorkTable insertion machinery as well as the
selected indexes; the relative result is the useful part.

## Boundaries before publication

- This campaign is single-threaded. Contended point operations remain required.
- Congee and Arctic require `persist: false`; this run therefore does not make a
  durability comparison.
- Range scans are excluded because the current Congee and Arctic adapters
  allocate full snapshots for range/iteration.
- Vanilla IndexSet persistence and WorkTablesIndex -> IndexSet ->
  WorkTablesIndex switching were tested by PR #187's integration suite, not
  benchmark-timed here.
- Sub-microsecond sampled p99 values approach the host clock's useful
  resolution. Throughput is the more stable discriminator for the fastest read
  phases.
- Re-run the shortlist on controlled AWS ARM hardware, with pinned CPU policy
  and captured environment metadata, before using any number in the paper or
  on worktable.dev.
