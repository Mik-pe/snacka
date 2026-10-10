# Measurements

## CPU method and version 2 run

Run `cargo bench --locked --bench kernels --features reference-tests` in a release
build with an independently installed libopus. Omit the feature to measure only
Snacka. Inputs, plans, workspaces and buffers are prepared before timing. Outputs
are observed through `black_box`. Audio input is a repeated two-tone synthetic
frame, not a representative speech corpus. Native decoding cycles 256 prepared
packets in sequence without resetting history.

Each operation receives 1,000 warmups. A batch is calibrated once to at least
200 microseconds, then 1,000 batches are timed and sorted. p95/p99 describe
**batch-average throughput**, not individual callback tail latency. Timer, loop,
validation and reference FFI overhead remain included.

The [version 2 raw table](measurements/2026-10-10-apple-m5-v2.csv) was captured on Apple M5,
macOS 27.0 build 26A428, aarch64, Rust 1.99.0 (`b940084d7`, 2026-09-28), and
libopus 1.6.1. Release overflow checks were enabled, with no LTO,
`target-cpu=native` or explicit SIMD. This shared desktop had neither CPU affinity
nor fixed power settings. The high percentiles show substantial scheduling
variation; they must not be presented as stable device deadlines.

Both codecs use 48 kHz mono, 10/20 ms frames and 12/24/32 kb/s. The competitor uses
restricted-lowdelay, CBR, complexity 10; queried lookahead is 120 samples. Native
packets include six framing/checksum bytes that the competing codec does not
have. Quality is not matched and no competitor quality score was measured.
This is a CPU workload comparison, not proof of codec superiority.

At 20 ms / 24 kb/s, median batch-average native encode/decode took about
6.49/4.93 microseconds; the reference took about 36.48/6.55 microseconds. These
measurements do not establish mobile battery use, perceptual quality or
real-time behavior under application load. The
[source hash manifest](measurements/2026-10-10-v2-source.sha256) records the
processing, benchmark and evaluation sources used for this measurement.
The earlier [version 1 table](measurements/2026-10-10-apple-m5.csv) and its
[source manifest](measurements/2026-10-10-source.sha256) remain historical results.

## Public speech smoke test

