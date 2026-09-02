#!/bin/sh
# Run one benchmark against a published WorkTable and against the local
# checkout, and compare them.
#
# This is the axis the moe_pgo suite exists for. Backends are a choice made
# once; a local build that is slower than the published crate is a regression,
# and it will not show up in a run that only ever builds one of them.
#
# Both sides share a CRITERION_HOME, so the second run compares against the
# first through Criterion's own baseline machinery rather than by eye.
#
# Usage:
#   scripts/compare-worktable-versions.sh 1.0.0-beta.16
#   scripts/compare-worktable-versions.sh 1.0.0-beta.16 moe_pgo
#
# POSIX sh: no arrays, no [[ ]], no pipefail.
set -eu

VERSION="${1:-}"
BENCH="${2:-moe_pgo}"

if [ -z "$VERSION" ]; then
    echo "usage: $0 <published-worktable-version> [bench-name]" >&2
    echo "example: $0 1.0.0-beta.16 moe_pgo" >&2
    exit 2
fi

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
export CRITERION_HOME="$ROOT/target/criterion-compare"
mkdir -p "$CRITERION_HOME"

cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

echo "=== published worktable $VERSION ==="
# A copy, so the published build never touches the working tree. The path
# dependency is rewritten to a registry one; everything else is identical, so
# the only difference between the two runs is WorkTable itself.
mkdir -p "$WORK/published"
# Only what the build needs. Copying the whole root drags `target` along, which
# is 27 GB here and pointless: the published side rebuilds from scratch anyway.
for item in Cargo.toml src benches; do
    cp -R "$ROOT/$item" "$WORK/published/"
done
sed -i.bak -E "s|^worktable = \{ path = \"[^\"]*\"(.*)\}|worktable = { version = \"=$VERSION\"\1}|" \
    "$WORK/published/Cargo.toml"
rm -f "$WORK/published/Cargo.toml.bak"

if ! grep -q "version = \"=$VERSION\"" "$WORK/published/Cargo.toml"; then
    echo "could not rewrite the worktable dependency; check Cargo.toml's format" >&2
    exit 1
fi

( cd "$WORK/published" && cargo bench --bench "$BENCH" -- --save-baseline published )

echo
echo "=== local checkout ==="
( cd "$ROOT" && cargo bench --bench "$BENCH" -- --baseline published )

echo
echo "Read moe_pgo/control first. It contains no WorkTable, so if it moved"
echo "between the two runs the machine moved and the comparison is void."
