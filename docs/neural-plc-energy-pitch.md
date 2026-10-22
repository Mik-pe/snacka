# Target-energy and local-pitch feasibility

**Prioritize a target-energy ablation; keep the accepted PR31 model installed.** This is a source audit and synthetic CPU/numerical preflight against main `ada82abf997af35f7563c066112f7db24ff8ec83`, not a newly trained model, speech-quality result or frozen training protocol. No cloud instance was allocated and no new speech was downloaded. Full measurements and source hashes are in [the numeric record](neural-plc-energy-pitch-results.json).

The [causal study](neural-plc-causal.md) establishes that distillation passed its particular validation and independent acceptance guards. It does not isolate why direct sequence training failed: objectives, targets and sequence/frame sampling differ. A noisy waveform-gradient explanation is plausible, but is not a measured causal mechanism. Nor does the result establish optimization saturation or a need for recurrence.

## Target-energy representation

The proposed 80→64→64→17 representation is feasible without adding network inputs. It does require an exact energy reference outside the existing 80 features. `History::observe` currently computes per-band mean power, transforms it to log amplitude, subtracts the largest band, scales and clips it. The overall energy feature is also scaled and clipped. These values cannot recover all original band energies: a new executable test constructs two received spectra whose complete 80-feature vectors are bit-identical while their first-band log powers differ by more than 1.0.

A simple implementation would retain 16 unnormalized log powers before feature clipping, costing **64 logical bytes** of f32 history. The expensive logarithms already occur on received frames; caching can reuse those intermediates. Extra stores, actual Decoder size, reset/recovery behavior, allocations and integrated runtime must still be measured after implementation. There is no established zero-cost received-frame result for that implementation.

Use the same native band edges and per-bin mean powers for both 480- and 960-bin frames:

```
L_last[b] = ln(mean(last_received_mdct[b]^2) + eps)
d[b]      = predicted L_target[b] - L_last[b]
L_pred[b] = ln(mean(actual_phase_shifted_candidate_mdct[b]^2) + eps)
g[b]      = exp(clamp(0.5 * (d[b] + L_last[b] - L_pred[b]),
                      ln(0.7875), ln(1.2125)))
```

The probe uses `eps=1e-12`. Clamp before exponentiation to bound finite extreme predictions safely. This makes the denominator consistent with the student's actual phase output, rather than the teacher's selected phase. No second forward MDCT is required. Band-mean power avoids a frame-width factor if frame geometries are compared. A zero candidate still cannot reconstruct missing energy through multiplication.

The cache and labels must come from the **post-Phase1 received/reference MDCT coefficients**, with the same epsilon and band definition. Re-export raw target log powers from uninterrupted decoded references. Existing teacher NPZ files contain **clamped gains**, so inverting those labels cannot recover the unclamped target powers. The old oracle uses f64 sum-power ratios without this mean-power epsilon; equivalence is approximate in weak-energy bands and must be audited explicitly, rather than asserted bit-exact.

Keep the phase control, bounded physical gains and existing eligibility checks. A zero target-energy output predicts the last received power; it does **not** generally imply gain=1. Disabled/fallback paths must explicitly return identity. A target-energy export also needs a distinct model/dataset format tag: its 17 rows cannot be loaded through the current gain/phase activation merely because the tensor dimensions match. Numerical tests cover ordinary/clamped ratios, silence and extreme finite logits; they do not establish perceptual improvement.

## Local pitch: history and reliability

Native `Concealment` holds **1,920 samples / 40 ms**. Its current pitch search uses a 960-sample correlation window and lags up to 960. A second full 20 ms window one 20 ms hop earlier would require `960 + 960 + lag` samples: every positive lag exceeds the available history. Therefore the proposed older-frame search cannot simply reuse those full-window bounds.

The prototype uses two **480-sample windows 480 samples apart**, each with a ±6 integer-lag search, stride3 dot products and fractional refinement. This fits all native lags 80…960 without extending received history. The older-only version reuses the native full-window estimate; the matched version measures both short windows so the two estimates have the same temporal support. These estimate a local period difference over 10 ms, not a direct per-frame phase offset. Mapping period drift to a phase trajectory requires a separate definition and ablation.

The prototype rejects incomplete/silent/low-confidence histories and records search-boundary hits. It recovers stationary periods and the drift sign on the tested clean slowly chirping tones. However, adding fixed noise to a 173 Hz tone with a **+35 Hz/s** slope gives a measured period difference **+0.842 samples**, the wrong sign, despite native/local correlations **0.948/0.949** and no search-boundary hit. This single synthetic counterexample does not measure speech error rates, but disproves the assumption that a high correlation alone guarantees a useful pitch derivative. The noisy timing example also lies below the production neural bypass threshold 0.985, so it exercises a potentially relevant confidence range.

Compute and cache local features only at loss start if this is pursued. Define an unavailable/edge-hit representation and reset rules, and use identical semantics at 12/24/32 kbps. Replacing currently sparse f0 slots would preserve 80 inputs but changes their meaning and discards some existing harmonic information at 12 kbps; freeze the mapping before training. Caching drift across a burst would add state. Neither a universal availability claim nor its CPU cost has been demonstrated on speech.

## M5 component timings

