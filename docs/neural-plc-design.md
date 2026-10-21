# Neural PLC: implementation and reproduction

Baseline: `1abd3a60c2775b712508dadbae6de1b40f962ee6` (Phase 1).
Experiment date: 2026-10-21. See `neural-plc-results.json` for final acceptance.

## Runtime

Phase 2 adds a loss-only **80 → 64 → 64 → 16** MLP with int8 weights,
per-output-channel f32 scales/biases, ReLU hidden layers and f32 activations.
The 16 positive spectral gains are `1 + 0.425 * 0.5 * tanh(output)`, bounded
between 0.7875 and 1.2125. Constants occupy **11,392 bytes**; Decoder grows
from 15,992 to 16,288 bytes on aarch64 (+296 bytes). There are no added runtime
dependencies or unsafe blocks. `src/opus/` and Phase 1 weights are unchanged.

This is an envelope predictor, rather than the originally suggested GRU or
pitch-trajectory predictor. It retains the existing waveform extrapolator,
pitch, correlation confidence and hard fade. The network refines only the
private MDCT spectrum of a missing frame before the existing synthesis.
A finite received-frame history avoids synthesized-state drift. It adds no
packet bits, algorithmic delay or recurrent synthesis state.

Each of the last three received frames contributes 24 features:

- 16 relative log-band RMS values, normalized to the largest band;
- log total RMS, normalized fundamental frequency, voiced flag and bandwidth;
- energy change, nonzero-bin fraction, duration and validity.

The remaining eight controls are duration, five baseline concealment controls
(elapsed loss, missing-frame ordinal, period, correlation confidence and fade), valid
context count and last RMS. The exact scales and control ordering live in
`src/neural_plc.rs` and `Concealment::features`; the extractor uses these same
Rust functions. Band edges in Hz are
`0,200,400,600,800,1000,1300,1600,2000,2500,3200,4000,5200,6800,9000,13000,24000`.

Inference is skipped before three contiguous received observations, at high
baseline periodic confidence (>=0.985), after the original fade, or for an
all-zero prediction. Loss invalidates history when reception resumes; predicted
frames never become received observations. The decoder observes features even
when correction is disabled. `Decoder::with_neural_plc(false)` provides an
ablation. It leaves the Phase 1 postfilter enabled.

Packets and uninterrupted PCM are identical to the frozen baseline. Received
PCM is identical beyond the existing 120-sample recovery overlap. Reset,
sequence checks, malformed-packet refusal, polarity, bounded output and exact
long-loss silence retain their existing contracts. Construction may allocate;
decode, conceal and reset do not. Tests exercise eligible neural inference,
not just the startup bypass.

## Corpus and split policy

The original 180-clip, 26-minute pilot was a pipeline check. It was expanded
before training to **5,809 clips / 11.596 hours**, including approximately
9.82 training hours:

| Source / split | Clips | Groups | Seconds | Native rate |
| --- | ---: | ---: | ---: | ---: |
| FLEURS Swedish train | 2,128 | 1,233 sentence IDs | 26,907.28 | 16 kHz |
| FLEURS Swedish validation | 257 | 140 sentence IDs | 3,191.62 | 16 kHz |
| VCTK train | 2,496 | 78 speakers | 8,437.20 | 48 kHz |
| VCTK validation | 544 | 17 speakers | 1,796.19 | 48 kHz |
| VCTK test | 384 | 12 speakers | 1,412.15 | 48 kHz |

