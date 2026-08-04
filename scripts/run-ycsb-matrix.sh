#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
records="${RECORDS:-1000000}"
operations="${OPERATIONS:-5000000}"
repetitions="${REPETITIONS:-5}"
sample_every="${SAMPLE_EVERY:-1024}"
mode="${MODE:-default}"
thread_list="${THREADS:-1}"
allow_unsafe_concurrent_d="${ALLOW_UNSAFE_CONCURRENT_D:-false}"

mkdir -p "$repo_root/results"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
output="${CAMPAIGN_RESULTS:-$repo_root/results/ycsb-${mode}-${stamp}.jsonl}"
environment="${CAMPAIGN_ENVIRONMENT:-${output%.jsonl}.environment.txt}"

if [[ -e "$output" ]]; then
    echo "error: refusing to overwrite existing results: $output" >&2
    exit 2
fi
if [[ -e "$environment" ]]; then
    echo "error: refusing to overwrite existing environment capture: $environment" >&2
    exit 2
fi

"$repo_root/scripts/capture-environment.sh" >"$environment"
{
    echo "campaign_mode=$mode"
    echo "campaign_records=$records"
    echo "campaign_operations=$operations"
    echo "campaign_repetitions=$repetitions"
    echo "campaign_sample_every=$sample_every"
    echo "campaign_threads=$thread_list"
    echo "allow_unsafe_concurrent_d=$allow_unsafe_concurrent_d"
} >>"$environment"

if [[ "$mode" == "versioned" ]]; then
    feature_name="versioned-row-publication"
    binary_name="ycsb-worktable"
elif [[ "$mode" == "sqlite" ]]; then
    feature_name="sqlite-adapter"
    binary_name="ycsb-sqlite"
elif [[ "$mode" != "default" ]]; then
    echo "MODE must be default, versioned, or sqlite" >&2
    exit 2
else
    feature_name=""
    binary_name="ycsb-worktable"
fi

for workload in A B C D E F; do
    for threads in $thread_list; do
        if [[ "$mode" == "default" && "$threads" != "1" && "$workload" != "C" ]]; then
            continue
        fi
        if [[ "$mode" == "versioned" && "$threads" != "1" && "$workload" == "D" \
            && "$allow_unsafe_concurrent_d" != "true" ]]
        then
            continue
        fi
        cargo_command=(cargo run --quiet --release)
        if [[ -n "$feature_name" ]]; then
            cargo_command+=(--features "$feature_name")
        fi
        cargo_command+=(--manifest-path "$repo_root/Cargo.toml" --bin "$binary_name" --)
        "${cargo_command[@]}" \
            --workload "$workload" \
            --records "$records" \
            --operations "$operations" \
            --threads "$threads" \
            --repetitions "$repetitions" \
            --sample-every "$sample_every" >>"$output"
    done
done

if command -v jq >/dev/null 2>&1; then
    jq -e -s '
        all(.errors == 0 and .operations_completed == .operations_requested)
    ' "$output" >/dev/null
else
    echo "warning: jq not found; YCSB completion validation was skipped" >&2
fi

echo "$output"
echo "$environment"
