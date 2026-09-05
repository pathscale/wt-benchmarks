# Probe order, concurrency, and the WorkTablesIndex point-get verdict

2026-09-05. Three measurements against one fixture: the `fn:unit%06d/loop:%d`
structural-path keys at 163 / 512 / 8,192 / 131,072 that `benches/arctic_paths.rs`
established for the EKOPathRS consumer profile.

Conditions are in [machine.txt](machine.txt). Every file here is raw captured
output, kept because the null arms in it are what say which rows are usable.

| File | Command | What it is |
|---|---|---|
| `probe_order.txt` | `cargo bench --bench probe_order` | The run to read. Four probe orders, three backends, four sizes, null twin per order. |
| `probe_order-fixed-arm-confound.txt` | same | **Invalid for the `fixed` rows only.** An earlier build whose `fixed` arm walked a one-element probe vector while every other arm walked `n`, so the modulus, the vector footprint and the loop shape all differed at once. Kept because its non-`fixed` arms are identical code to the good run and agree with it to 0.3-2.5% at 163/512/8,192, which is the run-to-run figure the good run's own null arm cannot supply. Both runs move by 10-14% at 131,072 shuffled; treat that row as +/-14%. |
| `arctic_concurrent.txt` | `cargo bench --bench arctic_concurrent` | The run to read. Six arms, 1/2/4/8 threads, four sizes, write mixes `w0` and `w20`. |
| `arctic_concurrent-w20-only.txt` | same | An earlier build with only the 5% mix. Superseded; agrees with the `w20` half of the good run. |
| `wti-point-get.txt` | `crates/wti-point-get-case`, `--budget-secs 400` | The point-get decomposition. Medians with p10/p90 over 15 repetitions, arms interleaved and reshuffled per repetition. |

## Rows that are not trustworthy

The machine was shared with other agent lanes throughout. The null arms caught
three places, and they are the only places to distrust:

- `probe_order.txt`, **512 / in_order**: null floor 11.5%. Every other cell in that
  run is under 6.7% and most are under 1.5%.
- `probe_order.txt`, **131,072 / shuffled**, all three backends: 10-14% between the
  two runs, which is larger than the within-run floor. The ratios drawn from it
  (2.3x to 2.8x) clear that comfortably; the absolute nanoseconds do not.
- `arctic_concurrent.txt`, **8,192 / w0 / t1 and t2**: null floor 21.2% at t1, and
  both Arctic arms sit ~40% below their `w20` twins in the same cells, which is
  backwards. A block-local disturbance. The t4 and t8 cells of the same row are fine.

## What each one concluded

**Probe order is the largest effect, and it is cache behaviour, not a defect.**
Against each backend's own in-order arm at 131,072: shuffling costs `std` 2.27x,
WorkTablesIndex 2.75x and Arctic 1.70x, while a single fixed probe *pays back*
0.57x / 0.62x / 0.89x. The give-away is that the fixed arm is nearly flat across
three orders of magnitude - `std` 1.4x from 163 to 131,072 against 8.2x shuffled -
which is what a fully resident search path looks like, and the divergence between
orders appears exactly where the population stops fitting in cache. Zipf 0.99 sits
between the two and tracks shuffled at scale, because `mix64` scatters the hot set
rather than clustering it.

The `fixed` arm is not simply a best case: it changes the measured quantity from
lookup throughput to single-lookup latency, which is why it is flat. That is what
made an earlier review, probing one key, report a 1.4x-shaped result where the real
scaling is 8x.

**Arctic's `ConcurrentMap` scales and every lock does not.** 6.2x to 8.2x from 1 to
8 threads; `std::BTreeMap`, `SequentialMap` and WorkTablesIndex's concurrent map all
peak at 1 or 2 threads and fall from there, ending 20x to 26x behind at 8 threads.
The `w0` mix shows the loss is the read-lock atomic rather than writer exclusion.
`NoOp` SMR is 2-8% above `ps-reclaim`, much of it inside the floor, so safe memory
reclamation is close to free on this read path - which is *not* the same claim as
`ps-reclaim` beating `seize`, and this bench does not make that one.

**The WorkTablesIndex point-get gap is real and explained.** 3.38x / 3.16x / 2.25x /
2.57x behind `std` against a 0.1-4.0% floor, and it is the node shape on string keys
only: a flat sorted `Vec<(&str, u64)>` searched with `slice::partition_point` costs
90-94% of `get`, and on `u64` keys from the same population WorkTablesIndex beats
`std` by 1.8x. See the module header of `crates/wti-point-get-case/src/main.rs` and
the EKOPathRS section of [`../../docs/BENCHMARK_CATALOG.md`](../../docs/BENCHMARK_CATALOG.md).