The checked-in runner compiles the native search and band definitions directly from current sources, runs 12 tests, then retains **all three timing runs**, each with 21 batches of 8,192 calls/component after warmup. Rust 1.99.1, `opt-level=3`, Apple M5. The machine was shared; scheduler/core/load variation is substantial. These are wall timings of synthetic components, not integrated decoder medians, a cost difference against production, or a worst-case deadline.

|Component|Samples|Run1 median µs|Run2 median µs|Run3 median µs|
|---|---:|---:|---:|---:|
|Band log powers|480|0.355|0.364|0.437|
|Band log powers + analytic gains|480|0.311|0.399|0.438|
|Band log powers|960|0.693|1.247|0.753|
|Band log powers + analytic gains|960|0.696|1.068|0.840|
|Native search, including history copy|1920|26.669|51.703|35.915|
|One older-window search, 13 lags + refinement|1920|5.544|2.654|2.371|
|Matched searches, 26 lags + refinement|1920|40.247|4.875|5.489|

Earlier exploratory runs were faster (approximately 0.20/0.42 µs for analytic gains at 480/960 bins and 1.83/3.30 µs for older/matched pitch windows). The final three runs above and their complete batch distributions are retained rather than selecting the fastest environment. The claimed `<0.15 µs` energy/gain path and `<0.4 µs` pitch overhead are **not established**. Different components may reverse their expected timing order under load.

PR31 measured about 22 µs for the full 20 ms first-loss path in its own environment. Do not add one of these component medians to that earlier figure and call the result a measured decoder runtime. Test an integrated candidate and the accepted control in the same interleaved CPU experiment before admission. Target energy appears the cheaper first experiment; the straightforward matched pitch search has material cost and still lacks a demonstrated useful speech signal.

## Bandwidth and fresh data

Deterministically band-limiting half the native48k English training clips would weaken the source-bandwidth/language association. Select the exact half **within speaker**, carry effective source bandwidth per record, and apply masks from that record rather than from language. Apply the lowpass/resampling before codec/noise augmentation and retain PCM hashes and filter settings. Source-matched validation needs English wideband and fullband strata, plus Swedish wideband, at all three bitrates.

This does not produce Swedish fullband speech or make the two languages fully bandwidth-symmetric. A 16 kHz file resampled to 48 kHz is still limited to an 8 kHz source Nyquist frequency. High bands must remain unsupported for that record.

A new VCTK speaker exception is unavailable under the documented inventory: the [earlier corpus](neural-plc-design.md#corpus-and-split-policy) already used **78 training + 17 validation + 12 test = 107** available mic2 speakers. New utterances from them do not constitute an unseen-speaker cohort. [VCTK's publisher](https://datashare.ed.ac.uk/handle/10283/3443) documents its native48k releases and the mic2 problems for p280/p315; those two are not a new 12-speaker holdout.

Other suggestions also require source-specific handling:

- [EARS](https://github.com/facebookresearch/ears_dataset) supplies 48 kHz recordings from 107 speakers, but its [actual license](https://github.com/facebookresearch/ears_dataset/blob/main/LICENSE) is CC BY-NC4.0. It is not an interchangeable permissive training source; this preflight does not ingest it or decide the permitted scope of a future use.
- [DNS-Challenge's official inventory](https://github.com/microsoft/DNS-Challenge/blob/master/README.md) explicitly includes VCTK in `clean_fullband`. Treating that directory as an entirely new corpus would permit duplicate speakers/audio. Resolve original source identities and licenses before selecting any holdout.
- PR31's LibriSpeech/RixVox acceptance speakers are consumed. New IDs must exclude all documented prior train/validation/test exposures. Published-ID separation is not biometric proof across different corpora.

A fresh fullband source/identity inventory remains unresolved. The preflight has neither reserved an independent fullband cohort nor opened new test audio.

## Proposed follow-up, not preregistration

1. Implement and verify the 64-byte energy cache, versioned raw target-energy export and native/PyTorch gain conversion. Test clipping/epsilon behavior, Phase1 target identity, history reset, source-band masks and inference parity. Measure production CPU/allocation/footprint changes against the accepted model.
2. Before fitting, seal data manifests, effective-bandwidth transforms, speaker exposure exclusions, sampling, objectives and selection. A small 2×2 fit design (gain-ratio versus target-energy; original versus bandwidth-balanced data) separates the two changes. Keep 80→64→64→17, optimizer-step budget and teacher phase rule matched. Report phase-off/on applications of each fit so changed shared-hidden phase behavior is visible. One successful run would not establish a new training-time guarantee of 11.66 seconds.
3. Admit/rank only on decoded validation, with waveform/envelope/ESTOI guards and explicit language/bandwidth/bitrate strata. Compare with both classical PLC and the installed PR31 control. Keep a single locked selection and no test fallback. Only the selected candidate enters newly inventoried independent speakers; freeze the independent rule before training and require paired non-regression versus the installed model as well as improvement versus classical PLC before replacement.
4. Treat local pitch features as a later experiment after their cost and reliability pass a separate speech-level preflight. No GRU is justified by these measurements.

Reproduce the isolated checks with:

```sh
python3 tools/measure_plc_energy_pitch.py /absolute/new/output-directory
```

No `src/`, runtime dependencies, packets or installed weights changed in this preflight. Colab remained inactive at **1020.43 CU**, rate0; this preflight added **0 CU**. No claim of a new trained-model quality improvement is made.
