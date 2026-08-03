#!/usr/bin/env bash
set -euo pipefail

campaign_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
campaign_repo="$(cd "$campaign_dir/../.." && pwd)"
campaign_manifest="$campaign_dir/Cargo.toml"
campaign_binary="$campaign_dir/target/release/wt-index-backend-campaign"
campaign_results="${CAMPAIGN_RESULTS:-$campaign_repo/results/index-backends-arm64.jsonl}"
campaign_rows="${CAMPAIGN_ROWS:-250000}"
campaign_operations="${CAMPAIGN_OPERATIONS:-2000000}"
campaign_mutations="${CAMPAIGN_MUTATIONS:-100000}"
campaign_sample_every="${CAMPAIGN_SAMPLE_EVERY:-64}"
campaign_repetitions="${CAMPAIGN_REPETITIONS:-7}"

if [[ "$(uname -m)" != "arm64" && "$(uname -m)" != "aarch64" ]]; then
    echo "error: this campaign must run on ARM" >&2
    exit 2
fi

if [[ -e "$campaign_results" ]]; then
    echo "error: refusing to overwrite existing results: $campaign_results" >&2
    exit 2
fi

cargo build --release --locked --manifest-path "$campaign_manifest"

for campaign_backend in worktables_index indexset congee arctic; do
    "$campaign_binary" \
        --backend "$campaign_backend" \
        --rows 10000 \
        --operations 100000 \
        --mutations 5000 \
        --sample-every 64 \
        --repetition 1 >/dev/null
done

campaign_repetition=1
while ((campaign_repetition <= campaign_repetitions)); do
    case $((campaign_repetition % 4)) in
        1) campaign_order="worktables_index indexset congee arctic" ;;
        2) campaign_order="indexset congee arctic worktables_index" ;;
        3) campaign_order="congee arctic worktables_index indexset" ;;
        0) campaign_order="arctic worktables_index indexset congee" ;;
    esac

    for campaign_backend in $campaign_order; do
        "$campaign_binary" \
            --backend "$campaign_backend" \
            --rows "$campaign_rows" \
            --operations "$campaign_operations" \
            --mutations "$campaign_mutations" \
            --sample-every "$campaign_sample_every" \
            --repetition "$campaign_repetition" >>"$campaign_results"
    done
    ((campaign_repetition += 1))
done

echo "wrote $campaign_results"
