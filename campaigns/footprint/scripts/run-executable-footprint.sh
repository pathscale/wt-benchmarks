#!/usr/bin/env bash
set -euo pipefail

campaign_root="$(cd "$(dirname "$0")/.." && pwd)"
output_dir="${1:-$campaign_root/results/executable-$(date -u +%Y%m%dT%H%M%SZ)}"
target_dir="$(mktemp -d "${TMPDIR:-/tmp}/wt-footprint-build.XXXXXX")"
trap 'rm -rf "$target_dir"' EXIT

if [[ -e "$output_dir" ]]; then
    printf 'error: output path already exists: %s\n' "$output_dir" >&2
    exit 2
fi
mkdir -p "$output_dir/binaries" "$output_dir/dependencies" "$output_dir/sections"
{
    rustc -Vv
    cargo -V
    uname -a
} > "$output_dir/environment.txt"

printf '%s\n' 'profile,engine,tables,file_bytes,checksum,sha256' > "$output_dir/results.csv"

file_bytes() {
    stat -f '%z' "$1" 2>/dev/null || stat -c '%s' "$1"
}

hash_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

capture_dependencies() {
    local binary="$1" output="$2"
    if command -v otool >/dev/null 2>&1; then
        otool -L "$binary" > "$output"
    elif command -v ldd >/dev/null 2>&1; then
        ldd "$binary" > "$output"
    else
        printf '%s\n' 'No otool or ldd available.' > "$output"
    fi
}

capture_sections() {
    local binary="$1" output="$2"
    if command -v llvm-size >/dev/null 2>&1; then
        llvm-size --format=sysv "$binary" > "$output"
    elif command -v size >/dev/null 2>&1; then
        size "$binary" > "$output"
    else
        printf '%s\n' 'No llvm-size or size available.' > "$output"
    fi
}

build_one() {
    local profile="$1" engine="$2" tables="$3" binary_name="$4" features="$5"
    CARGO_TARGET_DIR="$target_dir" cargo build \
        --quiet --locked --manifest-path "$campaign_root/Cargo.toml" \
        --profile "$profile" --no-default-features --features "$features" \
        --bin "$binary_name"

    local source="$target_dir/$profile/$binary_name"
    local stem="$profile-$engine-tables-$tables"
    local destination="$output_dir/binaries/$stem"
    local checksum
    checksum="$("$source")"
    cp "$source" "$destination"
    capture_dependencies "$destination" "$output_dir/dependencies/$stem.txt"
    capture_sections "$destination" "$output_dir/sections/$stem.txt"
    printf '%s,%s,%s,%s,%s,%s\n' \
        "$profile" "$engine" "$tables" "$(file_bytes "$destination")" \
        "$checksum" "$(hash_file "$destination")" >> "$output_dir/results.csv"
}

for profile in paper-speed paper-size; do
    for tables in 1 2 4 8; do
        build_one "$profile" control "$tables" footprint-control "tables-$tables"
        build_one "$profile" worktable "$tables" footprint-worktable \
            "worktable-backend,tables-$tables"
        build_one "$profile" sqlite-bundled "$tables" footprint-sqlite \
            "sqlite-backend,tables-$tables"
        build_one "$profile" redb "$tables" footprint-redb \
            "redb-backend,tables-$tables"
    done
done

printf 'results: %s\n' "$output_dir"
