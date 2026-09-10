#!/bin/zsh
# Sweep a benchmark over concurrency and collect structured rows.
#
# Every dimension comes back as a field, so nothing downstream parses columns.
# Reading a column index out of a formatted table is how a comparison in this
# study ended up putting one arm's "upserts" against another's "total" and
# inverting the result.
#
#   scripts/grid.sh page-size out.jsonl "1 1" "6 2" "12 4" "24 8"
#
# Environment passed through: WT_ROUNDS_N, WT_RUNTIME_WORKERS, WT_OPS,
# WT_DEFAULT_RUNTIME and the tuning knobs.
set -u
BIN_DIR=${BIN_DIR:-$(cd "$(dirname "$0")/.." && pwd)/target/release}
BENCH=$1; OUT=$2; shift 2
: > "$OUT"
for rw in "$@"; do
  R=${rw%% *}; W=${rw##* }
  echo "  ${BENCH}: ${R} readers / ${W} writers" >&2
  WT_JSON=1 WT_READERS=$R WT_WRITERS=$W "$BIN_DIR/$BENCH-worktable" >> "$OUT" 2>/dev/null
done
echo "  wrote $(wc -l < "$OUT" | tr -d ' ') rows to $OUT" >&2
