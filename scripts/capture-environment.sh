#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
worktable_dir="${WORKTABLE_DIR:-$repo_root/../WorkTable}"
benchmark_lockfile="${BENCHMARK_LOCKFILE:-$repo_root/Cargo.lock}"

tree_state() {
    local repository="$1"
    if [[ -n "$(git -C "$repository" status --porcelain 2>/dev/null)" ]]; then
        echo dirty
    else
        echo clean
    fi
}

echo "captured_at_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "suite_commit=$(git -C "$repo_root" rev-parse --verify HEAD 2>/dev/null || echo uncommitted)"
echo "suite_tree=$(tree_state "$repo_root")"
echo "worktable_dir=$worktable_dir"
echo "worktable_commit=$(git -C "$worktable_dir" rev-parse --verify HEAD 2>/dev/null || echo unknown)"
echo "worktable_tree=$(tree_state "$worktable_dir")"
if [[ -f "$benchmark_lockfile" ]]; then
    echo "benchmark_lockfile=$benchmark_lockfile"
    echo "benchmark_lock_hash=$(git hash-object "$benchmark_lockfile")"
fi
echo "target=$(rustc -vV | sed -n 's/^host: //p')"
rustc -vV
cargo -V
uname -a

if command -v lscpu >/dev/null 2>&1; then
    lscpu
fi
if command -v numactl >/dev/null 2>&1; then
    numactl --hardware
fi
if command -v free >/dev/null 2>&1; then
    free -h
fi
if command -v lsblk >/dev/null 2>&1; then
    lsblk -o NAME,MODEL,SIZE,ROTA,TYPE,MOUNTPOINTS
fi
if command -v sysctl >/dev/null 2>&1; then
    sysctl -n machdep.cpu.brand_string 2>/dev/null || true
    sysctl -n hw.physicalcpu 2>/dev/null || true
    sysctl -n hw.logicalcpu 2>/dev/null || true
    sysctl -n hw.memsize 2>/dev/null || true
fi
