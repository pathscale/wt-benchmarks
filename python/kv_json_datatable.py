#!/usr/bin/env python3
"""h2oai/datatable adapter for the KV + embedded-JSON comparison (kv_json suite).

datatable (https://datatable.readthedocs.io) is a columnar frame with a C/C++
core — the closest Python analogue to WorkTable's typed-columnar storage. It is
a FAIR comparison only on the columnar-native operations:

  * insert       — bulk build a frame of the shared Account schema
  * query_field  — vectorized filter `active & (age >= MIN_AGE)`

It is deliberately NOT run on point_get / update_field: datatable has no
by-primary-key point get or in-place single-row update (`DT[i, j]` is positional
/ mask selection), so those ops would misrepresent it. Those lanes belong to the
Rust engines (WorkTable typed columns vs redb/lmdb JSON blobs).

Emits one JSON object per line matching the Rust `KvResult` schema (see
src/kv.rs) with suite="kv_json", engine="datatable", so the ladder tooling reads
it identically to the native adapters.

Run (requires `pip install datatable`):
    python3 python/kv_json_datatable.py --rows 10000 --repetitions 5 > results/kv_json-datatable.jsonl
"""

import argparse
import json
import platform
import sys
import time

try:
    import datatable as dt
except ImportError:
    sys.stderr.write(
        "error: the `datatable` package is not installed.\n"
        "install it with `pip install datatable` and re-run.\n"
    )
    sys.exit(2)

MIN_AGE = 40  # must match benches/kv_json.rs


def make_columns(rows):
    """Build the shared Account record as columnar Python lists (matches
    Account::make in src/kv_json.rs field-for-field)."""
    ids = list(range(rows))
    return {
        "id": ids,
        "name": [f"user-{k:08}" for k in ids],
        "email": [f"user{k}@example.test" for k in ids],
        "age": [18 + (k % 60) for k in ids],
        "balance": [k * 1.5 for k in ids],
        "active": [(k % 2 == 0) for k in ids],
    }


# datatable's target triple naming, mapped to the Rust target_arch/target_os
# strings the JSONL uses, so rows line up with the native adapters.
def target_arch():
    m = platform.machine().lower()
    return {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "amd64": "x86_64"}.get(m, m)


def target_os():
    s = platform.system().lower()
    return {"darwin": "macos", "linux": "linux", "windows": "windows"}.get(s, s)


def emit(op, rep, rows, operations, elapsed_ns, checksum):
    print(
        json.dumps(
            {
                "schema_version": 1,
                "suite": "kv_json",
                "engine": "datatable",
                "layer": "columnar-frame",
                "operation": op,
                "repetition": rep,
                "rows": rows,
                "operations": operations,
                "payload_bytes": 0,
                "durability": "memory",
                "transaction_scope": "per-operation",
                "read_ownership": "materialized-frame",
                "elapsed_ns": elapsed_ns,
                "ops_per_second": operations / (elapsed_ns / 1e9) if elapsed_ns else 0.0,
                "checksum": checksum,
                "target_arch": target_arch(),
                "target_os": target_os(),
            }
        )
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rows", type=int, default=10_000)
    ap.add_argument("--repetitions", type=int, default=5)
    args = ap.parse_args()

    for rep in range(1, args.repetitions + 1):
        cols = make_columns(args.rows)

        # insert: build the columnar frame from the shared schema.
        t0 = time.perf_counter_ns()
        frame = dt.Frame(cols)
        insert_ns = time.perf_counter_ns() - t0
        emit("insert", rep, args.rows, args.rows, insert_ns, frame.nrows)

        # query_field: vectorized filter `active & (age >= MIN_AGE)`, then fold
        # the surviving ids into the same checksum shape the Rust query op uses.
        t0 = time.perf_counter_ns()
        hits = frame[(dt.f.active) & (dt.f.age >= MIN_AGE), :]
        id_sum = 0
        for v in hits["id"].to_list()[0]:
            id_sum = (id_sum + v) & 0xFFFFFFFFFFFFFFFF
        query_ns = time.perf_counter_ns() - t0
        # `operations` for a scan is the number of rows scanned (matches the
        # Rust query_field group throughput basis of ROWS).
        emit("query_field", rep, args.rows, args.rows, query_ns, id_sum)


if __name__ == "__main__":
    main()
