# What we compare, what is fair, and how to read it

This document is the comparison contract for every WorkTable number we publish,
in the paper, on the website, or anywhere else. A benchmark is only worth
showing if the reader can tell what is being measured and why the comparison is
honest. The governing rule is simple:

> **Never let a single number imply that two systems do the same work.**
> State the capability difference next to every comparison, in both directions.

WorkTable straddles two worlds. It looks "unfairly slow" next to a raw `Vec`
(which is not a database and has no index, concurrency, ownership, or
persistence) and "unfairly fast" next to a durable embedded store (which pays
for crash safety and cross-process access WorkTable does not offer). Both gaps
are real and both are the point: WorkTable occupies a spot on the spectrum that
neither neighbor occupies. Our job is to locate it honestly, not to win either
comparison.

---

## 1. The capability axes

Every comparison must be labeled on these axes. Two systems are only
"apples-to-apples" for an operation when they match on the axes that operation
exercises.

| Axis | Values |
|---|---|
| **Read ownership** | borrowed reference into the container vs. owned/materialized row |
| **Secondary access** | point-by-primary-key only vs. secondary-index lookup vs. ordered range |
| **Concurrency** | single-thread vs. concurrent readers/writers |
| **Typing** | untyped bytes/JSON vs. typed columns checked at compile time |
| **Durability** | in-memory only vs. background persist (no fsync) vs. synced/WAL |
| **Process model** | in-process only vs. cross-process |
| **Transactions** | per-operation atomicity vs. multi-statement transactions |

A comparison that differs on an axis the operation does not touch is fine. A
comparison that differs on an axis the operation *does* touch must say so, or it
is misleading.

---

## 2. The layers (who we compare against, and the honest question each answers)

Extends `COMPARISON_LAYERS.md`. Each layer answers a different question; none
answers "is WorkTable the fastest."

| Layer | Compared against | Honest question | Expected result |
|---|---|---|---|
| **L0 language floor** | raw `Vec<T>`, `HashMap`, `BTreeMap` (borrowed, positional, single-thread) | How close to the language floor is a *real engine*? | WorkTable is **slower** on raw ops; that closeness (small constant, not orders of magnitude) is the result. |
| **L1 hand-rolled table** | `Vec` + hand-maintained index + `RwLock` + owned reads (`vec_realistic`); an arena crate the ecosystem uses (`slotmap`, 90M downloads) | What does the typed table machinery cost vs. what a competent engineer hand-writes? | WorkTable trails on raw handle ops; **wins decisively on secondary lookup and range**, where the hand-roll has no index. |
| **L1' real-world jank** | `Vec` + linear scan + embedded-JSON per access (`vec_janky`), modeled on real production code (e.g. `iter().find()` + `serde_json::from_str`) | What does WorkTable replace in code people actually write? | WorkTable is **1-2 orders of magnitude faster** on lookup/update; this is the strongest honest comparison. |
| **L2 in-process engine** | SQLite `:memory:` | Generated typed access vs. a general embedded relational engine, no disk | comparison restricted to in-memory, non-durable ops; SQLite pays for SQL. |
| **L3 durable, relaxed** | redb, LMDB/heed, RocksDB in explicit **non-sync** mode; **persisted WorkTable, background mode** | Engine cost when durability is deliberately minimized and labeled | capability-matched: WorkTable persist ON (queue-drain), peers in no-sync/WAL mode. |
| **L4 durable, default** | redb, LMDB/heed, RocksDB, SQLite WAL; **persisted WorkTable** | What each engine gives under its documented durable mode | WorkTable has **no fsync tier**; its "durable" is background + drain. State this; do not claim fsync parity. |
| **L5 application** | end-to-end host-Rust workflow (HFT event path, cold start, SaaS TPS) | Does the engine matter in the whole workflow? | the number users actually feel. |

Paper: one representative from L0, L1/L1', L2, L3-or-L4. Website: the full
ladder.

---

## 3. The fairness rules (non-negotiable)

1. **Same work, only the studied variable changes.** For internal ablations
   (specialization ladder, lock-granularity), every rung runs the *identical*
   operation; only the one dimension under study differs. A tuned-to-win
   workload is worthless.

2. **Match read ownership.** WorkTable returns an **owned** row. Do not compare
   it against a baseline's **borrowed** read and call the gap a WorkTable cost.
   Either compare owned-vs-owned, or label the row as owned-vs-borrowed and say
   what the ownership buys.

3. **Match durability to the comparison.** Comparing against a durable store →
   WorkTable persist ON, peer in the matching tier (L3 non-sync, L4 default).
   Comparing against an in-memory structure → WorkTable persist OFF. Never mix.
   WorkTable has no fsync mode; never present it as L4-fsync-equivalent.

4. **Pair every peer number with a capability matrix.** No throughput bar for
   sled/redb/LMDB/SQLite ships without the adjacent table of what each system
   provides (durability, process model, transactions, SQL, index kinds).

5. **Report the losses.** Where WorkTable is slower (raw ops vs. arena/floor),
   show it. The symmetry is the credibility: we look too-good vs. jank for the
   same reason we look too-slow vs. `Vec`.

6. **One machine per comparison.** Throughput ratios are only comparable within
   a single run on one machine. Cross-machine numbers are never compared
   directly. Every result carries its environment manifest (machine, OS, arch,
   toolchain, WorkTable commit).

7. **No silent caps.** If a run bounds coverage (top-N threads, a sampled
   subset, a scan cap for O(n) baselines), the number states the bound.

8. **Reproducible.** Deterministic seeds, pre-generated workloads outside the
   timed region, fixed-process-per-cell where isolation matters, published
   commit and lockfile.

---

## 4. How to read the headline comparisons

- **"WorkTable is ~Nx slower than `Vec`"** means: a real typed/indexed/owned
  engine costs a small constant over a raw positional array. It does **not**
  mean WorkTable is slow; `Vec` is not an alternative for any workload needing
  an index, concurrency, or persistence.

- **"WorkTable is ~Nx faster than the hand-rolled scan"** means: on the
  operations that dominate real code without an engine (find-by-field, ranged
  reads, typed access over serialized data), the generated index and archive
  win by orders of magnitude. This is the comparison that reflects the choice a
  developer actually faces.

- **"WorkTable vs. SQLite/redb/LMDB"** is only shown for the operations and
  durability tier where the capability matrix says the systems overlap, and it
  ships with that matrix.

The reader should never have to guess which world a number lives in.
