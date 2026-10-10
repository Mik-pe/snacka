# Packet-loss evaluation

Concealment must be evaluated on speech, with intelligibility estimates as well
as waveform error. Lower reconstruction error alone does not establish better
speech. `tools/evaluate_loss.py` compares decoders with identical uninterrupted
output and encoded byte budgets. The optional quality scores use the method in
[quality evaluation](quality.md).

## Reproducible traces

The default `--patterns periodic` drops the last 1/3/6 frames in each 10/30/60
frames. `--patterns seeded` generates three independent trace families:

- `independent5`: independent Bernoulli drops with probability 0.05 per frame.
- `burst40`: a two-state Markov process targeting 10% loss and a 40 ms mean burst.
- `burst80`: the same process with an 80 ms mean burst.

For the burst models, the probability of leaving a loss state is
`frame_ms / mean_burst_ms`; the probability of entering it from received state
is one ninth of that value. These specify a stationary target of 10% loss.
Actual finite traces can differ, and can have no loss. State begins received;
the first 80 ms are guaranteed received. This deliberately separates steady
speech from startup-loss behavior. The final drain is always received.

Each trace uses Python's `random.Random` with the string seed
`snacka-loss-6716:clip:frame_ms:pattern`. Masks are shared across noise conditions
and bitrates for that clip/duration/pattern. The file contains sorted, unique,
zero-based ordinary-frame indices, one per line; its SHA-256 is recorded.
These are synthetic traces, not measured network conditions, and model neither
reordering nor transport jitter. Decoders act causally at the missing deadline.

The offline example accepts either periodic arguments or an explicit mask:

```sh
cargo run --release --example roundtrip -- input.f32 output.f32 24000 20 \
  --loss-frames loss.txt
```

Duplicate, invalid and out-of-range indices are rejected. The drain cannot be
listed as a lost frame. An empty file describes an uninterrupted stream.

## Alignment and validation

The example discards exactly 120 priming samples and retains the valid drain
prefix. A PCM sample at aligned index `i` belongs to decoder output frame
`(i+120) / frame_samples`. Lost-region masks follow that same mapping. Onset
masks cover the first 120 samples of the first dropped frame of a burst;
recovery masks cover the first 120 samples of the next received frame,
including any valid drain samples. Partial frames and removed padding are not
counted as original audio.

Waveform SNR compares each region with uninterrupted decoded PCM, isolating
loss from quantization. Missing, zero-energy and exact-match regions have blank
SNR cells rather than an invented finite score. Whole-utterance STOI and ESTOI
compare with clean speech, including in noisy conditions. Spectral distance
compares with encoder input. The tables include both absolute scores and changes
from uninterrupted decoding; these are proxies, not listening results.

For both executables, the evaluator checks:

- Equal uninterrupted PCM bytes and packet metadata before scoring loss.
- Equal ordinary and drain byte budgets, and the exact requested drop count.
- Complete, finite PCM within [-1,1].
- Byte identity with uninterrupted decoding in every received region after its
  potentially uncertain 120-sample recovery prefix. Loss error must not propagate
  farther through this codec's transform history.

Optional metrics require the existing `tools/quality-requirements.txt`.
The shared scorer seeds NumPy before each STOI/ESTOI pair using the first four
SHA-256 bytes, interpreted little-endian, of
`snacka-quality-6716:clip:condition:frame_ms:bitrate`. The seed is independent of
decoder and loss pattern, so identical PCM gets identical scores. This controls
pystoi 0.4.1's dither of zero-energy normalization rows. Insufficient-speech
warnings and non-finite scores remain errors.

## Version 5 speech results

The [raw table](measurements/2026-10-10-loss-seeded.csv) evaluates commit
`11edd2f28cb180dbb00acf6c54df09df40a57bc6`: 48 clips, 24 speakers, three
clean/noisy conditions, 10/20 ms frames and three seeded traces, at 24 kb/s.
There are 864 cases. Both independently built executables use that same codec
source and produce identical scores; this verifies the measurement controls,
without claiming a codec improvement. The library implementation is unchanged.

