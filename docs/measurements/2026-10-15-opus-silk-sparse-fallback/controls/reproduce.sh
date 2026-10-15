#!/bin/bash
set -euf
TASK_ARTIFACTS=/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015
TASK_CONTROLS="$TASK_ARTIFACTS/controls"
TASK_RUSTC=/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin/rustc
TASK_BASELINE_DEPS=/tmp/snacka-opus-silk-sparse-baseline-target-20261015/release/deps
TASK_CANDIDATE_DEPS=/tmp/snacka-opus-silk-sparse-fallback-target-20261015/release/deps
for TASK_PROBE in normal fallback composition
do
  "$TASK_RUSTC" --edition=2024 -O "$TASK_CONTROLS/$TASK_PROBE.rs" --extern "snacka=$TASK_ARTIFACTS/libsnacka_baseline_cca.rlib" -L "dependency=$TASK_BASELINE_DEPS" -o "$TASK_CONTROLS/$TASK_PROBE-baseline"
  "$TASK_RUSTC" --edition=2024 -O "$TASK_CONTROLS/$TASK_PROBE.rs" --extern "snacka=$TASK_ARTIFACTS/libsnacka_sparse.rlib" -L "dependency=$TASK_CANDIDATE_DEPS" -o "$TASK_CONTROLS/$TASK_PROBE-candidate"
  "$TASK_CONTROLS/$TASK_PROBE-baseline" "$TASK_CONTROLS/$TASK_PROBE-baseline-data"
  "$TASK_CONTROLS/$TASK_PROBE-candidate" "$TASK_CONTROLS/$TASK_PROBE-candidate-data"
done
"$TASK_RUSTC" --edition=2024 -O "$TASK_CONTROLS/late_output.rs" --extern "snacka=$TASK_ARTIFACTS/libsnacka_sparse.rlib" -L "dependency=$TASK_CANDIDATE_DEPS" -o "$TASK_CONTROLS/late-output-candidate"
"$TASK_CONTROLS/late-output-candidate" > "$TASK_CONTROLS/late-output.json"
python3 "$TASK_CONTROLS/compare_normal.py" "$TASK_CONTROLS/normal-baseline-data" "$TASK_CONTROLS/normal-candidate-data"
python3 "$TASK_CONTROLS/compare_fallback.py" "$TASK_CONTROLS/fallback-baseline-data" "$TASK_CONTROLS/fallback-candidate-data"
python3 "$TASK_CONTROLS/native_compositions.py" "$TASK_CONTROLS/composition-candidate-data"
python3 "$TASK_CONTROLS/compare_composition.py" "$TASK_CONTROLS/composition-baseline-data" "$TASK_CONTROLS/composition-candidate-data"
