# Bounded SILK refusal fallback, 2026-10-15

Experimental source: `8f723ba1f35c053e1cc840f38a4b20446a127dde`, frozen before the full comparison. Baseline: `cca87dd757d96623b91bff81d34a021e5a3e2792`. This local experiment addresses valid-input `Output` refusals. Root has accepted the bounded functional correction and its disclosed CPU cost after local integrated checks. Source-only integration commit `76224786eef73423a51b16f17c126ec07c534d24` passes 380 tests across 18 suites, formatting and strict release Clippy. Exact-head package/push is handled separately; this report does not claim remote publication. It does not establish perceptual parity.

## Frozen algorithm

The complete ordinary model/gain pipeline stays unchanged. Only a final missing regular-stage candidate invokes a separately compiled cold helper. The original native PCM and pretrial causal history are reused unchanged. The existing normal quantizer is const-specialized as mode zero, so its sample loop has no runtime sparse-policy condition.

Eight model slots are bounded in advance: original; existing spectrum broadening at 0.95, 0.9, 0.8 and 0.6 without pitch; flat spectrum; broadening at 0.8 and 0.6 with the original pitch when already voiced with a nonzero lag. No additional pitch search runs. Unvoiced variants preserve `Inactive` for an originally inactive frame; active input remains active.

Each model has exactly twelve requested points: gains 57/60/63 crossed with penalties 0.5/1.25/4, plus explicit zero-pulse choices at gains 0/60/63. At most 96 causal quantizations run per failed stage; unvoiced/inactive stages use six models and at most 72. Requested gains pass through the unchanged absolute/conditional wire rules, so requested zero is the lowest reachable anchor, not necessarily effective gain zero. Zero pulses still reconstruct the standardized offset and dither.

The sparse sample decision compares the existing nearest wire cell with zero. For reconstructed cell `v = n - sign(n)*20/256` and normalized target `c`, it retains the nearest pulse when `v*(2*c-v) > penalty`. All seed, LPC, LTP, output and shaping histories follow the chosen pulse. Candidate ranking uses finite-history, clamped f64 encoder-reconstruction proxy SSE against the same original input. Public quantization-error statistics retain their existing unclamped arithmetic and meaning. This proxy is not exact native PCM or a perceptual metric.

A candidate must pass the existing estimated cost and a real isolated frame serialization/termination check. Before actual output, a fallback layer receives an additional full syntax/termination preflight including header, prior LBRR, stereo metadata and all regular stages. Its fixed 8192-byte scratch cap is an admission guard; owned Opus payloads are at most 1275 bytes. It is not a general standalone SILK format limit. Scratch does not grow with the number of trials.

## Transactionality and retained limits

Regular state/cache commits remain after actual layer emission. A standalone caller can append to an already-started entropy coder, whose interval cannot be cloned. The new fresh-layer preflight therefore does not promise rollback for an eventual real `Output` on that caller's existing writer; callers retain the reset requirement. The external late-output probe tests this contract and subsequent reset/reuse.

Optional prior-LBRR deferral is deliberately outside this patch. Dropping a reserved prior layer honestly would require restarting the whole packed plan from original primary/shadow histories and preserving later causal cache decisions. A packet can therefore still refuse when its retained LBRR or minimum syntax leaves insufficient room. This fallback is bounded feature coverage, not universal encoder acceptance.

## Functional validation

The two recorded original development failures now encode without input attenuation, amplitude retry or activity relabeling:

| Profile | Baseline | Candidate | Native wire proof |
| --- | --- | --- | --- |
| NB 10 ms, mono, 12 kb/s, normalized fullscale clip, 48 kHz input | `Output` at frame 0 | 96/96 packets | all 15 bytes, active VAD, finite PCM and exact final ranges |
| WB 20 ms, mono, 6 kb/s, original `cen2-fjam-b` speech, 48 kHz input | `Output` at frame 22 | 75/75 packets | all 15 bytes, active VAD, finite PCM and exact final ranges |

The WB first 22 successful packets are byte exact. Subsequent candidate packets cannot be compared as an unchanged baseline history after the old refusal.

