# WorkTable concurrency scalability

The checked-in `concurrent_mix` benchmark includes two bounded scaling axes
across WTI, Arctic and Congee. Both use 1/2/4/8/16/32 threads, 4,000
pre-generated operations per thread, a 20,000-row table and disjoint key
ranges.

- `concurrent_read_scale` contains successful point reads only.
- `concurrent_scale` applies a deterministic 10% write ratio in every thread.

The beta18 local candidate produced these median Mops/s values:

| workload | backend | 1 | 2 | 4 | 8 | 16 | 32 |
|---|---|---:|---:|---:|---:|---:|---:|
| read | WTI | 8.748 | 14.042 | 24.181 | 22.968 | 31.507 | 23.382 |
| read | Arctic | 8.918 | 14.002 | 21.829 | 29.631 | 27.446 | 24.830 |
| read | Congee | 9.539 | 18.847 | 24.446 | 22.006 | 19.542 | 24.456 |
| 10% write | WTI | 4.826 | 5.426 | 9.701 | 4.632 | 4.256 | 4.055 |
| 10% write | Arctic | 4.852 | 5.066 | 8.601 | 4.651 | 4.709 | 5.164 |
| 10% write | Congee | 5.436 | 6.241 | 8.605 | 4.449 | 4.477 | 4.703 |

The host has 12 performance and four efficiency cores. Read-only speedup is
3.60x WTI, 3.32x Arctic and 2.56x Congee at each backend's best cell. The
10%-write workload peaks at four threads and drops for all three backends,
placing the ceiling in common WorkTable machinery rather than one selected
index. The same shape exists in beta13 and beta15. Adjacent equal-profile
beta18/beta15 reruns found one reproducible loss, Arctic at eight threads
(4.651 versus 5.022 Mops/s, -7.4%), while beta18 is 4.8% and 6.5% faster at 16
and 32 threads.

## Local version grid

Pure-read medians in Mops/s:

| version/backend | 1 | 2 | 4 | 8 | 16 | 32 |
|---|---:|---:|---:|---:|---:|---:|
| beta13 WTI | 7.515 | 10.961 | 13.898 | 9.627 | 7.939 | 7.897 |
| beta13 Arctic | 7.989 | 10.082 | 12.260 | 9.876 | 7.808 | 7.849 |
| beta13 Congee | 7.839 | 9.284 | 13.256 | 9.883 | 7.822 | 7.893 |
| beta15 WTI | 7.300 | 10.305 | 18.209 | 21.483 | 26.830 | 23.305 |
| beta15 Arctic | 7.059 | 8.146 | 10.982 | 16.246 | 21.964 | 22.260 |
| beta15 Congee | 6.870 | 13.446 | 18.762 | 20.295 | 19.688 | 23.258 |
| beta18 WTI | 8.748 | 14.042 | 24.181 | 22.968 | 31.507 | 23.382 |
| beta18 Arctic | 8.918 | 14.002 | 21.829 | 29.631 | 27.446 | 24.830 |
| beta18 Congee | 9.539 | 18.847 | 24.446 | 22.006 | 19.542 | 24.456 |

Ten-percent-write medians in Mops/s:

| version/backend | 1 | 2 | 4 | 8 | 16 | 32 |
|---|---:|---:|---:|---:|---:|---:|
| beta13 WTI | 3.994 | 4.109 | 6.987 | 4.758 | 4.302 | 4.045 |
| beta13 Arctic | 4.220 | 4.669 | 6.955 | 4.294 | 4.172 | 4.188 |
| beta13 Congee | 4.426 | 5.589 | 9.032 | 5.070 | 4.589 | 4.617 |
| beta15 WTI | 3.958 | 4.527 | 8.565 | 4.342 | 4.148 | 4.014 |
| beta15 Arctic | 4.003 | 4.954 | 7.860 | 5.022 | 4.492 | 4.851 |
| beta15 Congee | 4.163 | 5.241 | 8.333 | 4.537 | 4.607 | 4.595 |
| beta18 WTI | 4.826 | 5.426 | 9.701 | 4.632 | 4.256 | 4.055 |
| beta18 Arctic | 4.852 | 5.066 | 8.601 | 4.651 | 4.709 | 5.164 |
| beta18 Congee | 5.436 | 6.241 | 8.605 | 4.449 | 4.477 | 4.703 |

All WorkTable, DataBucket and WorkTablesIndex versions in this grid came from
local worktrees. Beta13 and beta15 were paired with their matching local
dependency versions. Contradictory cells were rerun as adjacent beta18/beta15
pairs; the table uses those targeted medians and retains the original output
in the validation record.

```sh
cargo bench --offline --bench concurrent_mix -- concurrent_read_scale
cargo bench --offline --bench concurrent_mix -- concurrent_scale
```
