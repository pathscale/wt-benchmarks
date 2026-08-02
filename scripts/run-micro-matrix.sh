#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rows="${ROWS:-1000000}"
operations="${OPERATIONS:-1000000}"
scan_operations="${SCAN_OPERATIONS:-10000}"
repetitions="${REPETITIONS:-5}"

mkdir -p "$repo_root/results"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
output="$repo_root/results/micro-layers-${stamp}.jsonl"
environment="$repo_root/results/micro-layers-${stamp}.environment.txt"

"$repo_root/scripts/capture-environment.sh" >"$environment"

for payload_bytes in ${PAYLOAD_BYTES:-"32 64 256 1024 4096"}; do
    for scan_length in ${SCAN_LENGTHS:-"1 10 100 1000"}; do
        if (( scan_length > rows )); then
            continue
        fi
        cargo run --quiet --release --manifest-path "$repo_root/Cargo.toml" \
            --bin micro-layers -- \
            --rows "$rows" \
            --operations "$operations" \
            --scan-operations "$scan_operations" \
            --scan-length "$scan_length" \
            --repetitions "$repetitions" \
            --payload-bytes "$payload_bytes" >>"$output"
    done
done

echo "$output"
echo "$environment"

