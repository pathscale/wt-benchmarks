#!/usr/bin/env bash
set -euo pipefail

campaign_root="$(cd "$(dirname "$0")/.." && pwd)"
output_dir="${1:-$campaign_root/results/storage-$(date -u +%Y%m%dT%H%M%SZ)}"
rows="${WT_FOOTPRINT_ROWS:-100000}"
payload_bytes="${WT_FOOTPRINT_PAYLOAD_BYTES:-64}"
drain_every="${WT_FOOTPRINT_DRAIN_EVERY:-5000}"
target_dir="$(mktemp -d "${TMPDIR:-/tmp}/wt-storage-build.XXXXXX")"
trap 'rm -rf "$target_dir"' EXIT

if [[ -e "$output_dir" ]]; then
    printf 'error: output path already exists: %s\n' "$output_dir" >&2
    exit 2
fi
mkdir -p "$output_dir/data"
{
    rustc -Vv
    cargo -V
    uname -a
    printf 'rows=%s\npayload_bytes=%s\ndrain_every=%s\n' \
        "$rows" "$payload_bytes" "$drain_every"
} > "$output_dir/environment.txt"

run_one() {
    local engine="$1" binary="$2" feature="$3"
    CARGO_TARGET_DIR="$target_dir" cargo build \
        --quiet --locked --manifest-path "$campaign_root/Cargo.toml" \
        --profile paper-speed --no-default-features --features "$feature" \
        --bin "$binary"
    "$target_dir/paper-speed/$binary" \
        --path "$output_dir/data/$engine" \
        --rows "$rows" \
        --payload-bytes "$payload_bytes" \
        --drain-every "$drain_every" \
        > "$output_dir/$engine.jsonl"
}

run_one worktable storage-worktable worktable-backend
run_one sqlite-bundled storage-sqlite sqlite-backend
run_one redb storage-redb redb-backend

printf 'results: %s\n' "$output_dir"
