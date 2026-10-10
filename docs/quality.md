# Speech quality evaluation

This is an experimental codec. Objective measures guide development; they do
not establish subjective quality, conversational acceptance or superiority over
other libraries. Listening tests and realistic noisy conversations remain
necessary.

## Metric reproducibility

The optional scorer now seeds NumPy for every paired comparison, independently
of encoder/decoder label, to control pystoi 0.4.1's random dither of zero-energy
normalization rows. Identical PCM must receive identical scores. The seed and
repeat controls are documented in [packet-loss evaluation](loss.md).

The historical ESTOI tables below were produced without this control. Small
ESTOI differences and counts of tiny regressions can therefore include scoring
variation. Their raw tables and historical source hashes are retained; they
should not be read as deterministic results. Waveform SNR, STOI and spectral
distance calculations are unaffected by this RNG issue. Listening acceptance
remains necessary.

## Version 5 bounded pulse exchanges

The encoder refines each pulse shape with at most two exchanges after projection
and greedy assignment. Each move preserves the pulse count and input-aligned
signs, and must strictly increase normalized correlation. Gain precision,
allocation, packet capacity, transform, delay and decoder are unchanged. The
version 5 bitstream needs no extension. The search is a bounded heuristic,
not a globally optimal solution. Its additional encoder CPU is reported in
[paired speech measurements](cpu.md).

The baseline is commit `c450d0424d4795076ab5d154f92523b736e17ec8`. Both release
executables use the same toolchain and frame/bitrate settings. All 864 paired
cases have equal normal-frame and drain byte counts. Four tables cover 48
LibriSpeech test-clean clips from 24 distinct speakers:

- [Development](measurements/2026-10-10-quality-refinement-development.csv):
  eight previously evaluated clips from four speakers; input hashes remain in
  the [original manifest](measurements/2026-10-10-speech-v2-input.sha256).
- [Previously evaluated speakers](measurements/2026-10-10-quality-refinement-holdout.csv):
  sixteen clips from eight speakers, with the
  [existing input manifest](measurements/2026-10-10-quality-holdout-input.sha256).
- [Additional previously evaluated speakers](measurements/2026-10-10-quality-refinement-additional.csv):
  sixteen clips from eight speakers used in earlier encoder experiments;
  [input hashes](measurements/2026-10-10-refinement-additional-input.sha256).
- [New unseen speakers](measurements/2026-10-10-quality-refinement-unseen.csv):
  eight clips from four speakers, selected before scoring the final algorithm;
  [input hashes](measurements/2026-10-10-refinement-unseen-input.sha256).
  Select the first two FLAC members of each of the first four additional
  speakers in official archive order, excluding the twenty previously evaluated
  speakers. Their IDs are 908, 7021, 8224 and 4507. The algorithm is unchanged
  after observing their scores.

Attribution, CC BY 4.0 licensing, archive paths and the official LibriSpeech
mirror are as specified below. No audio or transcripts are redistributed.
The method below is unchanged: clean speech, seeded Gaussian noise at 20/10 dB,
10/20 ms frames and 12/24/32 kb/s. No added-noise samples clip. STOI and ESTOI
compare with clean speech; waveform SNR and spectral distance compare with the
encoder input. Each table retains all input hashes, byte counts and scores.
Reproduce with `tools/evaluate_quality.py` and a separately built baseline.
There is no packet loss in this comparison.

| New unseen condition | Mean SNR change | Mean STOI change | Mean ESTOI change | Mean spectral distance change |
| --- | --- | --- | --- | --- |
| Clean | +0.014011 dB | +0.000278 | +0.000260 | −0.006760 dB |
| Noise at 20 dB | +0.007523 dB | +0.000191 | +0.000162 | −0.014140 dB |
| Noise at 10 dB | +0.001806 dB | +0.000381 | −0.000003 | −0.019806 dB |

The reconstruction-error improvement is small. No waveform-SNR comparison
regresses at the table's six-decimal precision. Other metrics are mixed:
262 STOI, 285 ESTOI and 180 spectral-distance comparisons regress across the
864 cases. Worst changes are −0.003023 STOI, −0.005715 ESTOI and +0.058671 dB
spectral distance. Means weight each clip/duration/bitrate equally. Positive
mean correlation or SNR does not establish audible or intelligibility benefit.
This remains read English with synthetic noise, not multilingual conversation,
double-talk or realistic microphone/background conditions. Listening and
competitor-quality acceptance remain open.