Inputs are [LibriSpeech test-clean](https://www.openslr.org/12), CC BY 4.0.
Attribution: Vassil Panayotov, Guoguo Chen, Daniel Povey and Sanjeev Khudanpur,
*LibriSpeech: an ASR corpus based on public domain audio books*, ICASSP 2015.
The [input manifest](measurements/2026-10-10-loss-seeded-input.sha256) records
original FLAC hashes. Archive paths follow
`LibriSpeech/test-clean/speaker/chapter/clip.flac`. The official mirror used was
`https://openslr.trmal.net/resources/12/test-clean.tar.gz`. No audio or transcripts
are redistributed. These clips have previously been evaluated during codec
development; this is not a new unseen quality comparison.

FFmpeg 9.0.2 converts 16 kHz mono FLAC to 48 kHz little-endian f32. Noise addition
uses the existing independently seeded 20/10 dB Gaussian-noise procedure.
No samples clip during noise addition. Means give each clip/duration equal
weight. Actual loss rates weight ordinary frames and include the received
startup period. Read English and synthetic noise do not establish conversational,
multilingual, double-talk or real microphone acceptance.

| Clean-speech trace | Actual frame loss | Mean STOI | Mean change from uninterrupted STOI |
| --- | --- | --- | --- |
| Independent 5% target | 4.884% | 0.949989 | −0.014237 |
| Burst, 40 ms mean | 9.769% | 0.912950 | −0.051276 |
| Burst, 80 ms mean | 9.982% | 0.899616 | −0.064610 |

Mean uninterrupted clean STOI is 0.964226. Longer bursts remain a material
weakness: the worst clean STOI change is −0.250943. A trace with no loss is
retained, without fabricating a loss-region score. Recovery byte checks pass in
all cases. They establish bounded error propagation, not inaudible concealment,
reconstruction of missing words or superiority over another codec.

Python is 3.14.8, NumPy 2.5.3, SciPy 1.18.1 and pystoi 0.4.1. Both examples use
Rust 1.99.0, opt-level 3, 16 codegen units and overflow checks; libraries use the
same release profile. Source hashes are in the
[manifest](measurements/2026-10-10-loss-evaluation-source.sha256).

```sh
cargo build --release --locked --example roundtrip
PYTHONDONTWRITEBYTECODE=1 OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 \
  python3 tools/evaluate_loss.py corpus results.csv \
  --roundtrip target/release/examples/roundtrip \
  --baseline /path/to/baseline/roundtrip --patterns seeded --quality
```

Build both examples from the same current example source when comparing library
revisions. For a repeated control with one revision, both executable arguments
may point to the same binary. `--bitrates 12000 24000 32000` extends evaluation;
the published loss table covers only 24 kb/s.

## CPU with loss

The CPU probe adds `loss` (one dropped frame per ten) and `burst` (three per
thirty) to ordinary `decode`. At 10/20 ms, the latter gives 30/60 ms bursts.
Preparation, packet cycling, process CPU and wall-time limits follow the
[paired CPU method](cpu.md). These modes measure the complete mixture of
decoding, prediction and recovery, not individual concealment-call tails.

Build both probes from the same source with identical flags, then select modes:

```sh
python3 tools/measure_cpu.py corpus cpu.csv \
  --baseline ./baseline-probe --current ./current-probe \
  --operations decode loss burst
```

The preflight traces remain uninterrupted traces. They prove ordinary packet/PCM
identity, and do not replace quality evaluation of concealed output.


The [raw CPU table](measurements/2026-10-10-apple-m5-loss-mix.csv) uses one clip
from each of development speakers 1320, 260, 5639 and 6930. Both independently
built libraries are the same revision above; paired runs are repeat controls,
not an optimization comparison. Three paired repetitions per clip/duration/mode
use 200,000 operations per process at 24 kb/s. Means below combine both runs
and give each clip/repetition equal weight. All 24 uninterrupted preflight
traces at 12/24/32 kb/s and both durations match exactly.

| Receive workload | 10 ms mean process CPU per frame | 20 ms mean process CPU per frame |
| --- | --- | --- |
| No loss | 4.330 µs | 10.255 µs |
| One loss per ten frames | 5.647 µs | 12.554 µs |
| Three losses per thirty frames | 4.871 µs | 11.710 µs |

Bursts need fewer pitch searches than isolated losses: continued concealment
reuses its period estimate. The host is Apple M5, aarch64, macOS 27.0 build
26A428. Compiler flags are opt-level 3, 16 codegen units and overflow checks,
without LTO, explicit SIMD or native-CPU flags. Desktop load and power are
uncontrolled; process startup and preparation remain included. These means do
not prove individual callback deadlines under load or mobile battery use.
