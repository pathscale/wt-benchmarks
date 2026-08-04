#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rows="${ROWS:-1000000}"
operations="${OPERATIONS:-1000000}"
scan_operations="${SCAN_OPERATIONS:-10000}"
repetitions="${REPETITIONS:-5}"
payload_bytes="${PAYLOAD_BYTES:-32 64 256 1024 4096}"
scan_lengths="${SCAN_LENGTHS:-1 10 100 1000}"

mkdir -p "$repo_root/results"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
output="${CAMPAIGN_RESULTS:-$repo_root/results/micro-layers-${stamp}.jsonl}"
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
    echo "campaign_rows=$rows"
    echo "campaign_operations=$operations"
    echo "campaign_scan_operations=$scan_operations"
    echo "campaign_repetitions=$repetitions"
    echo "campaign_payload_bytes=$payload_bytes"
    echo "campaign_scan_lengths=$scan_lengths"
} >>"$environment"

for current_payload_bytes in $payload_bytes; do
    for scan_length in $scan_lengths; do
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
            --payload-bytes "$current_payload_bytes" >>"$output"
    done
done

if command -v jq >/dev/null 2>&1; then
    jq -e -s '
        map(select(.operation == "range_scan"))
        | group_by([.payload_bytes, .scan_length, .repetition])
        | all(map(.checksum) | unique | length == 1)
    ' "$output" >/dev/null
    jq -e -s '
        map(select(.operation == "insert" or .operation == "update_field"))
        | group_by([.payload_bytes, .scan_length, .repetition, .operation])
        | all(map(.checksum) | unique | length == 1)
    ' "$output" >/dev/null
    jq -e -s '
        map(select(.operation | startswith("point_read")))
        | group_by([.payload_bytes, .scan_length, .repetition])
        | all(map(.checksum) | unique | length == 1)
    ' "$output" >/dev/null
else
    echo "warning: jq not found; cross-layer checksum validation was skipped" >&2
fi

echo "$output"
echo "$environment"
