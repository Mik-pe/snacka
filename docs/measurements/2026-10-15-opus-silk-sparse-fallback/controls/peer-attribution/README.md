Numerical attribution of the **same frozen NB clipping and WB speech packets**
from sparse fallback `8f723ba`. No encoding, retuning, new corpus, timing or
perceptual scoring occurred. `summary.json` pins the exact packets, range files,
saved PCM, library and diagnostic source hashes; its candidate library hash
matches the parent `controls/source-hashes.json`.

All 96 NB and 75 WB packets decode at their native output rate and at 48 kHz
with exact expected encoder ranges/counts and finite PCM. Regenerated owned and
native 48 kHz files exactly equal the previously saved files.

| Profile/output | Own/native max error | RMSE | Owned/native peak |
|---|---:|---:|---:|
| NB / 8 kHz | .00782247 | .000982807 | 1.0 / 1.0 |
| NB / 48 kHz | .38295269 | .0142561 | 1.38295269 / 1.0 |
| WB / 16 kHz | .00318984 | .000351421 | .660681 / .662109 |
| WB / 48 kHz | .00341892 | .000352498 | .708976 / .710541 |

NB has 255 samples above ±1 among 46,080 output samples. Those positions carry
98.4565% of total squared peer error. A diagnostic-only clamp of the owned
48 kHz PCM lowers RMSE to .00214687 and maximum error to .0953122. Thus output
overshoot explains the large .383 headline but not all remaining difference.
WB has no overshoot, and its discrepancy is already present at native rate.
The script does not modify the saved original PCM or production decoder.

Best peer alignment is zero for all four comparisons. Search is ±24 samples at
native rate and ±144 samples at 48 kHz; every candidate uses the same fixed
common interior. Positive lag compares `own[t]` with `native[t+lag]`. Raw global
statistics include all samples; aligned statistics trim search-sized edges.
`per-packet.csv` gives RMS, peak, maximum error, RMSE and clamp counterfactual
for each packet/rate. These are decoder diagnostics, not quality acceptance.

Owned delay/clamp model, from the frozen source:
- `silk/decoder.rs:159` adds one native sample of mono alignment.
- `silk/native_delay.rs:13` adds a literal 4 native samples for NB and 12 for WB
  when requesting the native output rate. The total synthesis-to-native-output
  delay is therefore 5 NB or 13 WB samples.
- `opus/decoder.rs:848` selects that native path. The 48 kHz path instead uses
  the third-order IIR and 14-tap polyphase numerator in `silk/resample.rs:620`.
  Its explicit delays are 0 NB and 7 WB native samples; frequency-dependent
  phase means the interpolator is not a pure fractional delay.
- Synthesis clamps its PCM (`synthesis.rs:344`), while the 48 kHz interpolator
  and final `write_output` copy have no final ±1 clamp. Signed interpolation
  coefficients can overshoot an already bounded native signal.

For uninterrupted owned mono SILK, writing `N` for owned native output and `R`
for the exact owned interpolator gives `owned48[j] = R(N)[j+F*D]` over the
available common prefix: advance interpolated native output 24 NB or 36 WB
samples. Mono alignment is already in `N`; do not advance another sample.
This source-derived relation was not used to replace the public decoder in
the numerical measurements.

Exact reproduction:

```sh
TASK_ROOT=/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015
TASK_PEER="$TASK_ROOT/controls/peer-attribution"
/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin/rustc --edition=2024 -O "$TASK_PEER/decode.rs" --extern "snacka=$TASK_ROOT/libsnacka_sparse.rlib" -L dependency=/tmp/snacka-opus-silk-sparse-fallback-target-20261015/release/deps -o "$TASK_PEER/decode-owned"
"$TASK_PEER/decode-owned" "$TASK_ROOT/refusal-sparse" "$TASK_PEER"
env PYTHONPATH=/tmp/snacka-parity-20261015/python-deps OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 /Users/mikpe/.cache/codex-runtimes/codex-primary-runtime/dependencies/python/bin/python3 "$TASK_PEER/analyze.py"
```

Native decoding uses the public libopus 1.6.1 API from
`/opt/homebrew/lib/libopus.dylib`.
