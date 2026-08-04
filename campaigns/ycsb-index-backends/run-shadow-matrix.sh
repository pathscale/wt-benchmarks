#!/usr/bin/env bash
set -euo pipefail

campaign_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
campaign_repo="$(cd "$campaign_dir/../.." && pwd)"
campaign_manifest="$campaign_dir/Cargo.toml"
campaign_mode="specialized"
campaign_backends="${CAMPAIGN_BACKENDS:-worktables_index congee arctic}"
campaign_threads="${CAMPAIGN_THREADS:-1 2 4 8 16 32}"
campaign_records="${CAMPAIGN_RECORDS:-2000}"
campaign_operations="${CAMPAIGN_OPERATIONS:-50000}"
campaign_repetitions="${CAMPAIGN_REPETITIONS:-5}"
campaign_payload_bytes="${CAMPAIGN_PAYLOAD_BYTES:-1024}"
campaign_buckets="${CAMPAIGN_BUCKETS:-32}"
campaign_vacuum="${CAMPAIGN_VACUUM:-true}"
campaign_delete_every="${CAMPAIGN_DELETE_EVERY:-7}"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
campaign_results="${CAMPAIGN_RESULTS:-$campaign_repo/results/shadow-concurrency-${stamp}.jsonl}"
campaign_environment="${CAMPAIGN_ENVIRONMENT:-${campaign_results%.jsonl}.environment.txt}"

mkdir -p "$campaign_repo/results"
if [[ -e "$campaign_results" ]]; then
    echo "error: refusing to overwrite existing results: $campaign_results" >&2
    exit 2
fi
if [[ -e "$campaign_environment" ]]; then
    echo "error: refusing to overwrite existing environment capture: $campaign_environment" >&2
    exit 2
fi

# WORKTABLE_DIR defaults to ../WorkTable inside capture-environment.sh, which is
# the live clone the campaign now builds against; the previous override pointed
# at a WorkTable-index-backends-versioned worktree that no longer exists.
WORKTABLE_DIR="${WORKTABLE_DIR:-$campaign_repo/../WorkTable}" \
BENCHMARK_LOCKFILE="$campaign_dir/Cargo.lock" \
    "$campaign_repo/scripts/capture-environment.sh" >"$campaign_environment"
{
    echo "campaign_mode=$campaign_mode"
    echo "campaign_backends=$campaign_backends"
    echo "campaign_threads=$campaign_threads"
    echo "campaign_records=$campaign_records"
    echo "campaign_operations=$campaign_operations"
    echo "campaign_repetitions=$campaign_repetitions"
    echo "campaign_payload_bytes=$campaign_payload_bytes"
    echo "campaign_buckets=$campaign_buckets"
    echo "campaign_vacuum=$campaign_vacuum"
    echo "campaign_delete_every=$campaign_delete_every"
} >>"$campaign_environment"

campaign_failed=0
target_dir="$campaign_dir/target/shadow-specialized"
cargo build --quiet --release --locked --manifest-path "$campaign_manifest" \
    --bin shadow-concurrency --target-dir "$target_dir"
binary="$target_dir/release/shadow-concurrency"

for threads in $campaign_threads; do
    for backend in $campaign_backends; do
        if ! WT_INDEX_BACKEND="$backend" "$binary" \
            --records "$campaign_records" \
            --operations "$campaign_operations" \
            --threads "$threads" \
            --repetitions "$campaign_repetitions" \
            --payload-bytes "$campaign_payload_bytes" \
            --buckets "$campaign_buckets" \
            --vacuum "$campaign_vacuum" \
            --delete-every "$campaign_delete_every" >>"$campaign_results"
        then
            campaign_failed=1
        fi
    done
done

echo "wrote $campaign_results"
echo "wrote $campaign_environment"
exit "$campaign_failed"
