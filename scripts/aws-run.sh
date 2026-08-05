#!/usr/bin/env bash
# Provision + run the full wt-benchmarks suite on a fresh AWS box.
# Idempotent: safe to re-run. Results land in results/aws-<host>-<utc>/ tagged
# by hostname so 3 servers never collide.
#
#   bash scripts/aws-run.sh              # paper scale (criterion defaults)
#   FAST=1 bash scripts/aws-run.sh       # smoke: tiny windows, ~5 min, proves it runs
#
# Assumes: Ubuntu/Debian (apt) or Amazon Linux (yum). Run from anywhere.
set -euo pipefail

WT_BENCH_REPO="https://github.com/pathscale/wt-benchmarks"
WT_REPO="https://github.com/pathscale/WorkTable"
WORKDIR="${WORKDIR:-$HOME/bench}"
HOST="$(hostname -s)"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"

log() { echo "[aws-run] $*"; }

# --- 1. system deps -------------------------------------------------------
if command -v apt-get >/dev/null; then
  sudo apt-get update -y
  sudo apt-get install -y build-essential pkg-config libssl-dev git curl clang
elif command -v yum >/dev/null; then
  sudo yum groupinstall -y "Development Tools"
  sudo yum install -y openssl-devel git curl clang
fi

# --- 2. rust --------------------------------------------------------------
if ! command -v cargo >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  . "$HOME/.cargo/env"
fi
. "$HOME/.cargo/env" 2>/dev/null || true

# --- 3. repos (bench + sibling WorkTable) ---------------------------------
mkdir -p "$WORKDIR"; cd "$WORKDIR"
[ -d wt-benchmarks ] && (cd wt-benchmarks && git pull --ff-only) || git clone "$WT_BENCH_REPO"
[ -d WorkTable ]     && (cd WorkTable && git pull --ff-only)     || git clone "$WT_REPO"
cd wt-benchmarks

# The bench pins `worktable = { path = "../WorkTable" }`; the sibling clone above
# satisfies it. Confirm it points at a beta.5+ WorkTable.
grep -q 'path = "../WorkTable"' Cargo.toml || { echo "expected ../WorkTable path dep"; exit 1; }

OUT="results/aws-${HOST}-${STAMP}"
mkdir -p "$OUT"
uname -a > "$OUT/environment.txt"
(cd ../WorkTable && git rev-parse HEAD) >> "$OUT/environment.txt"
git rev-parse HEAD >> "$OUT/environment.txt"

# --- 4. run ---------------------------------------------------------------
if [ "${FAST:-0}" = "1" ]; then
  ARGS="-- --warm-up-time 0.3 --measurement-time 0.6 --sample-size 10"  # smoke
else
  ARGS=""                                                               # paper defaults
fi

log "kv ..."
cargo bench --bench kv      --features external-adapters $ARGS > "$OUT/kv.log" 2>&1
log "kv_json ..."
cargo bench --bench kv_json --features external-adapters $ARGS > "$OUT/kvjson.log" 2>&1
log "ycsb ..."
cargo bench --bench ycsb                                 $ARGS > "$OUT/ycsb.log" 2>&1

# --- 5. collect criterion medians into one CSV ----------------------------
python3 - "$OUT" <<'PY' || true
import json, os, sys, glob, csv
out = sys.argv[1]
rows = []
for est in glob.glob("target/criterion/*/*/new/estimates.json"):
    parts = est.split("/")
    group, bench = parts[2], parts[3]
    ns = json.load(open(est))["median"]["point_estimate"]
    rows.append([group, bench, round(ns/1e6, 4)])
rows.sort()
with open(os.path.join(out, "medians.csv"), "w", newline="") as f:
    w = csv.writer(f); w.writerow(["group", "engine", "median_ms"]); w.writerows(rows)
print(f"wrote {len(rows)} rows to {out}/medians.csv")
PY

log "DONE -> $OUT"
