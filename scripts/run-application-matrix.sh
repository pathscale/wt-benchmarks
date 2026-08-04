#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
result_prefix="${CAMPAIGN_RESULT_PREFIX:-$repo_root/results/application-${stamp}}"
environment="${result_prefix}.environment.txt"
kv_output="${result_prefix}-kv.jsonl"
speedtest_output="${result_prefix}-speedtest1.jsonl"
linkbench_output="${result_prefix}-linkbench.jsonl"
tatp_output="${result_prefix}-tatp.jsonl"

rows="${CAMPAIGN_ROWS:-100000}"
operations="${CAMPAIGN_OPERATIONS:-100000}"
scan_operations="${CAMPAIGN_SCAN_OPERATIONS:-1000}"
scan_length="${CAMPAIGN_SCAN_LENGTH:-100}"
payload_bytes="${CAMPAIGN_PAYLOAD_BYTES:-64}"
repetitions="${CAMPAIGN_REPETITIONS:-5}"
redb_durabilities="${CAMPAIGN_REDB_DURABILITIES:-relaxed}"
redb_scopes="${CAMPAIGN_REDB_SCOPES:-per-operation batch}"
linkbench_nodes="${CAMPAIGN_LINKBENCH_NODES:-100000}"
linkbench_links_per_node="${CAMPAIGN_LINKBENCH_LINKS_PER_NODE:-20}"
linkbench_operations="${CAMPAIGN_LINKBENCH_OPERATIONS:-1000000}"
tatp_subscribers="${CAMPAIGN_TATP_SUBSCRIBERS:-100000}"
tatp_operations="${CAMPAIGN_TATP_OPERATIONS:-1000000}"
tatp_threads="${CAMPAIGN_TATP_THREADS:-1 4 8}"

mkdir -p "$repo_root/results"
for output in \
    "$environment" \
    "$kv_output" \
    "$speedtest_output" \
    "$linkbench_output" \
    "$tatp_output"
do
    if [[ -e "$output" ]]; then
        echo "error: refusing to overwrite existing result: $output" >&2
        exit 2
    fi
done

"$repo_root/scripts/capture-environment.sh" >"$environment"
{
    echo "campaign_rows=$rows"
    echo "campaign_operations=$operations"
    echo "campaign_scan_operations=$scan_operations"
    echo "campaign_scan_length=$scan_length"
    echo "campaign_payload_bytes=$payload_bytes"
    echo "campaign_repetitions=$repetitions"
    echo "campaign_redb_durabilities=$redb_durabilities"
    echo "campaign_redb_scopes=$redb_scopes"
    echo "campaign_linkbench_nodes=$linkbench_nodes"
    echo "campaign_linkbench_links_per_node=$linkbench_links_per_node"
    echo "campaign_linkbench_operations=$linkbench_operations"
    echo "campaign_tatp_subscribers=$tatp_subscribers"
    echo "campaign_tatp_operations=$tatp_operations"
    echo "campaign_tatp_threads=$tatp_threads"
} >>"$environment"

default_target="$repo_root/target/application-default"
versioned_target="$repo_root/target/application-versioned"
cargo build --quiet --release --locked --features external-adapters \
    --bins --manifest-path "$repo_root/Cargo.toml" --target-dir "$default_target"
cargo build --quiet --release --locked --features versioned-row-publication \
    --bin tatp-worktable --manifest-path "$repo_root/Cargo.toml" \
    --target-dir "$versioned_target"

common_kv_args=(
    --rows "$rows"
    --operations "$operations"
    --scan-operations "$scan_operations"
    --scan-length "$scan_length"
    --repetitions "$repetitions"
    --payload-bytes "$payload_bytes"
)

echo "running embedded KV matrix" >&2
"$default_target/release/kv-worktable" "${common_kv_args[@]}" \
    --durability memory --transaction-scope per-operation >>"$kv_output"
"$default_target/release/kv-sqlite" "${common_kv_args[@]}" \
    --durability memory --transaction-scope per-operation >>"$kv_output"
for durability in $redb_durabilities; do
    for scope in $redb_scopes; do
        echo "running redb: durability=$durability scope=$scope" >&2
        "$default_target/release/kv-redb" "${common_kv_args[@]}" \
            --durability "$durability" --transaction-scope "$scope" >>"$kv_output"
    done
done

common_speedtest_args=(
    --rows "$rows"
    --operations "$operations"
    --repetitions "$repetitions"
    --scan-length "$scan_length"
    --payload-bytes "$payload_bytes"
)
echo "running paired speedtest1 core shapes" >&2
"$default_target/release/speedtest1-worktable" "${common_speedtest_args[@]}" \
    >>"$speedtest_output"
"$default_target/release/speedtest1-sqlite" "${common_speedtest_args[@]}" \
    >>"$speedtest_output"

echo "running LinkBench shape" >&2
"$default_target/release/linkbench-worktable" \
    --nodes "$linkbench_nodes" \
    --links-per-node "$linkbench_links_per_node" \
    --operations "$linkbench_operations" \
    --repetitions "$repetitions" \
    --payload-bytes "$payload_bytes" >>"$linkbench_output"

for threads in $tatp_threads; do
    tatp_binary="$default_target/release/tatp-worktable"
    if ((threads > 1)); then
        tatp_binary="$versioned_target/release/tatp-worktable"
    fi
    echo "running TATP: threads=$threads" >&2
    "$tatp_binary" \
        --subscribers "$tatp_subscribers" \
        --operations "$tatp_operations" \
        --threads "$threads" \
        --repetitions "$repetitions" >>"$tatp_output"
done

if command -v jq >/dev/null 2>&1; then
    jq -e -s '
        group_by([.operation, .repetition])
        | all(map(.checksum) | unique | length == 1)
    ' "$kv_output" >/dev/null
    jq -e -s '
        group_by([.phase, .repetition])
        | all(map(.checksum) | unique | length == 1)
    ' "$speedtest_output" >/dev/null
else
    echo "warning: jq not found; cross-engine checksum validation was skipped" >&2
fi

echo "wrote $kv_output"
echo "wrote $speedtest_output"
echo "wrote $linkbench_output"
echo "wrote $tatp_output"
echo "wrote $environment"
