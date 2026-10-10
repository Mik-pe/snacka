# Speech quality evaluation

This is an experimental codec. Objective measures guide development; they do
not establish subjective quality, conversational acceptance or superiority over
other libraries. Listening tests and realistic noisy conversations remain
necessary.

## Adaptive bandwidth comparison

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
  --baseline /path/to/version-2/roundtrip
```

The [source manifest](measurements/2026-10-10-bandwidth-source.sha256) records
codec, benchmark, test and evaluation files. CPU evidence is in the
[bandwidth table](measurements/2026-10-10-apple-m5-bandwidth.csv), using the same
environment and limits as earlier measurements. At 20 ms / 24 kb/s, median
encode/decode throughput is 8.56/5.07 microseconds, versus 6.40/4.91 in the
previous implementation's run. Exploring bandwidth adds encoding cost; these
synthetic shared-desktop measurements do not establish mobile battery usage,
callback deadlines or a matched-quality competitor advantage.

## Format and robustness checks

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
