# Target-energy protocol and cache implementation

**The method for the 2×2 study is frozen; its dataset binding is still required before fitting.** Step1 now has an executable 64-byte cache overlay, native parity/allocation/CPU evidence, and tested PyTorch loss primitives. The installed PR31 decoder and weights remain unchanged. No new model has been fitted or admitted.

The immutable method specification is [neural-plc-target-energy-protocol.json](neural-plc-target-energy-protocol.json). It deliberately has `training_allowed:false`: a separate binding must pin the exact code commit, protocol hash, raw train/validation label exports, transformed PCM manifests, prior speaker-exposure audit and new16k acceptance metadata. This is not a claim that all training data have already been re-exported. The [numeric evidence](neural-plc-target-energy-results.json) retains source/executable/fixture hashes and every measured block summary.

## Three design choices

### Bounded teacher targets without dead student gradients

Let `lo=ln(.7875)`, `hi=ln(1.2125)`, `z=.5*(d+L_last-L_pred_teacher)` and `y=clamp(.5*(L_target-L_pred_teacher),lo,hi)`. Use `smoothL1(z,y,beta=.05)`. Clip the **teacher target**, while retaining the student's pre-clamp `z` in the supervised loss. The actual decoder gain remains `exp(clamp(z,lo,hi))`.

This limits the influence of unachievable teacher energy extremes: target changes from -8 to -80 to -800 nats give identical loss/gradient once below the physical bound. Unlike supervising only a hard-clamped student gain, this loss still has a corrective gradient when the student saturates at the wrong bound. Both properties are tested with PyTorch autograd.

The gain-ratio control uses `z=log(1+.2125*tanh(raw))` with the **same** target, Huber beta and action regularizer. It is a matched new control, not an exact rerun of PR31's older squared-log-gain objective. The 2×2 distinguishes representation and bandwidth under this new common objective; it does not independently attribute a difference versus PR31 to the denominator alone.

The offline `L_pred_teacher` is measured at the oracle's selected phase on strictly classical causal history. At inference the denominator is measured on the **actual student-phase** candidate. These are deliberately distinct: the training primitive is a fixed teacher-conditioned proxy, not a claim of exact end-to-end phase-conditioned waveform training. Phase-off/on evaluations of every fitted model must report the resulting behavior. Raw target log powers must still be re-exported from uninterrupted post-Phase1 references; clamped legacy gain NPZ files cannot provide them.

### Identity follows classical PLC, including its fade

Regularize `.01*(exp(clamp(z,lo,hi))-1)^2`, not `d²`. This anchors the action to gain1 and retains the classical fade. With a fading candidate, identity corresponds to `d=L_pred-L_last`, which is generally negative. `d=0` instead holds the last received target power and can request the maximum gain.

Native tests cover that distinction at multiple fade levels and both frame sizes. AdamW parameter weight decay remains separately fixed; it is not an output-level `d=0` regularizer. Phase supervision/regularization remains `.1*smoothL1(phase/8,target/8)+.001*(phase/8)^2`. Supported bands are averaged **per frame**, then balanced by language, so16 supported English bands do not silently outweigh13 Swedish bands.

### Reuse received-frame logarithms; measure integrated cost

The overlay saves each existing mean-power logarithm before normalization/clipping. It adds no received-frame spectral pass or extra logarithm. `History` grows by64 bytes; the compiled Decoder grows **16,288→16,352 bytes**, with Encoder12,128 and logical model constants11,464 unchanged. Cache reset and cloning are checked. All80 original features remain bit-identical across random, weak and silent spectra.

An exact21,184,704-byte packet/PCM/feature/reset trace matches the accepted control across12/24/32kbps,10/20ms,modes0/1, silence/noisy chirp/noise and1/2/6-frame losses/recovery. The cache-overlay full release suite passes96 checks, including existing active-neural allocation checks. A subsequently added converter-specific allocation test also passes; that is97 distinct native checks, with four final allocation tests passing. Only the standalone test source changed after timing, not the measured decoder/library sources or executables.

Eight interleaved AB/BA measured blocks follow one warmup block. All raw cases pair by source signal, rate, duration, harmonic mode, frame and operation. The table gives the median of eight block-level medians of **cache minus accepted per-call differences**:

|Operation|10ms difference µs|20ms difference µs|
|---|---:|---:|
|Received|+0.0625|+0.0630|
|First loss|+0.0938|+0.1253|
|Continued loss|+0.0420|+0.0833|
|Recovery|+0.0630|+0.0833|

There are27,648 paired measured calls /55,296 timed implementation calls. Individual block differences range below and above these values; all are retained in the JSON. These synthetic wall timings do not certify a25µs trained-model budget or a worst-case deadline. The target-energy head and actual post-MDCT conversion are not active in these cache-only decoder timings. Converter-specific allocation and numerical tests are separate.

The earlier overlapping standalone timing ranges likewise cannot establish that16 exponentials are free. An integrated candidate/control measurement is required after a real model exists.

