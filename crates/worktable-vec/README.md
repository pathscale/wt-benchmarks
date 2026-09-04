# WorkTable-vec

Explicit non-concurrent `Vec`-backed controls for WorkTable benchmarks:

- `LinearTable`: ordered rows with a linear point lookup;
- `IndexedTable`: identical rows plus a `BTreeMap` row-offset index;
- `ArcticTable`: identical rows plus an Arctic row-offset index.

This crate is deliberately not a persistence engine. It isolates the
application-level baseline so the generated WorkTable cost is measured against
the same logical rows without leaving bespoke table code in the consumer.
