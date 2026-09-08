#!/bin/sh
# Runs the whole case: the correctness reproducer, then four build conditions
# of the same program, interleaved.
#
#   ./run.sh [output-directory]
#
# Every condition is a release build. They differ only in what arctic-wt
# itself was compiled with, which is the whole point:
#
#   baseline        release, arctic-wt default features OFF, no assertions
#   validate        + arctic-wt/validate       (hypothesis 1, feature form)
#   debug-asserts   + RUSTFLAGS -C debug-assertions=on (hypothesis 1, the form
#                     a dev-profile build actually produces; RUSTFLAGS is
#                     global, so it reaches arctic-wt as well)
#   smr             + arctic-wt/smr-ps-reclaim (hypothesis 2, the default
#                     feature set any ordinary dependant links)
#
# The four binaries are built first and then run ROUND ROBIN, three rounds
# each, because running one condition to completion before starting the next
# confounds the condition with drift. `btree_get` is the control: its code is
# identical in all four builds, so whatever it does across conditions is the
# between-build floor, and no arctic-wt difference smaller than that floor is
# a result.
#
# Correctness before speed: three of the four reproducer tests fail on
# arctic-wt 0.1.7 and pass from 0.1.8, where "Fix validated string prefix scans"
# added `Read::into_prefix`. They are the regression guard, not a pin: a red
# one means the silent-prefix defect is back.
set -eu

out="${1:-results}"
mkdir -p "$out"

{
    echo "== machine and checkout =="
    sw_vers 2>/dev/null || true
    sysctl -n machdep.cpu.brand_string
    echo "physical cores: $(sysctl -n hw.physicalcpu), logical: $(sysctl -n hw.ncpu)"
    echo "arctic-wt checkout: $(git -C ../../../arctic-wt rev-parse HEAD 2>/dev/null || echo unknown)"
    echo "wt-benchmarks checkout: $(git -C ../.. rev-parse HEAD 2>/dev/null || echo unknown)"
} | tee "$out/machine.txt"

echo
echo "== correctness: the silent prefix reproducer =="
echo "   all four must pass from arctic-wt 0.1.8; three of them fail on 0.1.7"
cargo test --release --test prefix_silent_empty > "$out/prefix-reproducer.txt" 2>&1 || true
grep -E "^test |test result:" "$out/prefix-reproducer.txt" || true

echo
echo "== building four conditions =="
cargo build --release --quiet --bin arctic-seqmap-case
mkdir -p target/conditions
cp target/release/arctic-seqmap-case target/conditions/baseline

cargo build --release --quiet --features arctic-validate --bin arctic-seqmap-case
cp target/release/arctic-seqmap-case target/conditions/validate

cargo build --release --quiet --features arctic-smr --bin arctic-seqmap-case
cp target/release/arctic-seqmap-case target/conditions/smr

RUSTFLAGS="-C debug-assertions=on" cargo build --release --quiet \
    --target-dir target/debug-asserts --bin arctic-seqmap-case
cp target/debug-asserts/release/arctic-seqmap-case target/conditions/debug-asserts

echo
echo "== running, round robin =="
for round in 1 2 3; do
    for condition in baseline validate smr debug-asserts; do
        echo "  round $round: $condition"
        "target/conditions/$condition" --jsonl > "$out/$condition-round$round.txt"
    done
done

echo
echo "Wrote $out/<condition>-round<N>.txt, machine.txt and prefix-reproducer.txt"
echo "Compare one arm across conditions with, for example:"
echo "  grep -A 12 'point get] requested 8192' $out/*-round1.txt | grep 'arctic_get '"