## Native–PyTorch proof

A short Colab **CPU-only** session runs five autograd/action tests and reads32 actual Rust converter fixtures covering480/960 bins, silence, weak energies and extreme finite outputs. PyTorch2.11.0+cpu matches the native gains with maximum absolute difference **5.960464477539063e-08**, below the fixed5e-6 threshold. This is numerical tolerance parity of the converter, not a bit-exact trained-model inference claim.

The tests also check the wrong-side saturation gradient, irrelevance of unattainable teacher extremes, fade identity, per-record bandwidth masks/equal weighting, and equal losses for gain-ratio/energy outputs representing the same action. The owned CPU session was stopped immediately after proof retrieval. Displayed balance stayed1020.43CU, active0 and rate0. The rounded display does not establish an exactly zero charge; no GPU or model training was used.

## Fullband metadata reservation

There is now a concrete alternative to consumed VCTK speakers: [OpenSLR83](https://www.openslr.org/83/). Its authors describe48kHz mono16-bit PCM and its publisher supplies a [CC BY-SA4.0 license](https://www.openslr.org/resources/83/LICENSE); see the [original dataset paper](https://aclanthology.org/2020.lrec-1.804/). It is reserved for independent evaluation only, with source notices/license retained alongside any future waveform archive.

The complete17,877-row publisher index is hashed before selection. Its120 prefix/user pairs collapse to32 five-digit user IDs, with27 IDs recurring across prefixes. Rather than assuming those anonymous numbers are globally unique, [the selector](../tools/freeze_plc_fullband_holdout.py) groups each five-digit ID conservatively across all prefixes, selects12 hash-ranked groups, one eligible hash-ranked prefix per group, and two hash-ranked15–40-word utterances. The [frozen reservation](neural-plc-target-energy-fullband-holdout.json) contains24 utterances and all source/index/license hashes. It contains no waveforms or waveform-derived scores.

No SLR83 source appears in the documented prior Snacka corpus inventory. This establishes a new documented source and conservative published-ID groups; anonymous corpus IDs cannot certify biometric nonoverlap with VCTK or LibriSpeech. Actual48kHz mono headers and file hashes must be verified after the sole candidate is selected. A header/source failure invalidates the binding; it is not permission to replace unfavorable test clips after quality unblinding.

The independent design includes SLR83 at original48k and a deterministic16k source-bandwidth view of the same clips, plus fresh LibriSpeech16k and RixVox16k identities. Selection rules/exclusions for those new16k identities are fixed in the method; their concrete metadata binding is still outstanding. This does not create Swedish fullband coverage.

## Remaining execution sequence

1. Implement the versioned raw-label exporter and four-fit driver around the tested primitives. Re-export raw labels/last powers, generate and hash the within-speaker50% English bandwidth transforms, finish the prior-exposure and fresh16k metadata binding, and seal the complete dataset/code binding before fitting. This delivery contains no new training entrypoint.
2. Fit exactly four80→64→64→17 models, with matched initialization, fixed1,920 updates and a final-step checkpoint: gain-ratio/target-energy × original/balanced bandwidth. No epoch/seed search. Evaluate phase-off/on applications of every fit, including failures.
3. Apply complete native validation guards against **both classical PLC and installed PR31**, by language/bandwidth and bitrate strata. Rank only admitted candidates; test exactly one on the newly bound independent groups, with no fallback or retuning. Only passing independent quality and native allocation/footprint/CPU/compatibility gates permit replacement.
4. Keep local pitch search and GRU outside this study.

The overlay is reproducible without modifying the installed codec:

```sh
python3 tools/build_plc_target_energy.py /absolute/new/staged
cargo test --offline --release --manifest-path /absolute/new/staged/cache/Cargo.toml --target-dir /absolute/new/staged/cache/target
cargo build --offline --release --examples --manifest-path /absolute/new/staged/accepted/Cargo.toml --target-dir /absolute/new/staged/accepted/target
cargo build --offline --release --examples --manifest-path /absolute/new/staged/cache/Cargo.toml --target-dir /absolute/new/staged/cache/target
python3 tools/measure_plc_target_cache.py /absolute/new/staged /absolute/new/measurements
# In a CPU environment with PyTorch:
python3 tools/test_plc_target_energy_loss.py --parity /absolute/new/measurements/analytic-gain-parity.bin --report /absolute/new/torch-proof.json
```

Use fresh absolute output directories and the explicit target directories shown above. No `src/`, installed weights, Phase1, Opus or runtime dependencies are changed in this protocol/preflight delivery.

The private Drive proof archive retains all raw timing CSVs, exact executables/source overlays, test logs, converter fixtures and complete SLR83 metadata/license. Full readback verifies2,341,632 bytes, SHA256 `0b47a7ff106742b6bfc2c7f9fd3c11babce3f2056c8c46199f4068cc0d56ebcb`. Its whitelisted80 files exclude credentials, signed URLs and speech waveforms.
