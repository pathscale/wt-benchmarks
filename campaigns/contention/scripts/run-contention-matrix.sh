#!/usr/bin/env bash
# run-contention-matrix.sh — paper C2 lock-granularity contention sweep.
#
# Runs contention-paper once per cell (fresh process), sweeping:
#   lock mode      x thread count x publication on/off x repetition
# and appends one JSONL record per cell to a results file, alongside a
# sidecar environment manifest (machine, commits, toolchain).
#
# Fresh-process-per-cell is deliberate: it prevents allocator state, warmed
# pages, and tokio worker threads from one cell leaking into the next, which
# is the reproducibility discipline the paper numbers require.
#
# Usage:
#   scripts/run-contention-matrix.sh [OUT_PREFIX]
# Env overrides:
#   THREADS="1 2 4 8 16 32"   MODES="field_granular overlap whole_row single_mutex"
#   PUBLICATION="off on"      REPS=5   OPS_PER_THREAD=200000   WARMUP_OPS=20000
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_PREFIX="${1:-contention-${STAMP}}"
RESULTS_DIR="${RESULTS_DIR:-$HERE/../../results}"
mkdir -p "$RESULTS_DIR"
OUT="$RESULTS_DIR/${OUT_PREFIX}.jsonl"
ENV_OUT="$RESULTS_DIR/${OUT_PREFIX}.environment.txt"

THREADS="${THREADS:-1 2 4 8 16 32}"
MODES="${MODES:-field_granular overlap whole_row single_mutex}"
PUBLICATION="${PUBLICATION:-off on}"
REPS="${REPS:-5}"
OPS_PER_THREAD="${OPS_PER_THREAD:-200000}"
WARMUP_OPS="${WARMUP_OPS:-20000}"

# ---- environment manifest (mirrors the other campaigns' sidecar) ----
{
  echo "captured_at_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "campaign=contention"
  echo "suite_commit=$(git -C "$HERE" rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "suite_tree=$(git -C "$HERE" diff --quiet 2>/dev/null && echo clean || echo dirty)"
  WT_DIR="$(cd "$HERE/../../../WorkTable" && pwd)"
  echo "worktable_dir=$WT_DIR"
  echo "worktable_commit=$(git -C "$WT_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "worktable_tree=$(git -C "$WT_DIR" diff --quiet 2>/dev/null && echo clean || echo dirty)"
  echo "threads=$THREADS"
  echo "modes=$MODES"
  echo "publication=$PUBLICATION"
  echo "reps=$REPS"
  echo "ops_per_thread=$OPS_PER_THREAD"
  echo "warmup_ops=$WARMUP_OPS"
  echo "target=$(rustc -vV | awk '/host:/{print $2}')"
  rustc --version
  cargo --version
  uname -a
  if [[ "$(uname)" == "Darwin" ]]; then
    sysctl -n machdep.cpu.brand_string 2>/dev/null || true
    echo "logical_cpus=$(sysctl -n hw.logicalcpu 2>/dev/null || echo '?')"
  else
    grep -m1 'model name' /proc/cpuinfo 2>/dev/null || true
    echo "logical_cpus=$(nproc 2>/dev/null || echo '?')"
  fi
} >"$ENV_OUT"
echo "environment -> $ENV_OUT"

# ---- build each requested feature config once, to a stable per-config path ----
# (No associative arrays: macOS ships bash 3.2, which lacks `declare -A`.)
bin_for_pub() {  # echo the binary path for pub=on|off
  if [[ "$1" == "on" ]]; then echo "$HERE/target/release/contention-paper-pub-on"
  else echo "$HERE/target/release/contention-paper-pub-off"; fi
}
for pub in $PUBLICATION; do
  if [[ "$pub" == "on" ]]; then
    echo "building contention-paper (versioned-row-publication ON)..."
    cargo build --release --bin contention-paper --features versioned-row-publication
  else
    echo "building contention-paper (default: publication OFF)..."
    cargo build --release --bin contention-paper
  fi
  # cargo overwrites the same target path across configs; snapshot per-config
  cp "$HERE/target/release/contention-paper" "$(bin_for_pub "$pub")"
done

: >"$OUT"
cells=0
for pub in $PUBLICATION; do
  bin="$(bin_for_pub "$pub")"
  for mode in $MODES; do
    for t in $THREADS; do
      for r in $(seq 1 "$REPS"); do
        "$bin" --mode "$mode" --threads "$t" \
          --ops-per-thread "$OPS_PER_THREAD" --warmup-ops "$WARMUP_OPS" \
          --repetition "$r" >>"$OUT"
        cells=$((cells + 1))
      done
      echo "  done: pub=$pub mode=$mode threads=$t (${REPS} reps)"
    done
  done
done

echo "wrote $cells cells -> $OUT"
# quick sanity: flag any cell that lost updates
if command -v python3 >/dev/null; then
  python3 - "$OUT" <<'PY'
import json, sys
bad = 0
for line in open(sys.argv[1]):
    r = json.loads(line)
    if not r.get("passed", False):
        bad += 1
        print(f"  FAILED cell: mode={r['lock_mode']} threads={r['threads']} "
              f"pub={r['feature_versioned_row_publication']} "
              f"lost_updates={r['lost_updates']} errors={r['errors']}")
print(f"correctness: {'ALL PASSED' if bad == 0 else f'{bad} cells FAILED'}")
PY
fi
