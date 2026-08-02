# Comparison layers

There is no single honest “WorkTable versus everything” chart. The suite uses
layers so readers can see both the cost of added capability and the comparison
to products they might actually choose.

| Layer | Engines | Question |
|---|---|---|
| L0 — language lower bound | `Vec<T>`, `HashMap`, `BTreeMap` | How far is WorkTable from direct specialized Rust storage? |
| L1 — synchronized collections | `Vec<RwLock<T>>`, `RwLock<HashMap>`, DashMap, WorkTablesIndex primitives | What does typed table/index machinery cost versus common concurrent collections? |
| L2 — in-process tables | WorkTable, SQLite `:memory:` | What is the difference between generated typed access and a general embedded relational engine without disk? |
| L3 — embedded stores, relaxed durability | redb, LMDB/heed, RocksDB with explicit non-sync/WAL modes, SQLite memory/WAL variants | How do engine costs compare when durability guarantees are deliberately minimized and labeled? |
| L4 — embedded stores, default durability | redb, LMDB/heed, RocksDB, SQLite WAL, persisted WorkTable | What does an application get under each engine's documented durable mode? |
| L5 — application | HFT event path, desktop cold start, SaaS HTTP TPS | Does the engine matter in the complete host-Rust workflow? |

Paper candidates should normally include one representative from L0, L1, L2,
and L3/L4, not every engine. The website carries the complete ladder.

## Initial external target set

The high-value core is:

- SQLite `:memory:` and WAL: familiar table/SQL reference;
- redb: maintained pure-Rust embedded B-tree store;
- LMDB through heed: mature memory-mapped B-tree reference;
- RocksDB: widely recognized LSM reference, always labeled as a semantically
  different persistent KV engine;
- DashMap: common concurrent Rust map;
- `Vec`, `HashMap`, and `BTreeMap`: transparent lower bounds.

Add fjall to the website after the core adapters. Keep sled only as a historical
website datapoint if its pinned version still builds; it should not consume
paper space.

## Required modes

Do not collapse these into one bar:

- engine-only versus serialization/HTTP end-to-end;
- borrowed/reference read versus owned/materialized read;
- default versus stronger WorkTable publication;
- no persistence, background persistence, acknowledged write, and fsync;
- warm versus cold database/cache;
- single-thread throughput versus open-loop tail latency.

