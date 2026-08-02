# Official measurement campaign

Twenty hours of machine time is a reasonable minimum once external engines,
fresh loads, ten repetitions, and outlier reruns are included. Analysis and
paper-figure production should be budgeted separately.

## Preconditions

- WorkTable and WorkTablesIndex target commits are merged, recorded, and not
  changed during the campaign.
- The suite commit and every external dependency version are locked.
- A dedicated AWS instance is provisioned with no burstable CPU credits or
  unrelated services; CPU topology, frequency policy, NUMA, storage, kernel,
  and allocator are captured.
- All binaries are built before measurement. The compile-cost suite uses a
  separate clean target directory and is not allowed to warm or disturb the
  runtime campaign.
- A development-scale smoke matrix has zero operation errors and all
  correctness invariants pass.

## Twenty-hour machine schedule

| Window | Campaign |
|---|---|
| 0:00–2:00 | Host validation, thermal/frequency stability, build verification, data-size and duration calibration, one discarded warm-up sweep |
| 2:00–5:00 | Micro layers, specialization ladder, memory/row, compile and binary-size controls; ten randomized repetitions |
| 5:00–11:00 | Portable workload sweep: selected paper mixes first, then YCSB A–F and external stores; include load time separately |
| 11:00–15:00 | Concurrency and tail latency: threads, disjoint/overlap, hot keys, open-loop rate sweep, publication feature off/on |
| 15:00–18:00 | HFT order lifecycle/book path, desktop cold start/project view, and SaaS support/HTTP workloads |
| 18:00–20:00 | Mandatory reruns for wide confidence intervals, drift, failed invariants, suspicious reversals, and the exact paper candidate configurations |

If external-engine compiles or database loads consume this window, extend the
campaign rather than cutting repetitions. Compilation should normally happen
before the clock starts.

## Paper-first ordering

The exact paper candidate configurations run early and late. This detects host
drift and protects the deadline if lower-priority website sweeps overrun:

1. specialization ladder;
2. micro L0/L1/L2 representatives;
3. read-mostly, update-heavy, and RMW mix;
4. disjoint/overlapping contention and publication off/on;
5. one production-derived HFT path if ready.

## Statistical rules

- Ten measured repetitions for paper candidates; five for broad website
  exploration unless variance requires more.
- Randomize/interleave engine order rather than running every WorkTable test
  first and every external engine last.
- Preserve every run. Do not drop an outlier without a recorded external cause.
- Report median, bootstrap 95% confidence interval, and relative difference.
- A repeatable 2% regression is material; investigate it.
- Separate throughput saturation from latency. Tail-latency runs use open-loop
  offered rates and report deadline misses/coordinated-omission handling.
- Any correctness failure invalidates the performance result.

## After machine time

Reserve at least four additional human/analysis hours to validate result
manifests, calculate intervals, inspect distributions, select paper panels,
generate figures, and reproduce the selected numbers from the recorded command.