All 43 local debug/release tests pass. The new direct vector tests independently
sum correlation, integer norm and pulse magnitudes, cover signs, zero input,
maximum pulse count and deterministic equal-score choices. Existing independent
packet/cosine checks and the counting allocator remain active; both durations
and both spectral models allocate zero times during processing. A requested
30-second AddressSanitizer packet fuzz run with seed 6716 completes 2,674
inputs without a reported failure. Source checks pass for iOS, iOS Simulator
and Android; installed-device behavior is unverified. Codec, tests and evaluator
sources are recorded in the
[source manifest](measurements/2026-10-10-refinement-source.sha256).

## Version 5 pulse-priority optimization

Caching a band's marginal pulse cost reduces repeated allocation calculations
without changing the mathematical allocation or format. The
[paired speech CPU evaluation](cpu.md) compares against version 5 at commit
`8089bc873d5d6122ee49a37cdbaea5e8bc13ac19`. All 48 full-clip traces at both durations
and 12/24/32 kb/s match packet and PCM bytes exactly. This optimization makes
no new speech-quality claim; the improvements and regressions below remain
relevant to version 5.

## Version 5 normalized pulse bands

A packet can now select four band gains and normalized signed-pulse vectors in
place of the single-gain scalar model. Allocation derives from gain codes and
byte budget using bounded integer priorities. The encoder compares both models
and 8/12/24 kHz bandwidths by full-spectrum reconstruction error. The native
format is version 5; legacy packets are rejected. Frame duration, PCM rate,
framing overhead, delay and packet byte budgets are unchanged. See the
[format specification](bitstream.md) for the independent signed-multiset code.

The baseline is version 4 at commit
`023a8b61414e03a527e9212766f37b3dd63dcdea`. Equal toolchains and evaluation settings
produce equal normal-frame and drain byte counts in every comparison. The three
raw tables contain 576 paired cases from 32 clips and 16 distinct speakers:

- [Development](measurements/2026-10-10-quality-pulse-development.csv): eight
  clips from the original four speakers.
- [Previously evaluated additional speakers](measurements/2026-10-10-quality-pulse-holdout.csv):
  sixteen clips from eight speakers already measured for versions 3 and 4.
- [New unseen speakers](measurements/2026-10-10-quality-pulse-unseen.csv): eight
  clips from four further speakers, selected before evaluating the final pulse
  algorithm. Take the first two FLAC members of each of the first four new
  speakers in official archive order, excluding the original twelve speakers.
  Their IDs are 1995, 1284, 5142 and 8455. The algorithm was not tuned after
  observing their scores. A subsequent cache optimization preserves decoded PCM
  and frame/byte counters exactly in twelve real-speech cases.

Original FLAC hashes for the new inputs are in the
[unseen-input manifest](measurements/2026-10-10-pulse-unseen-input.sha256).
Attribution, CC BY 4.0 licensing, archive paths and the official mirror are as
specified below for LibriSpeech. No audio or transcripts are redistributed.
The method below is unchanged: clean speech plus seeded Gaussian noise at
20/10 dB, 10/20 ms and 12/24/32 kb/s. No added-noise samples clipped. Each row
records the input hash, matched byte counts, model/bandwidth counts and all
metric values. Reproduce with `tools/evaluate_quality.py` and a separately built
version 4 baseline executable as described below. There is no packet loss here.

| New unseen condition | Mean SNR change | Mean STOI change | Mean ESTOI change | Mean spectral distance change |
| --- | --- | --- | --- | --- |
| Clean | +1.095 dB | +0.017274 | +0.026062 | −2.040 dB |
| Noise at 20 dB | +0.630 dB | +0.014586 | +0.018897 | −3.142 dB |
| Noise at 10 dB | +0.211 dB | +0.007340 | +0.004842 | −4.298 dB |

