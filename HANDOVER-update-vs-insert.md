# Handover: WorkTable single-column update is ~4x slower than a fresh insert

## The finding (benchmark-derived, reproducible, arch-independent)
On the kv_json typed table (`{id, name, email, age, balance, active}`, 10k rows),
a single-column `update` (`update_balance`, one column) costs **3.6–4.7x** a
fresh `insert` of the whole row. Measured on 4 AWS boxes, size-10, all 3 index
backends — the ratio is stable everywhere, so it's a real WT update-path cost,
NOT a benchmark artifact or arch quirk.

| arch (instance)        | WTI insert→update | ratio | Congee | Arctic |
|------------------------|-------------------|-------|--------|--------|
| Intel (r8i.16xlarge)   | 12.6 → 52.2 ms    | 4.16x | 4.69x  | 4.65x  |
| AMD   (m8i.16xlarge)   | 12.6 → 52.8 ms    | 4.21x | 4.58x  | 4.66x  |
| ARM   (r8g.16xlarge)   | 18.1 → 69.7 ms    | 3.85x | 4.43x  | 4.51x  |
| Budget (t4g.2xlarge)   | 27.3 → 99.0 ms    | 3.62x | 4.38x  | 4.56x  |

Raw criterion data: `results/aws-run-20260805/{intel,amd,arm,budget}/crit.tgz`.

## Hypothesis
Delete + insert would cost LESS than an in-place single-field update. The update
path (`update_<col>`) is awaited per-op and does a `LockGuard` acquire +
`PublishedRow::replace` per call; insert is cheaper per op. Distinct from the
overwrite-reinsert bug already fixed in beta.5 — that was correctness, this is
the *normal* update path being slow.

## Reproducing test case (drop in WorkTable/tests/worktable/)
Times N inserts (baseline) vs N single-column updates on the same typed table.
The `assert ratio < 4.5` fails today (~4x) → live regression guard; tighten as
the path improves. Add a delete+insert block to confirm the hypothesis.

```rust
use std::time::Instant;
use worktable::prelude::*;
use worktable::worktable;

worktable!(
    name: Acct, persist: false,
    columns: { id: u64 primary_key, name: String, email: String, age: u32, balance: f64, active: bool },
    queries: { update: { Balance(balance) by id, } }
);

fn row(k: u64) -> AcctRow {
    AcctRow { id: k, name: format!("user-{k:08}"), email: format!("user{k}@example.test"),
              age: 18 + (k % 60) as u32, balance: k as f64 * 1.5, active: k % 2 == 0 }
}

#[tokio::test]
async fn update_field_vs_fresh_insert_cost() {
    const N: u64 = 10_000;

    let t_ins = AcctWorkTable::default();
    let s = Instant::now();
    for k in 0..N { t_ins.insert(row(k)).unwrap(); }
    let insert_ns = s.elapsed().as_nanos();

    let t_upd = AcctWorkTable::default();
    for k in 0..N { t_upd.insert(row(k)).unwrap(); }
    let s = Instant::now();
    for k in 0..N {
        t_upd.update_balance(BalanceQuery { balance: k as f64 * 2.25 }, k).await.unwrap();
    }
    let update_ns = s.elapsed().as_nanos();

    // OPTIONAL third block — confirm the hypothesis (delete+insert cheaper?):
    // let t_di = AcctWorkTable::default();
    // for k in 0..N { t_di.insert(row(k)).unwrap(); }
    // let s = Instant::now();
    // for k in 0..N { t_di.delete(k).await.unwrap(); let mut r = row(k); r.balance = k as f64*2.25; t_di.insert(r).unwrap(); }
    // let di_ns = s.elapsed().as_nanos();

    let ratio = update_ns as f64 / insert_ns as f64;
    eprintln!("insert {:.2}ms | update {:.2}ms | update/insert = {:.2}x",
              insert_ns as f64/1e6, update_ns as f64/1e6, ratio);
    assert!(ratio < 4.5, "single-column update is {ratio:.2}x a fresh insert — update path regressed");
}
```

## Profiling pointers
`cargo test --release ... -- --nocapture`, then perf/samply the update loop.
Expect hotspots in per-op `LockGuard` acquire + `PublishedRow::replace`. Likely
wins: batch the publish, or cut per-op async/lock overhead.

## Timing / cost
Wall-clock for a full size-10 run (kv + kv_json + ycsb) is ~12-14 min, dominated
by YCSB (~14s/estimate) and build — NOT sample count. Size-25 costs nearly the
same; the insert/update repro above is seconds once built.
