#!/bin/zsh
# Pivot grid rows into a table, in whichever format the reader is.
#
#   scripts/pivot.sh out.jsonl page_size runtime            # md, the default
#   WT_FORMAT=toon scripts/pivot.sh out.jsonl readers runtime
#   WT_FORMAT=tsv  scripts/pivot.sh out.jsonl page_size
#
# Columns are selected by name and never by position. Reading a column index
# out of formatted text is how a comparison in this study ended up putting one
# arm's "upserts" against another's "total".
python3 -c '
import json, os, sys, statistics
from collections import defaultdict

path = sys.argv[1]
dims = sys.argv[2:] or ["page_size", "runtime"]
fmt = os.environ.get("WT_FORMAT", "md").lower()

rows = [json.loads(line) for line in open(path) if line.strip().startswith("{")]
if not rows:
    sys.exit("no rows in " + path)

cells = defaultdict(list)
for r in rows:
    cells[tuple(str(r.get(d)) for d in dims)].append(r)

def med(values, key):
    picked = [v for v in (x.get(key) for x in values) if v is not None]
    return statistics.median(picked) if picked else None

def lat(values, side, pct):
    picked = [x[side].get(pct) for x in values if x.get(side) and x[side].get(pct) is not None]
    return statistics.median(picked) if picked else None

metrics = [
    ("ops_per_second", lambda v: med(v, "ops_per_second")),
    ("cpu_x",          lambda v: med(v, "cpu_x")),
    ("rd_p50_ns",      lambda v: lat(v, "read_latency", "p50_ns")),
    ("rd_p99_ns",      lambda v: lat(v, "read_latency", "p99_ns")),
    ("wr_p50_ns",      lambda v: lat(v, "write_latency", "p50_ns")),
    ("wr_p99_ns",      lambda v: lat(v, "write_latency", "p99_ns")),
    ("runs",           lambda v: len(v)),
]
header = dims + [name for name, _ in metrics]

def sort_key(combo):
    out = []
    for value in combo:
        try:
            out.append((0, float(value)))
        except ValueError:
            out.append((1, value))
    return out

table = []
for combo in sorted(cells, key=sort_key):
    values = cells[combo]
    row = list(combo)
    for name, fn in metrics:
        computed = fn(values)
        if computed is None:
            row.append("")
        elif name == "cpu_x":
            row.append(f"{computed:.2f}")
        elif name == "runs":
            row.append(str(computed))
        else:
            row.append(f"{computed:.0f}")
    table.append(row)

if fmt == "toon":
    # Token-Oriented Object Notation: one header naming the fields, then bare
    # rows. Roughly half the bytes of the equivalent markdown, which is the
    # point when the reader is a model rather than a person.
    fields = ",".join(header)
    print("grid[" + str(len(table)) + "]{" + fields + "}:")
    for row in table:
        print("  " + ",".join(row))
elif fmt == "tsv":
    print("\t".join(header))
    for row in table:
        print("\t".join(row))
elif fmt == "md":
    def cell(value, name):
        # Thousands separators for the wide numbers only: they help a person
        # and cost a model nothing here because md is the human format.
        if name in ("ops_per_second", "rd_p50_ns", "rd_p99_ns", "wr_p50_ns", "wr_p99_ns") and value:
            return f"{int(value):,}"
        return value
    print("| " + " | ".join(header) + " |")
    print("|" + "|".join("---" for _ in header) + "|")
    for row in table:
        print("| " + " | ".join(cell(v, h) for v, h in zip(row, header)) + " |")
else:
    sys.exit(f"WT_FORMAT={fmt} is not a format: expected md, toon or tsv")
' "$@"
