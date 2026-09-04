#!/bin/sh
# Compare one local WorkTable checkout with the local beta18 checkout.
# Registry packages are never substituted for either side.
#
# This is the axis the moe_pgo suite exists for. Backends are a choice made
# once; a local build that is slower than a historical local checkout is a
# regression, and it will not show up in a run that only builds one of them.
#
# Both sides share a CRITERION_HOME, so the second run compares against the
# first through Criterion's own baseline machinery rather than by eye.
#
# Usage:
#   scripts/compare-worktable-versions.sh LABEL WORKTABLE DATABUCKET WTI
#
# POSIX sh: no arrays, no [[ ]], no pipefail.
set -eu

LABEL="${1:-}"
WORKTABLE_PATH="${2:-}"
DATABUCKET_PATH="${3:-}"
WTI_PATH="${4:-}"

if [ -z "$LABEL" ] || [ -z "$WORKTABLE_PATH" ] || [ -z "$DATABUCKET_PATH" ] || [ -z "$WTI_PATH" ]; then
    echo "usage: $0 <label> <local-WorkTable> <local-DataBucket> <local-WorkTablesIndex>" >&2
    exit 2
fi
for path in "$WORKTABLE_PATH" "$DATABUCKET_PATH" "$WTI_PATH"; do
    if [ ! -f "$path/Cargo.toml" ]; then
        echo "not a local crate checkout: $path" >&2
        exit 2
    fi
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
export CRITERION_HOME="$ROOT/target/criterion-compare"
mkdir -p "$CRITERION_HOME"
MANIFEST="$ROOT/tools/version-grid/Cargo.toml"
FILTER='12288|fixed_work'

echo "=== local $LABEL: $WORKTABLE_PATH ==="
(
    cd "$ROOT"
    cargo --offline \
        --config "patch.crates-io.worktable.path='$WORKTABLE_PATH'" \
        --config "patch.crates-io.data_bucket.path='$DATABUCKET_PATH'" \
        --config "patch.crates-io.WorkTablesIndex.path='$WTI_PATH'" \
        bench --manifest-path "$MANIFEST" --bench moe_pgo \
        --features historical-grid -- "$FILTER" --save-baseline "$LABEL"
)

echo
echo "=== local beta18: $ROOT/../WorkTable ==="
(
    cd "$ROOT"
    cargo --offline bench --manifest-path "$MANIFEST" --bench moe_pgo \
        -- "$FILTER" --baseline "$LABEL"
)

echo
echo "Read moe_pgo/control first. It contains no WorkTable, so if it moved"
echo "between the two runs the machine moved and the comparison is void."