No paired waveform-SNR or speech-band spectral-distance score regresses across
576 cases; some are unchanged. Intelligibility proxies are mixed: 69 STOI and
83 ESTOI comparisons regress, with worst changes −0.020156 and −0.021975.
Among the 144 new unseen cases, 21 STOI and 23 ESTOI scores regress, including
three and two clean cases respectively. Means give equal weight to each case.
Reducing spectral error does not guarantee better intelligibility. These results
support continued development, not a listening, multilingual, conversational
or competitor-quality acceptance claim.

The [CPU table](measurements/2026-10-10-apple-m5-pulse.csv) uses the same Apple M5,
Rust release flags and synthetic two-tone throughput method as earlier runs.
At 20 ms / 24 kb/s, median encode/decode batch averages are 19.88/8.70 microseconds,
compared with 9.39/5.62 in the recorded version 4 run. At 10 ms / 24 kb/s they
are 8.24/2.59, compared with 4.66/2.77. Exploring pulse shapes costs additional
encoding CPU. Workload-dependent model selection and uncontrolled scheduling
make these historical comparisons approximate; quality is not matched to the
external codec benchmark. Batch percentiles are not individual callback tails,
installed mobile deadlines or battery evidence.
The [source manifest](measurements/2026-10-10-pulse-source.sha256) records codec,
tests, benchmark, fuzz and evaluation sources.

All 41 local debug/release tests pass. Independent packets cover every supported
pulse-model byte budget and all six bandwidth/duration combinations, checked
against double-precision inverse cosine sums. Mixed signed multisets, exact cost
tables, allocation bounds, invalid gains and scalar/pulse switches with nonzero
overlap are covered. The counting allocator exercises both models at both
frame durations and observes zero processing allocations. A new AddressSanitizer
packet fuzz run, seed 6716 and requested 30-second limit, completes 4,953 inputs
without a reported failure. It reaches both models with repaired checksums,
primes four frames and tests twelve-frame loss bursts. iOS, iOS Simulator and
Android source checks pass. Installed-device behavior, realistic loss quality
and listening acceptance remain open.

## Version 4 compact framing comparison

Version 4 removes the fixed two-byte magic and moves version/profile and
sequence to the first two bytes. The transport negotiates codec identity before
processing packets. CRC-16 is retained and covers version/profile, sequence and
the complete payload. A complete packet slice remains
the API boundary. There is no stream scanner. The total bitrate, frame duration,
PCM rate, transform delay and quantizer are unchanged; two additional bytes per
packet are available to spectral coding. The largest coefficient count is now
83, with the expanded exact cardinality tables checked independently.

At 10 ms / 12 kb/s, framing/checksum overhead falls from 6/15 bytes (40%) to
4/15 bytes (26.67%). Payload grows from 9 to 11 bytes, a 22.22% increase.
At 20 ms / 12 kb/s, it grows from 24 to 26 bytes. These are packet-format
calculations, not transport savings: normal and final-drain byte counts are
unchanged in every measured pair.

The historical baseline is commit `f66d2c8825e8ccc828f2be1bb6933d12b116adcd`.
The same corpora and method below yield 432 paired comparisons: the
[development table](measurements/2026-10-10-quality-framing-development.csv)
and [held-out table](measurements/2026-10-10-quality-framing-holdout.csv).
The additional speakers remain separate from the development set; they have
already been evaluated for version 3, so this is not a newly unseen test set.
All 432 waveform-SNR and speech-band spectral-distance comparisons improve.
The held-out clean means by duration and bitrate show the largest benefit at
the lowest bitrate and shorter duration:

| Duration | Bitrate | Mean clean SNR change | Mean clean STOI change |
| --- | --- | --- | --- |
| 10 ms | 12 kb/s | +1.502 dB | +0.034071 |
| 10 ms | 24 kb/s | +0.679 dB | +0.008564 |
| 10 ms | 32 kb/s | +0.615 dB | +0.005012 |
| 20 ms | 12 kb/s | +0.540 dB | +0.009941 |
| 20 ms | 24 kb/s | +0.241 dB | +0.003172 |
| 20 ms | 32 kb/s | +0.210 dB | +0.001486 |

| Held-out condition | Mean SNR change | Mean STOI change | Mean ESTOI change | Mean spectral distance change |
| --- | --- | --- | --- | --- |
| Clean | +0.631 dB | +0.010374 | +0.014303 | −0.381 dB |
| Noise at 20 dB | +0.503 dB | +0.010401 | +0.012690 | −0.715 dB |
| Noise at 10 dB | +0.261 dB | +0.009048 | +0.007710 | −0.916 dB |

