#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
records="${RECORDS:-1000000}"
operations="${OPERATIONS:-5000000}"
repetitions="${REPETITIONS:-5}"
sample_every="${SAMPLE_EVERY:-1024}"
mode="${MODE:-default}"
thread_list="${THREADS:-1}"

mkdir -p "$repo_root/results"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
output="$repo_root/results/ycsb-${mode}-${stamp}.jsonl"
environment="$repo_root/results/ycsb-${mode}-${stamp}.environment.txt"

"$repo_root/scripts/capture-environment.sh" >"$environment"

features=()
if [[ "$mode" == "versioned" ]]; then
    features=(--features versioned-row-publication)
elif [[ "$mode" != "default" ]]; then
    echo "MODE must be default or versioned" >&2
    exit 2
fi

for workload in A B C D E F; do
    for threads in $thread_list; do
        if [[ "$mode" == "default" && "$threads" != "1" && "$workload" != "C" ]]; then
            continue
        fi
        cargo run --quiet --release "${features[@]}" --manifest-path "$repo_root/Cargo.toml" \
            --bin ycsb-worktable -- \
            --workload "$workload" \
            --records "$records" \
            --operations "$operations" \
            --threads "$threads" \
            --repetitions "$repetitions" \
            --sample-every "$sample_every" >>"$output"
    done
done

echo "$output"
echo "$environment"

