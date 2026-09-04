# MoE-PGO local WorkTable version grid

Validated 2026-09-05. No WorkTable-family crate in this grid came from
crates.io.

The isolated harness in `tools/version-grid` compiles only `moe_pgo`, allowing
the same benchmark source to build against local beta13, beta15 and beta18
APIs. Historical WorkTable versions are paired with their exact local
DataBucket and WorkTablesIndex versions. Arctic, Congee and ps-reclaim resolve
to the local development checkouts.

Only donor width 12,288 and the no-WorkTable control are run. Criterion uses 10
samples and five seconds per phase; the control uses 20 samples and three
seconds. The accepted pass controls were:

| version | control median |
|---|---:|
| beta13 | 1.5329 ms |
| beta15 | 1.5416 ms |
| beta18 | 1.5501 ms |

Their 1.1% span is the host-noise validity bound for this grid. A first beta15
pass produced a 1.9398 ms control and was rejected; its raw phase values remain
in the session record rather than being blended into the accepted run.

## Raw medians

| phase | backend | beta13 | beta15 | beta18 |
|---|---|---:|---:|---:|
| accumulate, 200k updates | WTI | 127.22 ms | 121.01 ms | 87.146 ms |
|  | Congee | 96.937 ms | 76.214 ms | 63.854 ms |
|  | Arctic | 85.409 ms | 75.556 ms | 73.496 ms |
|  | Array control | 324.20 µs | 327.37 µs | 332.02 µs |
| publish, 8 × 12,288 rows | WTI | 56.450 ms | 59.550 ms | 34.917 ms |
|  | Congee | 37.259 ms | 39.042 ms | 11.399 ms |
|  | Arctic | 34.202 ms | 34.881 ms | 7.7407 ms |
| retire, 8 maps under 4 readers | WTI | 3.8748 µs* | 1.7007 ms | 50.655 µs |
|  | Congee | 3.7604 µs* | 2.4858 ms | 1.7240 ms |
|  | Arctic | 3.8184 µs* | 2.1884 ms | 135.64 µs |

`*` Beta13's retire implementation does not perform production-accessible
reclamation. It pushes the old map into a retained `Vec<Arc<_>>`; reclaim
requires an exclusive `&mut self` call that the shared router cannot make.
Those cells time a leak and are not compared with beta15/beta18 reclamation.

Against beta15, beta18's real retire phase is 33.6x faster for WTI, 1.44x for
Congee and 16.1x for Arctic. Publish plus retire improves by 42.9%, 68.4% and
78.8%, respectively. Accumulate improves by 28.0%, 16.2% and 2.7%.

## Reproduction

```sh
scripts/compare-worktable-versions.sh beta13 \
  /private/tmp/worktable-beta13 \
  /private/tmp/databucket-053 \
  /private/tmp/wti-006

scripts/compare-worktable-versions.sh beta15 \
  /private/tmp/worktable-beta15 \
  /private/tmp/databucket-055 \
  /private/tmp/wti-009
```

Read `moe_pgo/control/fixed_work` first. If controls do not match, repeat the
pair; do not normalize or conceal a moved host.
