#!/usr/bin/env bash
set -euo pipefail

criterion_root="${1:-}"
if [[ -z "$criterion_root" || ! -d "$criterion_root/criterion" ]]; then
    echo "usage: summarize-criterion-changes.sh CARGO_TARGET_DIR" >&2
    exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "error: jq is required" >&2
    exit 2
fi

find "$criterion_root/criterion" -path '*/change/estimates.json' -print \
    | LC_ALL=C sort \
    | while IFS= read -r estimate_file; do
        benchmark="${estimate_file#"$criterion_root/criterion/"}"
        benchmark="${benchmark%/change/estimates.json}"
        jq -c --arg benchmark "$benchmark" '{
            benchmark: $benchmark,
            mean_change: .mean.point_estimate,
            mean_confidence_lower: .mean.confidence_interval.lower_bound,
            mean_confidence_upper: .mean.confidence_interval.upper_bound,
            median_change: .median.point_estimate,
            median_confidence_lower: .median.confidence_interval.lower_bound,
            median_confidence_upper: .median.confidence_interval.upper_bound
        }' "$estimate_file"
    done