All 96 held-out clean STOI/ESTOI cases improve. Across both sets, 12 STOI and
30 ESTOI cases regress; this includes one clean development case for each
metric. The largest decreases are −0.006196 STOI and −0.009510 ESTOI. Complete
results remain in the tables. Better reconstruction error does not guarantee
better intelligibility or perceived quality.

The [compact-framing CPU table](measurements/2026-10-10-apple-m5-framing.csv)
uses the same toolchain, hardware and benchmark method as the version 3 run.
At 20 ms / 24 kb/s, median encode/decode throughput is 9.39/5.62 microseconds,
versus the previously recorded 8.56/5.07. Additional coefficients require more
work. Shared-desktop scheduling is uncontrolled; 10 ms encoding in this run
has substantial batch-average tail variation. These runs do not establish
callback deadlines, mobile battery consumption or a matched-quality competitor
advantage. The [source manifest](measurements/2026-10-10-framing-source.sha256)
records processing, tests and offline evaluation sources.

All 34 local debug/release tests pass. New tests reject original version 3
silence packets and out-of-range packet lengths without consuming history,
and reject every contiguous run of 1–16 flipped bits at both packet-budget
endpoints for all six profiles. These are specified corruption patterns, not
exhaustive validation of arbitrary multi-bit errors. CRC is not authentication.
Independent cosine checks now reach the expanded coefficient counts. Existing
sequence-wrap, bitrate-change, concealment and allocation tests continue to pass.
A requested 30-second AddressSanitizer packet fuzz run with seed 6716 completes
5,940 inputs without a reported failure. iOS, iOS Simulator and Android source
checks pass; installed-device behavior and listening acceptance remain open.

## Version 3 adaptive bandwidth comparison

Version 3 compares three coded spectral domains within each existing byte budget.
The PCM rate, transform length, frame duration, delay and amplitude quantizers
are unchanged. The baseline is version 2 at commit
`64744bf9d967bcb281a9b56ca85a3f0022997f0b`. Both executables use the same release
toolchain and emit equal normal-frame and drain byte counts in every paired case.
No fallback implementation is present in the library.

Inputs are 24 LibriSpeech test-clean utterances from 12 speakers. The original
eight clips from four speakers form the
[development table](measurements/2026-10-10-quality-bandwidth-development.csv).
The [held-out table](measurements/2026-10-10-quality-bandwidth-holdout.csv) uses
the first two FLAC members from each of eight additional speakers in archive
order, excluding the original four. Files were chosen before this comparison;
codec choices were unchanged during held-out evaluation. These are read-English
recordings, not multilingual conversations, double-talk or real background noise.

