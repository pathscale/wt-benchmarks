# §Eval narrative: the honest-baseline story (draft note, not paper prose)

C.B. wants this told as a *story* in the paper — it is the strongest
methodological beat we have, and it is exactly the register CIDR rewards.
Capturing it here so it survives IDE upgrade / context loss. Real prose gets
written into body.tex later; numbers below are throwaway M4-MacBook figures,
to be replaced by the AWS run.

## Framing spine: WorkTable straddles two worlds, and looks "unfair" in BOTH

This is the governing idea for the whole §Eval, per C.B. — not a footnote.
WorkTable sits between raw language collections and real embedded databases,
and every comparison will look unfair to *someone*. The eval's job is NOT to
win either comparison; it is to LOCATE WorkTable on the spectrum and name, in
both directions, what each neighbor pays that the other does not.

**Downward (vs raw `Vec`/`HashMap`): the gap looks unfair to WorkTable — and
that is the result.** No database benchmarks itself against a raw `Vec`,
because a `Vec` is not a database: it is the language's performance floor, with
no index, no concurrency, no owned result, no persistence. We push the `Vec`
comparison ON PURPOSE. The claim is not "we beat `Vec`" (impossible, by
construction) — it is "we come within a small constant of the floor *while
being a real engine*." Landing ~2x off `Vec` on point reads, not 1000x, is the
remarkable thing. The unfairness is the point: WorkTable pays almost nothing
for the capabilities `Vec` lacks entirely.

**Upward (vs sled / redb / LMDB / SQLite): the gap will look unfair to THEM —
and we say so, loudly.** On in-memory point operations WorkTable will beat
these engines, sometimes by a lot. That comparison is unfair to them, because
they carry crash durability, cross-process access, and (SQLite) SQL that
WorkTable does not offer. We refuse to let a fast in-memory number imply
semantic equivalence to a durable engine. Every peer comparison is paired with
a capability matrix and restricted to the in-memory / non-durable operations
where the comparison is apples-to-apples; where we run the durable tier, we
turn WorkTable's own persistence ON to match (see the L3/L4 comparison layers).

**The honesty is the symmetry.** We are explicit that we look too-good against
peers for the same reason we look too-bad against `Vec`: we occupy a point on
the spectrum that neither neighbor occupies. Naming the unfairness in both
directions is what makes the eval credible rather than cherry-picked — and it
is the CIDR-shaped move: a paper honest about where its own numbers flatter it.

## The arc (three beats)

**Beat 1 — the naive benchmark that would have killed our own system.**
We first benchmarked WorkTable against a raw `Vec<T>`. The numbers look
damning: point reads and field updates are orders of magnitude slower than
array indexing.

    raw Vec point read (borrowed, positional):  ~689  Mops/s
    WorkTable point read (owned, indexed):       ~1.5 Mops/s
    => ~200x "slower"

If we had stopped there, WorkTable would never have been built. But raw `Vec`
is not a peer: it is a positional array handing back a borrow. It has no
secondary index, no concurrency, no owned result, no typed access over
serialized data. Comparing it to an indexed, concurrent, owned-read engine is
a pointer dereference versus a database lookup. The 200x is an artifact of the
mismatch, not a measurement of cost.

**Beat 2 — why we did not go hunting for a flattering synthetic benchmark.**
The tempting move is to construct a microbenchmark tuned until WorkTable wins.
We refused: a synthetic workload shaped to prove a point is the thing a
reviewer discounts on sight, and we did not need it. We already knew where the
value was, because we had lived it in production.

**Beat 3 — we turned inward, to real code we had never optimized.**
Instead we looked at our own production systems (PathScale / the trading
backend) for code with the characteristics of a *good WorkTable candidate*:
tabular data held in a `Vec`, looked up by a non-primary key, with typed
access over a serialized payload. We found it, untouched, in the wild:

    web3.trading-backend/src/handlers/s3/sub_s3_execution.rs:140,204
        slippage_rows.iter().find(|s| s.event_id == close_id)
    // O(n) linear scan to find one row by a non-primary key. No index.

    deribit/.../rest/order.rs, AgencyZero/crates/wt-tools/src/lib.rs
        serde_json::from_value(result_value.clone())
        serde_json::from_str(raw)...
    // typed access = a JSON parse on every touch.

This is what an application actually writes *before* adopting an engine: a
`Vec`, no index, JSON at the typing boundary. It is precisely what WorkTable
replaces — a generated B-tree index (no scan) and a typed archive (no
per-access parse). Against *this* baseline — the honest one — the story
inverts:

    operation            janky Vec (scan+JSON)   WorkTable      delta
    secondary lookup     5,243   ops/s           1,015,842/s    ~194x FASTER
    field update         5,618   ops/s           497,000/s      ~88x  FASTER
    point read (typed)   2.57 Mops/s             1.34 Mops/s    ~2x slower

WorkTable trades a modest constant factor on trivial point operations for
one-to-two orders of magnitude on the operations that dominate real
hand-rolled tabular code: unindexed lookups and typed updates over serialized
data. That is the "no silent scans / ORM readability without the O(n)
surprise" claim of the paper, measured.

## The three baseline tiers (all in this campaign, same machine)

| tier | binary | what it is | role |
|---|---|---|---|
| raw floor | `baselines` | `Vec` / `HashMap` / `DashMap`, borrowed, positional | the naive "200x" — Beat 1 |
| fair floor | `vec_realistic` | `Vec` + hand secondary index + `RwLock` + owned reads | disciplined hand-roll: ~2-12x |
| real code | `vec_janky` | `Vec::iter().find()` scan + embedded-JSON per access | what apps ACTUALLY write — Beat 3 |
| engine | `ablation` (specialized) | WorkTable | the system |

The paper should show the raw floor AND the janky real baseline, and *narrate
the gap between them* — that gap is the paper's honesty and its punchline.

## TODO before this is paper-ready
- Re-run all four on the AWS box; replace every number above.
- Fix vec_realistic `update_field` (currently a no-real-work ~324 Mops/s number;
  take the lock per op / do comparable work).
- Confirm the two production line-refs are quotable (anonymize if needed).
- Decide table vs prose: C.B. wants prose/story, likely one small table + a
  narrated paragraph in §Eval.
