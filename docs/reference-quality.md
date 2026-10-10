# Matched speech quality reference

Snacka needs substantial improvement at 12 kb/s. This comparison evaluates the
version 5 Rust codec at commit `57a5211170fddb432326b5c7d72ad16be5d35956` against a
separately installed libopus 1.6.1, with matched frame duration, packet bytes and
120-sample algorithmic delay. It changes no codec implementation or format.
The external library is an optional offline measurement oracle, never a runtime
codec, fallback or dependency of the Rust library.

## Profiles and controls

The [original adapter](../tools/opus_roundtrip.py) calls the documented float
encoder/decoder API through Python ctypes. No external codec implementation
source is copied, translated, linked into Snacka or vendored. API constants and
controls follow the [encoder API](https://opus-codec.org/docs/opus_api-1.6/group__opus__encoder.html)
and [encoder controls](https://opus-codec.org/docs/opus_api-1.6/group__opus__encoderctls.html).
The variadic control declaration specifies the fixed arguments as required by
[ctypes on Apple ARM64](https://docs.python.org/3.14/library/ctypes.html#calling-variadic-functions).

Both encoders consume the same normalized 48 kHz mono f32 input. The reference
uses restricted-lowdelay, complexity 10, CBR, DTX disabled and otherwise default
controls. Its queried lookahead must equal 120 samples. At 10/20 ms and
12/24/32 kb/s, every encoded packet must occupy exactly the same total byte
budget as Snacka, including each format's framing. Transport overhead is
excluded for both. Stream state persists throughout each utterance. Both paths
encode a final drain frame, discard exactly 120 priming samples and trim
partial-frame padding. Normal packet bytes and drain bytes match separately in
all 864 cases; the scorer refuses mismatched budgets or reported delay.

This is a comparison with one explicitly configured low-delay Opus profile.
It does not cover Opus's other applications, higher-delay modes, VBR, FEC,
packet loss or all possible encoder settings. The 120 samples exclude frame
buffering, resampling, devices and transport for both codecs. A Python reference
process is unsuitable for callback CPU comparisons; these results measure
quality only.

## Corpus and scoring

The 48 previously evaluated clips contain 24 distinct speakers from
[LibriSpeech test-clean](https://www.openslr.org/12), CC BY 4.0. Attribution:
Vassil Panayotov, Daniel Povey, Guoguo Chen and Sanjeev Khudanpur,
*LibriSpeech: an ASR corpus based on public domain audio books*, ICASSP 2015.
Original files follow `LibriSpeech/test-clean/speaker/chapter/clip.flac`;
[input hashes](measurements/2026-10-10-loss-seeded-input.sha256) identify the
complete selection. No audio or transcripts are redistributed.

FFmpeg 9.0.2 converts 16 kHz FLAC to 48 kHz f32. Each clip is evaluated clean,
with seeded Gaussian noise at 20 dB and at 10 dB, for both frame durations and
all three bitrates: 864 pairs. No noise samples clipped. These are read-English
recordings with synthetic noise, not unseen evaluation data, multilingual
conversation or real microphones. The native codec is unchanged after scoring.

[The shared scorer](../tools/evaluate_quality.py) and
[metric controls](loss.md#alignment-and-validation) use Python 3.14.8, NumPy 2.5.3,
SciPy 1.18.1 and pystoi 0.4.1. STOI and ESTOI compare against clean speech,
including when the encoder input is noisy. Waveform SNR and speech-band
log-spectral distance compare against encoder input. Definitions are in the
[quality method](quality.md#method). Each paired metric gets the same deterministic
RNG seed. An 18-case native-against-itself control has exactly zero deltas for
all four metrics. Private mismatch controls reject unequal delay and byte
budgets before writing a result table.

## Results

Each mean weights the 48 clips equally. Positive ESTOI change favors Snacka.
Neither STOI nor ESTOI is a percentage of understood words or a listening result.

| Input | Frame ms | kb/s | Reference STOI | Snacka STOI | ESTOI change |
| --- | ---: | ---: | ---: | ---: | ---: |
| Clean | 10 | 12 | 0.949883 | 0.813071 | -0.184239 |
| Clean | 10 | 24 | 0.986020 | 0.956558 | -0.056059 |
| Clean | 10 | 32 | 0.992412 | 0.978040 | -0.030346 |
| Clean | 20 | 12 | 0.960611 | 0.899147 | -0.093352 |
| Clean | 20 | 24 | 0.965667 | 0.971893 | +0.005753 |
| Clean | 20 | 32 | 0.982260 | 0.985546 | +0.002750 |
| Noise 20 dB | 10 | 12 | 0.937661 | 0.793010 | -0.163306 |
| Noise 20 dB | 10 | 24 | 0.966713 | 0.935700 | -0.053603 |
| Noise 20 dB | 10 | 32 | 0.972162 | 0.956467 | -0.030721 |
| Noise 20 dB | 20 | 12 | 0.946613 | 0.882285 | -0.086432 |
| Noise 20 dB | 20 | 24 | 0.968190 | 0.950451 | -0.032906 |
| Noise 20 dB | 20 | 32 | 0.972407 | 0.965393 | -0.013693 |
| Noise 10 dB | 10 | 12 | 0.887358 | 0.751670 | -0.090581 |
| Noise 10 dB | 10 | 24 | 0.912528 | 0.882009 | -0.028607 |
| Noise 10 dB | 10 | 32 | 0.918990 | 0.908726 | -0.013229 |
| Noise 10 dB | 20 | 12 | 0.895808 | 0.840480 | -0.040361 |
| Noise 10 dB | 20 | 24 | 0.913984 | 0.900473 | -0.014967 |
| Noise 10 dB | 20 | 32 | 0.918699 | 0.918417 | -0.002335 |

Across all 864 pairs, Snacka has lower STOI in 753 cases and lower ESTOI in 765.
The worst changes are −0.207795 STOI and −0.250269 ESTOI. It has higher spectral
distance in 854 cases, with a worst increase of 19.809244 dB. Waveform SNR is
lower in 36 cases, with a worst change of −1.548653 dB. All scores, codec labels,
input hashes, budgets, delays and reference provenance are retained in the
[raw table](measurements/2026-10-10-quality-reference.csv).

For clean 20 ms speech at 24/32 kb/s, Snacka's mean STOI is slightly higher,
by 0.006226/0.003286. Its mean spectral distance is still worse by
3.572547/2.830518 dB. At clean 12 kb/s the STOI deficit is 0.136812/0.061463
for 10/20 ms. Positive waveform-SNR means alongside worse spectral and
intelligibility proxies show why selecting models by reconstruction error alone
is insufficient. This is evidence to investigate perceptual allocation and
spectral-envelope coding at low bitrate; it does not establish their solution.
Blind listening, independent speech, real noise and physical mobile measurements
remain acceptance requirements. No competitive-quality claim is made.

## Reproduce and verify

Build the native release example with Rust 1.99.0, optimization level 3,
16 codegen units and overflow checks enabled, as specified by Cargo's release
profile. Use the same current example source when evaluating an older native
library: the scorer now requires both drivers to report delay. The core sources
and measurement tools are recorded in the
[source manifest](measurements/2026-10-10-quality-reference-source.sha256).

Install quality-scoring packages outside the library and supply your own
installed reference library explicitly:

```sh
cargo build --locked --release --example roundtrip
export SNACKA_REFERENCE_OPUS_LIBRARY=/path/to/libopus.so
OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 python tools/evaluate_quality.py \
  corpus matched.csv --roundtrip target/release/examples/roundtrip \
  --baseline tools/opus_roundtrip.py
```

Use `.dylib` on macOS or `.dll` on Windows as appropriate. There is no automatic
library download or discovery. The measured library identifies itself as
`libopus_1.6.1`; its SHA-256 is
`98dcd3cc62c6cf913b789051536ab246c56cc163c74bd8b4968ac467ec74b68b`.
Every pair records this version and binary hash. Python drives native examples
as executables and `.py` references with its own interpreter.

Before scoring, verify the adapter independently against FFmpeg using clip
`1320-122617-0003` from the same corpus. Explicit float encoding and decoding
avoid FFmpeg's default 16-bit conversion. With the same installed library,
[the verification tool](../tools/check_opus_reference.py) requires byte-identical
aligned float output for all six profiles:

```sh
ffmpeg -v error -y -i corpus/1320-122617-0003.flac -ar 48000 -ac 1 -f f32le input.f32
python tools/check_opus_reference.py input.f32 verification.csv
```

All six profiles match exactly over 301,680 samples in the
[verification table](measurements/2026-10-10-reference-verification.csv), which
records PCM hashes and reference identity. A different library build, version or
numerical path must be checked rather than assumed equivalent. Additional local
adapter checks cover 48 empty/partial-frame/near-silence profiles and seven
invalid-input/configuration cases; failures preserve existing output files.
No reference code runs in ordinary Cargo builds or codec callbacks.
