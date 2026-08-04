# Binary and persistent-storage footprint campaign

This standalone campaign measures two different deployment costs:

1. the shipped application executable for a minimal typed/indexed table; and
2. every database file required to reload the same logical dataset.

It is intentionally isolated from the main `wt-benchmarks` Cargo package. Its
scripts build into a temporary target directory and write only beneath this
campaign's ignored `results/` directory unless an explicit output path is
provided.

The full research rationale, reporting rules, and limitations are in
[`../../docs/BINARY_AND_STORAGE_FOOTPRINT.md`](../../docs/BINARY_AND_STORAGE_FOOTPRINT.md).

## Executable footprint

```bash
scripts/run-executable-footprint.sh
```

The matrix builds a no-database Rust control, WorkTable, bundled SQLite, and
redb with 1, 2, 4, and 8 distinct schemas. Each is built under both:

- `paper-speed`: `opt-level=3`, fat LTO, one codegen unit, symbols stripped;
- `paper-size`: `opt-level=s`, fat LTO, one codegen unit, symbols stripped.

The output retains each executable, its SHA-256, dynamic dependency listing,
and `llvm-size`/`size` section report. Raw file bytes are the portable primary
metric. A system-SQLite executable must not be compared by file bytes alone;
its SQLite dynamic-library closure is part of the deployment.

## Persistent storage footprint

```bash
WT_FOOTPRINT_ROWS=10000 scripts/run-storage-footprint.sh
```

The development default is 100,000 rows. The paper configuration should use
at least 1,000,000 rows and sweep payloads of 0/16/64/256/1024 bytes after the
small run is stable. The workload has five typed columns and one non-unique
secondary index. It reports the following states as JSONL:

- `loaded`: dense initial load;
- `churned`: 25% deletes plus 25% same-length updates;
- `vacuumed` or `compacted`: backend compaction operation completed;
- `reloaded`: row count checked after reopen.
- `closed-after-reload`: final cleanly closed deployment footprint.

Both logical file length and allocated filesystem blocks are reported. The
SQLite runner additionally reports page count, freelist count, and page size.

The WorkTable runner drains its asynchronous persistence queue every
`WT_FOOTPRINT_DRAIN_EVERY` rows and before every measurement. Its result is a
warm-restart persistence footprint, not a crash-durability-equivalent result.

## Development verification

```bash
CARGO_TARGET_DIR=/tmp/wt-footprint-check cargo check --offline \
  --all-targets \
  --features worktable-backend,sqlite-backend,redb-backend,tables-8
```