External synthetic probes exercise mono/stereo, native/48 kHz input, 10/20/40/60/80/100/120 ms, retained prior LBRR, loss recovery and normal continuation. Their 57 baseline-refusing profiles now encode all 462 candidate packets. Separate mapped/projection proofs rescue four more profiles and 64 native multistream packets, matching independent gathered/mixed stream encoders in transport bytes, per-stream/XOR ranges and owned PCM. Synthetic callback allocation counters remain zero; complete construction/processing proofs run inside a 1 MiB thread. These are exercised bounds, not a formal maximum-stack proof. The 22 ordinary control streams and 354 packets stay exact.

The larger historical generator completes all 924 cases and 2800 traces. Its original 288686-packet count included attenuated retries after baseline errors. Against the current baseline, all 2578 entirely successful traces (267374 packets) remain exact, and all 222 prefixes before the first old refusal (1530 packets) remain exact. Packet bytes, final ranges, metadata, ordinary output, PLC-return output and FEC-return output are covered. The candidate completes all 288686 original full-amplitude packets without any refused trace. After-refusal histories are validated independently and are not asserted identical. `trace-summary.json` pins the external full-row hash.

All 264 release library tests pass with `opus,reference-tests`, including six new SILK quantizer/budget/state tests. Release Clippy for library and tests passes with warnings denied. Native checks use installed `libopus 1.6.1` solely as a black-box oracle.

## Quality and performance limits

The same two original development signals retain a substantial quality deficit. Native-decoded candidate unscaled aligned SNR is 1.166 dB versus native-reference 4.588 dB for the clipping profile, and 1.213 dB versus 4.060 dB for WB speech. The WB 6 kb/s native encoder actually selects NB 20 ms (TOC 8/11), while the owned candidate remains WB 20 ms (TOC 75): that native-reference diagnostic is explicitly **UNMATCHED in wire bandwidth**, and is not a WB quality-parity comparison. The NB reference selects the matching NB 10 ms wire mode. These delay-searched diagnostics trim 1920 samples at both ends and search 0..1024 samples; they are not independent perceptual acceptance. Candidate output RMS is lower. Maximum owned/native PCM difference is 0.383 on NB clipping and 0.00342 on WB speech despite exact entropy ranges; the preserved decoder diagnosis below attributes most of the NB maximum to interpolation overshoot. No source was retuned after these observations.

Native-rate attribution on exactly the same packets finds NB own/native RMSE 0.000983 at 8 kHz and 0.014256 at 48 kHz. Owned 48 kHz peak is 1.38295 while native peak is 1.0. Only 255/46080 output samples exceed ±1; those samples carry 98.4565% of squared peer error. A diagnostic-only clamp reduces RMSE to 0.002147, leaving a measurable residual. WB RMSE is 0.000351 at 16 kHz and 0.000352 at 48 kHz, with no overshoot. Best peer alignment is zero in every tested output rate. Source review identifies bounded native synthesis followed by the independently fitted, unclamped interpolation filter; no clamp or codec change was introduced. The peer attribution folder retains per-packet statistics, RMS, alignment conventions, source/binary hashes and exact reproduction.

The [completed quiet M5 cost report](cpu/README.md) retains every throughput regression and all 33,088 individual intervals. Old-success median cost changes range from +0.060% to +2.723%. No observed callback exceeds its period; the worst fraction is 27.35%, and the largest absolute call is 23.568 ms on a 120 ms packet. Sixteen repeated deterministic contexts in one process per role are not independent host/scheduler replicates or universal real-time bounds. Root functional admission follows the local integrated-check checkpoint recorded above; publication is a separate step. No fresh English or reserved Swedish corpus was inspected or scored.

## Reproduction and artifacts

External run root: `/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015`. `frozen-source.json` binds production source and pre-proof libraries; raw packet/PCM outputs and full trace rows remain external with hashes in summaries. The archived probe and runner retain their original absolute paths to make their executed commands explicit.

```sh
LIBRARY_PATH=/opt/homebrew/lib CARGO_TARGET_DIR=/tmp/snacka-opus-silk-sparse-fallback-target-20261015 cargo test --locked --release --features opus,reference-tests --lib
LIBRARY_PATH=/opt/homebrew/lib CARGO_TARGET_DIR=/tmp/snacka-opus-silk-sparse-fallback-target-20261015 cargo clippy --locked --release --lib --tests --features opus,reference-tests -- -D warnings
python3 /tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015/run-prefix-proof.py
python3 /tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015/native-refusals.py
```
