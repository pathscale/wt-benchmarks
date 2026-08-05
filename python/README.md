# Python adapters

Cross-language engines for the benchmark ladder. Each script emits JSONL rows
matching the Rust `KvResult` schema (see `../src/kv.rs`) so the ladder tooling
reads them identically to the native adapters.

## kv_json_datatable.py — h2oai/datatable, kv_json suite

[datatable](https://datatable.readthedocs.io) is a columnar frame with a C/C++
core — the closest Python analogue to WorkTable's typed columnar storage.

Scoped to the two columnar-native ops where it is a fair comparison:
- `insert` — bulk-build a frame of the shared `Account` schema
- `query_field` — vectorized `active & (age >= 40)` filter

Deliberately NOT run on `point_get` / `update_field`: datatable has no
by-primary-key point get or in-place single-row update, so those lanes belong to
the Rust engines (WorkTable typed columns vs redb/lmdb JSON blobs).

The schema and the `age >= 40 && active` predicate mirror `benches/kv_json.rs`
and `src/kv_json.rs::Account` field-for-field, so the `datatable` rows drop into
the same `kv_json/insert` and `kv_json/query_field` comparison.

```
pip install datatable
python3 python/kv_json_datatable.py --rows 10000 --repetitions 5 \
    > results/kv_json-datatable.jsonl
```
