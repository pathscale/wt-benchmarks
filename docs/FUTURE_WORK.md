# Future benchmark work

Deferred comparisons that strengthen the story but are not built yet. Each is
scoped so it can be picked up cleanly. All must obey `FAIRNESS.md`.

---

## 1. OSS-extracted Vec baseline (the "in the wild" tier)

**Goal.** Complete the Vec comparison ladder with a baseline that cannot be
dismissed as a strawman we wrote: a real scan-by-field pattern pulled from a
public, permissively-licensed repository and extracted faithfully into a
benchmark, with the source cited in-code.

**Why.** We already have raw `Vec` (floor), hand-tuned `Vec`+index (`vec_realistic`),
an ecosystem arena (`slotmap`), and a modeled janky pattern (`vec_janky`). The
missing tier is code we did not author. GitHub code search shows the
`Vec` + `iter().find(|x| x.field == key)` scan-by-field pattern is pervasive
(30k+ hits). Strong, recognizable, permissive candidates:
`EmbarkStudios/krates`, `oxidecomputer/sshauth`, `vaultwarden`.

**Approach.** Pick one repo, extract its actual scan-by-field access into
`campaigns/contention/src/bin/vec_oss.rs`, keep a comment citing the exact file
and commit, and run the same op set as the other Vec tiers so it lines up in the
ladder table. Expect WorkTable to win by 1-2 orders of magnitude on
find-by-field, same as `vec_janky`, but now un-strawmannable.

**Status.** Not built. Candidate identified; extraction pending.

---

## 2. SaaS application vs. Postgres (the honest RDBMS comparison)

**Goal.** Answer "how much is Postgres actually costing you?" at the
*application* level, not with an unfair op microbenchmark. Same SaaS app, two
data layers: `api.support.cafe` on WorkTable+S3 vs. the same app on Postgres.
Both achieve the same thing; the models differ; the delta is the cost of the
Postgres model for this workload.

**Vehicle.** `api.support.cafe` (real, open-source, already WorkTable-based,
portable). Pull it into wt-benchmarks as an external dependency to extract its
schema and real query mix.

**Approach (recommended: Option B, extract the workload).** Do NOT port the
whole HTTP app: WorkTable is baked into the `db/` layer with no storage-abstraction
trait, so a full port is days of app engineering. Instead, extract the app's
actual operation mix (create session, append support message, fetch conversation,
list by member, config reads, etc.) and replay that exact mix against WorkTable+S3
and Postgres as a benchmark harness. Same operations, same data shapes, same
access patterns; reproducible without standing up the web server. The published
claim is identical: "the SaaS workload of api.support.cafe, WT+S3 vs Postgres."

**Open questions before building.**
- Postgres available / OK to install on the target box?
- Durability tier per `FAIRNESS.md`: WT+local-disk vs Postgres-local (pure
  engine cost), or WT+S3 vs Postgres+WAL (full durable stack)? These are
  different fairness tiers and must be labeled.

**Framing caveat.** This is the RDBMS comparison from `FAIRNESS.md`: never
headline it as a WorkTable "win." Frame it as the cost of the guarantees
Postgres provides (network, MVCC, ACID, crash recovery, multi-client) that this
embedded workload does not use.

**Status.** Deferred. Not today.

---

## 3. TodoMVC-style "same app, swap the store" framing

**Idea.** The credibility model to aim for is TodoMVC: one canonical, fixed
application spec, implemented N ways, compared apples-to-apples because the app
is held constant and only the implementation varies. Applied here: a single
fixed reference app (or the RealWorld/Conduit spec, its larger CRUD sibling)
implemented once per data layer (WorkTable, SQLite, Postgres, a KV store), so
"here is the cost of each backend for the same application" is a recognized,
un-cherry-pickable comparison rather than a bespoke microbenchmark.

**Relation to item 2.** Item 2 is the concrete first instance of this idea
using our own real app (`api.support.cafe`). The TodoMVC framing is the general
principle: if we want the comparison to read as standard rather than
self-serving, anchor it to a fixed, externally-recognized app spec and vary only
the store.

**Status.** Framing note. If pursued, RealWorld/Conduit is the actionable
external spec (already implemented across many backends including Postgres, so a
WorkTable backend would slot into an established comparison).
