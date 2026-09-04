# Multiple persisted instances

This focused compatibility case asks whether published WorkTable
`1.0.0-beta.17` can instantiate one generated schema more than once. It opens
two independent persisted instances, inserts disjoint rows, gracefully closes
and reloads both, then closes one while proving the other remains queryable.

Run from outside this repository so the experimental local overrides in
`.cargo/config.toml` do not replace the published crates:

```sh
cd /Users/revenge/code
CARGO_BUILD_JOBS=2 nice -n 10 cargo run --manifest-path \
  /Users/revenge/code/wt-benchmarks/campaigns/multi-cluster-instance/Cargo.toml
```

This tests table-template multiplicity and lifecycle. It does not claim that
beta17 can store multiple generated table instances as named spaces inside one
already-open DataBucket file; its public persistence config is path/table-name
oriented.

Measured 4 September 2026 against crates.io (not the repository's local patch
set): the run passed with `schema_instances=2`, isolated queries, graceful
close/reload, and independent unload all true. The warm application-only rerun
finished in 2.9 seconds including a 2.1-second dev-profile compile; these are
test harness timings, not a database load benchmark.