[Google FLEURS](https://huggingface.co/datasets/google/fleurs) is pinned to
`70bb2e84b976b7e960aa89f1c648e09c59f894dd`. Its official train archive is
partitioned 90/10 by a salted hash of sentence ID, grouping all takes.
Reliable speaker IDs are unavailable: Swedish speaker disjointness is
**unverified**. The Phase-1-used development split stays out of fresh training.

[CSTR VCTK 0.92](https://datashare.ed.ac.uk/items/30e7453c-9ea8-48b4-8e18-f96d0dc62928/full)
uses mic2 and up to 32 utterances per available speaker. A salted 80/10/10 hash
partitions speaker IDs before selection; unavailable p280/p315 are excluded.
No test utterances enter feature training or checkpoint selection. Manifest
FLAC hashes, decoded PCM duplicates and group boundaries were checked. Both
sources are CC BY 4.0; retain their source attribution with redistributed audio.

The independent acceptance selection contains one >=3-second utterance from
each of the 12 VCTK test speakers, plus 12 fresh official FLEURS test sentence
groups excluding all train/validation and earlier diagnostic sentence groups.
The old eight English and sixteen Swedish diagnostic clips are a separate
regression set. Selection hashes and group counts accompany the results.

Upsampling Swedish 16 kHz audio to the codec's 48 kHz input does not create
information above 8 kHz. This work establishes low-band Swedish PLC behavior;
general Swedish fullband quality remains unproven. EARS was excluded because
its license is noncommercial. NST ASR is 16 kHz; the separate higher-rate NST
synthesis corpus was not ingested.

## Native records and training

`extract_plc_dataset` decodes real packets with Phase 1 enabled, snapshots
baseline concealment at horizons 1–6, and records actual MDCT targets. No
future target or true burst length is a model input. Every horizon shares only
received past; there is no teacher-forced synthesized history. SPLC v1 binary
records contain four metadata bytes and 128 f32 values (516 bytes total):
80 model features plus 16-band prediction energy, prediction-target product
and target energy. The Rust snapshot API changes scratch only.

`extract_plc_corpus.py` chooses one deterministic combination of 12/24/32 kb/s,
10/20 ms, default/harmonic mode, clean/noise20/noise10 and 0/-12/-24 dB per
utterance. It records source hash, seed, clipping, mode and shard hashes.
Train and validation features remain separate; no test features are staged.
Stride 24 produced **470,922 training and 67,515 validation records**.

The trainer normalizes quadratic statistics by the combined prediction and
target energy with a silence floor. Schema-1 archives used target normalization;
the loader renormalizes both versions symmetrically. It optimizes normalized
MDCT quadratic error plus 0.1 log-envelope loss and 0.01 identity regularization.
It uses an equal-language objective, seeded shuffling, ten float epochs then
fake-quantized int8 weights through epoch 60, batch size 8,192, seed 20261021.
Checkpoint selection minimizes the composite validation objective. Checkpoints
include optimizer and RNG state; the current CLI trains from scratch.

The accepted training run used an L4, PyTorch 2.11.0+cu130 and 60 epochs
(12.566 seconds in the training loop, excluding staging/startup). Epoch 60 was
selected. An earlier saturated mute candidate was rejected. The QAT export
and restored checkpoint produce bit-identical quantized arrays; CPU/GPU gains
agree within 8.35e-7 on the export fixture. Rust/GPU parity is tested on 32
frozen vectors with 5e-5 tolerance.

The original gain strengths 0.25/0.5/0.75/1.0 did not satisfy all validation
gates. A documented validation-only refinement selected 0.425. Numeric gates
were unchanged. See `neural-plc-strength-refinement.json`; the candidate was
frozen before fresh acceptance. No test-driven tuning was performed.

## Reproduction and cloud storage

Raw audio, manifests, native feature shards, checkpoints and evidence are
stored privately under the owner's Drive `Snacka/neural-v2`. Uploads use the
connected Drive service; every data archive was downloaded again and checked
with SHA256. Private asset IDs are kept in that Drive folder, outside Git.
No permanent MacBook audio corpus is required. Temporary staging can be removed
after verified cloud persistence; retain model source and compact receipts.

Use CPU for acquisition and feature extraction, L4 only for training. A source
publisher may reject Colab IPs; in this run VCTK used a bounded local HTTP-range
staging fallback. Archives are split below 100 MiB for connector transfer.
Copy shards onto `/content` before training instead of reading tiny files over
Drive/FUSE. Never save OAuth tokens or expiring signed URLs in artifacts.

Example CPU workflow (Python needs numpy, soundfile and ffmpeg; evaluation also
needs scipy and pystoi):

```sh
python tools/prepare_neural_corpus.py --training --output corpus
cargo build --locked --release --example extract_plc_dataset --example plc_compare
python tools/extract_plc_corpus.py corpus features \
  --binary target/release/examples/extract_plc_dataset
python tools/train_neural_plc.py features result --epochs 60 --batch-size 8192
```

CUDA is preferred; the trainer also supports CPU and MPS. When combining separately extracted sources, merge
train/validation shard lists without name collisions and retain both source
provenance files. Do not stage test shards. Existing pinned feature archives
permit exact training reproduction without refetching publishers.

`tools/run_colab_plc_training.sh SESSION BOOTSTRAP OUTPUT` creates an L4, waits
for CUDA, launches a detached bootstrap, polls bounded status, retrieves weights,
parity, report and checkpoint, and stops its owned session through an EXIT trap.
The private bootstrap stages verified shards and writes
`/content/snacka-plc-training/status.json`. `tools/colab_download.py` streams
large session files through the CLI's authenticated Jupyter endpoint. It must
run in the CLI's Python environment and never logs credential-bearing URLs.
Direct Drive mounting still requires separate interactive authorization; an
already authenticated Colab CLI does not imply a mounted Drive.

After training, install only the reviewed generated model and parity fixture,
then validate exported Rust waveform behavior:

```sh
cargo fmt --check
cargo clippy --locked --all-targets --features opus -- -D warnings
cargo test --locked --release --features opus
python tools/evaluate_neural_plc.py selection.json results.csv \
  --binary target/release/examples/plc_compare \
  --baseline-probe BASELINE/target/release/examples/cpu_probe --quality
python tools/summarize_neural_plc.py results.csv --output gates.json
```

Each selection row has a source-relative path, corpus_root, source, language,
group_id, native_rate and flac_sha256. Keep local roots out of public receipts.
The evaluator clips 48 kHz resampling overshoots uniformly to the encoder's
[-1,1] input contract, records counts, uses independent scorer processes, and
rejects incomplete runs. It verifies external frozen-main packet/PCM traces,
baseline PLC equality and the recovery bound before scoring all six loss masks.

Acceptance is frozen in `neural-plc-acceptance.json`: loss-error improvement
>=0.25 dB per language, onset/recovery >=-0.1 dB, STOI/ESTOI >=-0.001 per
language/mode/duration, LSD <=+0.05 dB, and worst-decile loss SNR >=-0.5 dB.
Confidence intervals resample source speaker/sentence groups, not individual
frames. Validation was +0.277 dB English / +0.320 dB Swedish. Consult the final
result receipt for heldout, regression, CPU and safety results. These metrics
do not establish subjective preference or superiority to Opus.

## Measured acceptance

All frozen gates passed on 2026-10-21, including the heldout evaluation run
only after strength/checkpoint selection:

| Set | Rows | English loss-error improvement | Swedish loss-error improvement |
| --- | ---: | ---: | ---: |
| Validation | 2,592 | +0.277 dB | +0.320 dB |
| Fresh heldout | 5,184 | +0.314 dB | +0.313 dB |
| Prior diagnostic regression | 5,184 | +0.298 dB | +0.291 dB |

The fresh-test group-bootstrap 95% intervals are [0.233, 0.397] dB English
and [0.236, 0.363] dB Swedish. The >=0.25 dB gate applies to the point
estimate. Worst-decile mean delta is -0.045 dB; mean onset/recovery gains are
+0.430/+0.218 dB. Every language/mode/duration STOI/ESTOI and LSD gate passes.
This is a modest objective fidelity improvement, not evidence of subjective
preference. Eight reference/A/B WAV files and a label key accompany the private
evidence archive. They have not been assessed by blind listeners.

External frozen-main process-CPU comparisons use two fixed training clips,
three bitrates, both durations and three alternating-order repetitions:

| Host | Received decode: paired median added time | Median CPU ratio |
| --- | ---: | ---: |
| Apple M5, macOS 27 | +0.51 us/frame | 1.034 |
| Colab x86_64 Xeon 2.20 GHz | +3.92 us/frame | 1.057 |

Separate per-call wall timings at 24 kb/s/default mode cover first loss,
continued loss, received frames and recovery. First-loss p50/p95 is
17.79/28.50 us on M5 and 92.79/200.01 us on Colab. Continued-loss p50/p95 is
7.46/12.92 us and 48.93/125.22 us respectively. These include scheduler noise;
the largest observed delays and full distributions are in the result receipt.
The 25 us total lost-frame optimization target is exceeded on x86_64 and at
the M5 timing tail. Average CPU is not a per-call deadline guarantee. Actual
stack high-water and WCET have not been measured; model scratch arrays have
fixed compile-time sizes and introduce no recursion or dynamic allocation.

Mac release tests with `opus`, format and Clippy pass. Linux full release tests
and selected-candidate library/codec/allocation tests pass. CI results belong
to the eventual exact PR head and must be checked separately.

## Later phases

Phase 3 can explore an integer entropy prior, but it must avoid dependence on
unrecovered previous packets or explicitly reset/signal that state. Measure
actual bit savings and deterministic encoder/decoder CDF parity. Phase 4 can
revisit postfilter/search coupling and genuine fullband Swedish data. Neither
is part of this Phase 2 implementation.
