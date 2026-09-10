# WorkTablesIndex point-get decomposition

The retained result isolates the sequential WorkTablesIndex string point-get
gap found by `benches/arctic_paths.rs`.

- `wti-point-get.txt`: raw medians and p10/p90 across 15 interleaved
  repetitions, with null twins, node-capacity controls, integer controls, and
  equivalent flat-array searches.
- `machine.txt`: machine and toolchain conditions for the captured run.

The other probe-order and concurrency captures from the source audit were not
folded in: the current benchmark branch has a corrected start gate, pure-read
group, and separately named 95/5 mixed group, so those older files are not the
canonical concurrency evidence.