The corpus is [LibriSpeech](https://www.openslr.org/12), CC BY 4.0. Attribution:
Vassil Panayotov, Daniel Povey, Guoguo Chen and Sanjeev Khudanpur,
*LibriSpeech: an ASR corpus based on public domain audio books*, ICASSP 2015.
Original hashes are recorded for the
[development inputs](measurements/2026-10-10-speech-v2-input.sha256) and
[held-out inputs](measurements/2026-10-10-quality-holdout-input.sha256).
Archive paths follow `LibriSpeech/test-clean/speaker/chapter/clip.flac` where the
clip ID is `speaker-chapter-utterance`. The official mirror used was
`https://openslr.trmal.net/resources/12/test-clean.tar.gz`. No audio or transcripts
are redistributed in the repository.

## Method

FFmpeg 9.0.2 converts original 16 kHz mono FLAC to 48 kHz little-endian f32 PCM.
The script evaluates clean speech and the same independently seeded 20/10 dB
Gaussian-noise conditions described in [measurements](measurements.md).
Both 10/20 ms durations and 12/24/32 kb/s are evaluated: 432 paired cases.
Decoded output discards exactly 120 priming samples and retains the final drain
prefix before removing partial-frame padding. There is no packet loss in this
comparison. No samples clipped during noise addition.

- Waveform SNR compares output with the PCM supplied to the encoder, including
  noise when present. It measures ordinary reconstruction error over the clip.
- STOI and ESTOI compare output with the original clean waveform, even in noisy
  conditions. They estimate intelligibility of the entire noise-plus-codec path.
  The optional evaluator uses [pystoi](https://github.com/mpariente/pystoi) 0.4.1.
  See [Taal et al.'s original STOI paper](https://ceestaal.nl/Taal%282010%29.pdf).
  Neither score is a listening result or a percentage of understood words.
- Log-spectral distance compares output with encoder input. It uses 1,024-sample
  Hann windows, a 480-sample hop and FFT bins between 150 and 8,000 Hz. Magnitudes
  are floored at −60 dB relative to each reference frame's peak. Frames below
  −40 dB RMS relative to the reference clip's loudest frame are excluded. Average
  the per-frame RMS log-magnitude difference in dB; lower is better. This
  evaluation intentionally covers speech frequencies and does not evaluate
  full-band music quality.

Means below give equal weight to each clip/duration/bitrate combination. The
script treats insufficient-speech metric warnings and non-finite scores as
errors. Exact PCM hashes, byte counts, bandwidth frame counts and individual
scores are in the CSV files.

| Held-out condition | Mean SNR change | Mean STOI change | Mean ESTOI change | Mean spectral distance change |
| --- | --- | --- | --- | --- |
| Clean | +1.251 dB | +0.016920 | +0.023298 | −0.819 dB |
| Noise at 20 dB | +0.908 dB | +0.015077 | +0.018197 | −1.199 dB |
| Noise at 10 dB | +0.356 dB | +0.015117 | +0.014309 | −1.637 dB |

All 432 waveform-SNR and spectral-distance comparisons improve. All 144 clean
comparisons improve STOI and ESTOI. Noise exposes regressions: four held-out
STOI cases regress, worst −0.002568, and eight held-out ESTOI cases regress,
worst −0.016368. The development set has four noisy ESTOI regressions,
worst −0.002636. The full tables retain these cases. These small and mixed
intelligibility-proxy changes need listening evidence before a perceptual claim.

## Reproduction

Use Python 3.14.8 with NumPy 2.5.3, SciPy 1.18.1 and pystoi 0.4.1. These are
offline evaluation dependencies only; normal Cargo builds do not use them.
Build the historical baseline executable separately from its exact commit and
place original FLACs in the corpus directory.

```sh
python3 -m venv /tmp/snacka-quality
/tmp/snacka-quality/bin/python -m pip install -r tools/quality-requirements.txt
cargo build --release --locked --example roundtrip
PYTHONDONTWRITEBYTECODE=1 OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 \
  /tmp/snacka-quality/bin/python tools/evaluate_quality.py corpus results.csv \
  --roundtrip target/release/examples/roundtrip \
  --baseline /path/to/baseline/roundtrip
```

The [version 3 source manifest](measurements/2026-10-10-bandwidth-source.sha256) records
codec, benchmark, test and evaluation files. CPU evidence is in the
[bandwidth table](measurements/2026-10-10-apple-m5-bandwidth.csv), using the same
environment and limits as earlier measurements. At 20 ms / 24 kb/s, median
encode/decode throughput is 8.56/5.07 microseconds, versus 6.40/4.91 in the
previous implementation's run. Exploring bandwidth adds encoding cost; these
synthetic shared-desktop measurements do not establish mobile battery usage,
callback deadlines or a matched-quality competitor advantage.

## Version 3 format and robustness checks

All 31 local debug/release tests pass, including all six profiles and every
supported packet byte budget checked against independently constructed entropy
packets and double-precision inverse cosine sums. Gain boundaries and both
amplitude quantizers are covered. Dense spectral signals force bandwidth changes
without stream reset and retain content above 12 kHz. Invalid versions and
profiles preserve output and persistent history. Both frame durations retain
zero allocations over 1,000 processing repetitions.

A new AddressSanitizer packet fuzz run, seed 6716 and requested 30-second limit,
completes 6,262 inputs without a reported failure. It mutates all three bandwidth
identities with repaired checksums, primes four received frames and exercises
twelve-frame loss bursts. The entropy primitive is unchanged. Mobile source
checks cover iOS, iOS Simulator and Android, not installed-device behavior.
