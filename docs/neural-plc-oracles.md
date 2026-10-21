# Native PLC oracle diagnostic

Baseline source: `cf5ed449a0e3fb8ff168d290f843d0ed460aa4fb`.
This is a noncausal diagnostic, not a replacement model or a new acceptance
test. The 24 previously scored clips produce 5,184 cases per variant over the
original bitrates, durations, modes, noise conditions and loss masks. Speaker
and sentence groups retain their original identities. See
`neural-plc-oracle-protocol.json` for the matrix and provenance fingerprints.

## What is optimized

For a missing frame, `p` is its native PLC prediction and `t` is the MDCT
target obtained by receiving the actual packet in the uninterrupted decoder,
with the Phase 1 postfilter enabled. The target is **codec output**, including
the selected noise condition, rather than a separate clean-speech target.
Per-band statistics are `pp=sum(p²)`, `pt=sum(p*t)` and `tt=sum(t²)`.

| Variant | Gain before clipping | Applied range |
| --- | --- | --- |
| Baseline | 1 | identity |
| Current | installed 60-epoch MLP | 0.7875–1.2125 |
| MDCT oracle | `pt/pp` | 0.7875–1.2125 |
| Envelope oracle | `sqrt(tt/pp)` | 0.7875–1.2125 |
| Combined oracle | numerical global minimum of the functional below | 0.7875–1.2125 |
| Wide combined oracle | same functional | 0.5–1.5 |

When `pp=0`, scaling cannot create the missing spectrum; identity is used.
The same history, confidence, fade and nonzero-prediction rules as main decide
whether any correction can be applied. Each native oracle follows its own
decoder state through loss and recovery. The separate fixed-prediction table
holds the baseline prediction and eligibility mask constant across gains.

Let `T=max(sum_b(pp_b+tt_b),1e-10)` and `(a,b,c)=(pp,pt,tt)/T`. The combined
functional is

```text
sum_b(a_b*g_b² - 2*b_b*g_b + c_b)
+ 0.01 * mean_b((g_b-1)²)
+ 0.10 * mean_b(log((a_b*g_b²+0.001)/(c_b+0.001))²).
```

Language weights are constant for a record and do not change its minimizing
gain. The scalar problem can be nonconvex. Interval branch-and-bound uses
independent quadratic/log lower bounds and a Taylor lower bound that retains
cancellation between opposing gradients. The pre-f32 objective gap is bounded
by `1e-9` per band, apart from floating-point rounding. The tiny final error
floor in the training metric is below this precision. Applied gains are f32;
their numerical error is checked separately against NumPy/SciPy solutions.

The original trainer optimizes **raw gains `q` in 0.5–1.5**, then deployment
applies **`G=1+0.425*(q-1)`**. Primary combined oracles evaluate the functional
at the physical applied gain `G`. This is not the original strength path.
The fixed-prediction report therefore also solves the original raw-gain
training optimum and applies 0.425 afterward. It reports both functionals.
These diagnostics evaluate f64 statistics. The current model's raw gain is
inferred from its deployed f32 gain, so its diagnostic functional is not a
bitwise reconstruction of a PyTorch checkpoint score.
The extractor-compatible subset additionally limits horizons to 1–6 and uses
the original nonquiet prediction/target thresholds; these are heldout records,
not training examples.

## Measurement and interpretation

All six variants receive fresh waveform and STOI/ESTOI/LSD scoring within the
same build. Archived scores are a historical comparison, not a cross-build
bitwise scalar-error contract. Initial attempts to reuse archived quality
values stopped when tiny scalar-error differences appeared; those attempts
are retained separately and excluded from the final matrix.

Each replay checks diagnostic baseline/current PCM against ordinary native
`Decoder` calls bit for bit. Input hashes match the original selection,
uninterrupted output is unchanged, output is finite and bounded, and received
PCM is unchanged beyond the existing 120-sample recovery overlap. Both CPU
shards execute the same binary bytes. Only complete, disjoint shards with the
same source/binary/protocol hashes can be merged.

The archived Swedish inputs were produced by the original preprocessing host.
The Colab resampler did not reproduce their bytes. Native FLAC files were
therefore streamed through that host's RAM, and clean/noise20/noise10 PCM was
frozen only after all 36 archived fingerprints matched exactly. Temporary
compressed chunks were uploaded and deleted; no local raw corpus was retained.
The canonical PCM archive can subsequently move directly from Drive to Colab.
Already completed 48 kHz English jobs were retained after a recorded protocol
migration: their inputs, binary and numerical scoring branch were unchanged,
and every cached input fingerprint and statistics hash was checked.

Results distinguish:

- **Native waveform outcomes:** aggregate lost-sample error energy, onset and
  recovery are measured against uninterrupted codec output. STOI/ESTOI/LSD
  instead use the original clean PCM before noise is added, matching the
  existing quality evaluator. Their deltas cover the whole clip, including
  received frames. Confidence intervals resample speaker/sentence groups;
  Swedish sentence groups do not establish speaker-independent generalization.
- **Fixed-prediction headroom:** the same baseline MDCT prediction under the
  learned and oracle gains, including the original training-strength path.
- **Correlation and conflict:** quartiles of `rho=pt/sqrt(pp*tt)` by band,
  language, condition and eligibility; undefined zero-energy cases are counted
  and excluded. An additional table requires relative band energy above `1e-6`
  in both signals. Gradient opposition and clamp occupancy are recorded.

Quartiles describe loss-frame occurrences in this matrix; frames and masks
are not independent speakers. The Swedish source is native 16 kHz, while the
added noise can contain higher frequencies. Interpret clean and noisy
high-band correlations separately.

Only the MDCT oracle minimizes MDCT error. A combined optimum is not an SNR
upper bound. Even the exact per-frame MDCT optimum is not a global waveform
optimum through overlap-add and decoder-state feedback. Every oracle sees
future information, so its gap does **not** establish how much a causal MLP
can learn. None of these objective measurements establishes listener preference.

## Reproduction

`build_plc_oracle.py` creates a fresh, dependency-free diagnostic source copy.
It derives its private hook from the current concealment body and fails if
the expected anchors change. The shipped source/API and `src/opus/` are not
edited. Offline solvers may allocate; the production allocation contract is
unchanged.

```sh
python tools/build_plc_oracle.py /tmp/fresh-oracle
# Optional: on the original preprocessing host, where the corpus is accessible.
python tools/freeze_plc_oracle_inputs.py selection.json reference.csv canonical \
  --language sv
# Transfer canonical/ to the evaluation hosts, or use its verified Drive archive.
python tools/evaluate_plc_oracles.py selection.json reference.csv shard0 \
  --binary /tmp/fresh-oracle/target/release/examples/oracle --task-shard 0 2 \
  --prepared-inputs canonical/manifest.json
python tools/evaluate_plc_oracles.py selection.json reference.csv shard1 \
  --binary /tmp/fresh-oracle/target/release/examples/oracle --task-shard 1 2 \
  --prepared-inputs canonical/manifest.json
python tools/merge_plc_oracle_shards.py selection.json merged shard0 shard1
python tools/summarize_plc_oracles.py merged summary.json
python tools/plot_plc_oracles.py summary.json overview.png
```

Use the exact same binary on multiple evaluation hosts. The freezer also
accepts a byte-stream loader when the original host has no local raw corpus.
Evaluation audio stays in the CPU environment; code and compact receipts can
be retained locally.
Canonical data and result archives remain in the owner's private Drive.