Three test-clean utterances from the same speaker in the
[LibriSpeech corpus](https://www.openslr.org/12), licensed CC BY 4.0, were evaluated:
`6930-75918-0000`, `6930-75918-0006`, `6930-75918-0007`. Attribution: Vassil Panayotov,
Daniel Povey, Guoguo Chen and Sanjeev Khudanpur, *LibriSpeech: an ASR corpus based
on public domain audio books*, ICASSP 2015. Files came from
`LibriSpeech/test-clean/6930/75918/` in `test-clean.tar.gz`; their original FLAC
hashes are [recorded here](measurements/2026-10-10-speech-input.sha256).
No audio or transcripts are redistributed in this repository.

Reproduce with FFmpeg 9.0.2 to convert each original mono 16 kHz FLAC into
little-endian mono f32 at 48 kHz:

```sh
ffmpeg -i input.flac -ar 48000 -ac 1 -f f32le input.f32
cargo run --release --example roundtrip -- input.f32 output.f32 24000 20
```

The example trims the initial 120-sample delay, keeps the final drain prefix and
removes padding of the last partial frame. It measures ordinary waveform SNR,
`10*log10(sum(input^2)/sum((input-output)^2))`, over the entire utterance.
This is **not a perceptual or intelligibility score** and must not be compared
with another codec without matching alignment and evaluation settings.

The historical [version 1 result table](measurements/2026-10-10-speech-smoke.csv)
spans both durations and 12/24/32 kb/s. At 20 ms / 24 kb/s it measures
8.30–9.19 dB. This exposes the baseline's considerable reconstruction error. Three clips from one clean,
read-English speaker are a smoke test, not a representative quality evaluation.
Unvoiced speech, noisy inputs, diverse speakers, multiple languages, double-talk
and burst packet loss remain necessary before a quality acceptance decision.

## Version 2 quantization comparison

The [paired results](measurements/2026-10-10-speech-v2.csv) compare version 2
against commit `ccc1dc9800003cb20a4f79b44acb44e2f2a1f4a0` using eight utterances
from four LibriSpeech test-clean speakers. Attribution and licensing are as
above. Original FLAC hashes are in the
[input manifest](measurements/2026-10-10-speech-v2-input.sha256). For clip
`speaker-chapter-utterance`, the archive member is
`LibriSpeech/test-clean/speaker/chapter/speaker-chapter-utterance.flac`.
The archive was downloaded from the official corpus mirror
`https://openslr.trmal.net/resources/12/test-clean.tar.gz`.

The offline [evaluation script](../tools/evaluate_speech.py) converts FLAC with
FFmpeg 9.0.2 and Python 3.14.8. It tests clean PCM and Gaussian white noise added at
20 and 10 dB input SNR. A separate Python `random.Random` is seeded with
`snacka-speech-6716:<clip>:<snr>` for each noisy condition; noise energy is scaled
over the whole utterance before f32 conversion. Clipping counts and hashes of
the exact test PCM are recorded. No samples clipped in this run. The reference
waveform is the noisy input itself: this measures codec distortion, not
denoising or intelligibility. Neither codec receives noise-specific settings.

Both durations and all three bitrates yield 144 paired cases. Version 2 improves
aligned waveform SNR in every measured case, by 0.094–1.423 dB. Clean speech
improves by 0.232–1.423 dB. At 20 ms / 24 kb/s, clean scores range from
9.15 to 19.43 dB, with mean paired improvement 0.458 dB. These are equal-rate
comparisons against our previous implementation, not evidence of superiority
over another library or of better perceived quality. Four read-English voices
and synthetic noise still leave conversational speech, other languages and
real background noise untested.

Build the current and baseline `roundtrip` executables separately with the same
release toolchain, place the eight original FLACs in `corpus`, then run:

```sh
cargo build --release --locked --example roundtrip
python3 tools/evaluate_speech.py corpus results.csv \
  --roundtrip target/release/examples/roundtrip \
  --baseline /path/to/version-1/roundtrip
```

Omit `--baseline` to evaluate current output alone. No corpus audio, transcripts
or decoded output are committed. The baseline executable is an external
historical measurement artifact, not an alternate library implementation.

## Correctness and robustness evidence

- Both frame durations roundtrip 300-frame streams at six target bitrates,
  including fractional byte quotas and sequence wrap. Every supported integer
  packet budget also handles dense noise, transients, DC, alternating-sign input
  and a chirp. Bitrate changes preserve history and exact cumulative quotas.
- Framing bit flips, truncated packets, invalid gain/amplitudes with valid CRC,
  wrong profiles, invalid PCM and insufficient buffers are refused without
  changing documented output or persistent history.
- Both amplitude quantizers and gain boundaries are decoded from independently
  constructed packets and checked against double-precision inverse cosine sums.
  Incompatible versions are refused before changing history. The debug and
  release suites contain 25 tests, plus the documented example test.
- Independent entropy fixtures cover 128 frames and 16,384 events, checking
  values, interval and whole/eighth-bit usage. The live libopus oracle also
  decoded all native full-budget entropy frames, independently checking padded
  termination. This validates the primitive, not native audio interoperability.
- FFT agrees with an independent double-precision DFT above 115 dB for tested
  lengths up to 480. MDCT agrees with its defining cosine sum above 110 dB for
  120/240/480/960 coefficients. Unquantized streaming reconstruction has maximum
  error below `3e-6` and exactly 120 priming/drain samples at both frame durations.
- A thread-local counting allocator checks both profiles over 1,000 processing
  repetitions after construction, including concealment, bitrate update and
  reset. Steady-state allocations observed: zero.
- Coverage-guided libFuzzer with AddressSanitizer, seed 6716 and a requested
  30-second limit completed 37,125 version 2 packet inputs without a reported
  failure. The unchanged entropy primitive previously completed 3,800,240
  inputs. Packet payload mutations have their CRC
  repaired to reach the audio decoder. Mutation length limits were 1,024 and 4,096 bytes; the packet seed corpus
  also contained earlier inputs up to 4,083 bytes.
  These counts describe short development runs, not security certification.

## Remaining acceptance

Perceptual metrics and blind listening, a matched-quality competitor comparison,
long-running fuzzing, realistic loss traces, callback tails under load, installed
ARM64/x86-64 device measurements and battery evidence are still required.
Cross-compilation alone does not establish mobile runtime acceptance.
