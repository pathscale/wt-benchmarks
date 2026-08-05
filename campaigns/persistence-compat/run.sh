#!/usr/bin/env bash
set -euo pipefail

campaign_dir="$(cd "$(dirname "$0")" && pwd)"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/wt-persistence-compat.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

render_manifest() {
    local version="$1"
    local destination="$2"
    sed "s/__WORKTABLE_VERSION__/$version/" "$campaign_dir/Cargo.toml.in" > "$destination"
}

run_writer() {
    local version="$1"
    local source="$2"
    local slug="${version//[^a-zA-Z0-9]/-}"
    local crate_dir="$scratch/writer-$slug"
    local store="$scratch/store-$slug"

    mkdir -p "$crate_dir/src"
    render_manifest "$version" "$crate_dir/Cargo.toml"
    cp "$source" "$crate_dir/src/main.rs"
    CARGO_TARGET_DIR="$scratch/target" cargo run --quiet --manifest-path "$crate_dir/Cargo.toml" -- "$store"

    local reader_dir="$scratch/reader-$slug"
    mkdir -p "$reader_dir/src"
    render_manifest "1.0.0-beta.5" "$reader_dir/Cargo.toml"
    cp "$campaign_dir/reader.rs" "$reader_dir/src/main.rs"
    CARGO_TARGET_DIR="$scratch/target" cargo run --quiet --manifest-path "$reader_dir/Cargo.toml" -- "$store"

    printf 'PASS writer=%s reader=1.0.0-beta.5\n' "$version"
}

run_writer "0.8.19" "$campaign_dir/legacy-writer.rs"
run_writer "0.9.0-beta0.2.2" "$campaign_dir/modern-writer.rs"
run_writer "0.9.0-beta0.2.3" "$campaign_dir/modern-writer.rs"
run_writer "0.9.2" "$campaign_dir/modern-writer.rs"
run_writer "0.9.4" "$campaign_dir/modern-writer.rs"
