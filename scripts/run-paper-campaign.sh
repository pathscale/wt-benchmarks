#!/usr/bin/env bash
# run-paper-campaign.sh — one command to produce every paper number.
#
# Runs all four campaigns at publication scale, timed and manifest-tagged, into
# results/paper-<stamp>/. Designed to be started on a quiet dedicated box (AWS
# Linux ARM64 for the paper) and left alone. Estimated wall time at the default
# PROFILE=paper scale: ~1-2h on a fast box; minutes at PROFILE=smoke.
#
# Usage:
#   scripts/run-paper-campaign.sh            # PROFILE=paper (full scale)
#   PROFILE=smoke scripts/run-paper-campaign.sh   # tiny, ~3 min, validates wiring
#
# Everything here is throwaway on a laptop; the load-bearing run is the AWS one,
# whose environment manifests record the machine + WorkTable commit per campaign.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
PROFILE="${PROFILE:-paper}"
outdir="$repo/results/paper-$stamp"
mkdir -p "$outdir"

if [[ "$PROFILE" == "smoke" ]]; then
  ROWS=100000; REPS=1; THREADS="1 2 4"; YCSB_RECORDS=50000; YCSB_OPS=200000
  CONTENTION_OPS=30000; TORTURE_RECORDS=1500; TORTURE_OPS=15000
elif [[ "$PROFILE" == "macbook" ]]; then
  # Fast reduced-scale full pipeline for an Apple-silicon laptop, tuned to finish
  # in ~5-10 min. Enough signal to see scaling shape and ratios; NOT the
  # load-bearing AWS paper run. Bump YCSB_* / REPS for tighter numbers.
  ROWS=200000; REPS=3; THREADS="1 2 4 8"; YCSB_RECORDS=200000; YCSB_OPS=400000
  CONTENTION_OPS=200000; TORTURE_RECORDS=4000; TORTURE_OPS=40000
else # paper
  ROWS=10000000; REPS=10; THREADS="1 2 4 8 16 32"; YCSB_RECORDS=1000000; YCSB_OPS=5000000
  CONTENTION_OPS=1000000; TORTURE_RECORDS=10000; TORTURE_OPS=200000
fi

log() { printf '\n=== %s ===\n' "$1"; }
secs() { python3 -c "import time;print(time.time())"; }
t_start=$(secs)

# ---- 1. baseline ladder (Table 1/2: floor, slotmap, janky, vec+idx, WorkTable) ----
log "baseline ladder (ROWS=$ROWS REPS=$REPS)"
cd "$repo/campaigns/contention"
cargo build --release --bins >/dev/null 2>&1
ladder="$outdir/ladder.csv"; echo "tier,engine,op,ops_per_sec" > "$ladder"
export ROWS REPS SCAN_LOOKUPS=3000 RANGE_WIDTH=100 RANGE_LOOKUPS=3000
for bin in baselines slotmap_baseline vec_janky vec_realistic; do
  ./target/release/$bin 2>/dev/null | tail -n +2 >> "$ladder"
done
# Emit BOTH the specialized and dynamic-twin rows: the paper's specialized-vs-
# runtime-schema throughput ratio is derived from the two. Drop only the
# binary's own header line.
./target/release/ablation 2>/dev/null | grep -E '^ablation,(specialized|dynamic),' >> "$ladder"
echo "  -> $ladder ($(wc -l <"$ladder") rows)"

# ---- 2. contention matrix (C2: lock-granularity, publication on/off) ----
log "contention matrix (THREADS=$THREADS)"
THREADS="$THREADS" MODES="field_granular overlap whole_row single_mutex" \
  PUBLICATION="off on" REPS="$REPS" OPS_PER_THREAD="$CONTENTION_OPS" WARMUP_OPS=20000 \
  RESULTS_DIR="$outdir" \
  bash scripts/run-contention-matrix.sh "contention" >/dev/null
echo "  -> $outdir/contention.jsonl"

# ---- 3. YCSB A-F scaling, publication ON (concurrency throughput) ----
log "YCSB scaling sweep (versioned, THREADS=$THREADS)"
MODE=versioned THREADS="$THREADS" RECORDS="$YCSB_RECORDS" OPERATIONS="$YCSB_OPS" \
  REPETITIONS="$REPS" CAMPAIGN_RESULTS="$outdir/ycsb-versioned.jsonl" \
  bash "$repo/scripts/run-ycsb-matrix.sh" >/dev/null
echo "  -> $outdir/ycsb-versioned.jsonl"

# ---- 4. torture / correctness sweep (all backends, v1 gate) ----
log "torture sweep (all backends, THREADS=$THREADS)"
CAMPAIGN_BACKENDS="worktables_index congee arctic" CAMPAIGN_THREADS="$THREADS" \
  CAMPAIGN_RECORDS="$TORTURE_RECORDS" CAMPAIGN_OPERATIONS="$TORTURE_OPS" \
  CAMPAIGN_REPETITIONS="$REPS" \
  CAMPAIGN_RESULTS="$outdir/shadow-concurrency.jsonl" \
  CAMPAIGN_ENVIRONMENT="$outdir/shadow-concurrency.environment.txt" \
  bash "$repo/campaigns/ycsb-index-backends/run-shadow-matrix.sh" >/dev/null
echo "  -> $outdir/shadow-concurrency.jsonl"

t_end=$(secs)
elapsed=$(python3 -c "print(f'{($t_end-$t_start)/60:.1f}')")

# ---- correctness gate: every torture + contention cell must have passed ----
python3 - "$outdir" <<'PY'
import json, sys, glob, os
outdir=sys.argv[1]; bad=0
for f in glob.glob(os.path.join(outdir,'*.jsonl')):
    for line in open(f):
        try: r=json.loads(line)
        except: continue
        if 'passed' in r and not r['passed']:
            bad+=1; print(f"  FAILED cell in {os.path.basename(f)}: {r.get('lock_mode') or r.get('backend')} threads={r.get('threads')}")
print(f"correctness gate: {'ALL PASSED' if bad==0 else f'{bad} cells FAILED'}")
PY

echo
echo "PROFILE=$PROFILE  wall time=${elapsed} min  results -> $outdir"
