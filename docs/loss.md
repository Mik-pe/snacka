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
without claiming a codec improvement. These controls use the same library implementation.

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
same release profile. The manifest records the measurement checkout, including
its Cargo files before the optional listening lab was added; the library source
is identical. Source hashes are in the
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

## Pitch-search work and exact output

The current-window energy is accumulated once per coarse and fine pitch search,
with the original f32 summation order. It is independent of the candidate lag.
This removes 146 repeated 160-sample energy sums in the coarse search; products
and lagged-window energy retain their order. The fine search likewise reuses
its 320-sample current-window energy. A precisely zero downsampled window skips
correlation search and retains the existing waveform fallback. Confirming actual
zero samples avoids treating underflowed squares as silence; cancellation in
the downsampled signal must not discard nonzero high-frequency PCM.

The [complete-PCM table](measurements/2026-10-10-plc-correlation-identity.csv)
contains 2,592 comparisons across the same 48 clips and 24 speakers: 12/24/32
kb/s, both frame durations, clean/20/10 dB noise, and all three seeded loss
families. Baseline core source is revision
`11edd2f28cb180dbb00acf6c54df09df40a57bc6`; the library core is identical in
`90432f71bc2afe44fed2c0b54b62f11fe75334eb`, before this change. Both examples
are built from the same current source and compiler flags. Every complete PCM
buffer, including concealment and recovery, matches byte for byte. Packet/drain
budgets and received-region byte checks also match. Input hashes and attribution
are the same [48-clip manifest](measurements/2026-10-10-loss-seeded-input.sha256).
No new quality scores are inferred from reduced arithmetic work: the unchanged
PCM preserves the measured audio, including its existing loss weaknesses.

The evaluator records both complete decoded PCM hashes and `pcm_identical`.
`--require-identical` makes a mismatch an error before a success table is
written. This is appropriate for output-preserving changes; ordinary quality
experiments can still compare changed concealment without that flag.

```sh
python3 tools/evaluate_loss.py corpus identity.csv \
  --roundtrip ./current-roundtrip --baseline ./baseline-roundtrip \
  --patterns seeded --require-identical --bitrates 12000 24000 32000
```

[Source hashes](measurements/2026-10-10-plc-correlation-source.sha256) distinguish
baseline library files from the current library, common examples and tools.
Build baseline library revision above and link the common examples exactly as
in the earlier comparison; both use opt-level 3, 16 codegen units and overflow
checks. Default library builds enable no optional audio-device dependencies.

### Paired CPU observations

The [speech CPU table](measurements/2026-10-10-plc-correlation-cpu-speech.csv)
uses one clip from each of speakers 1320, 260, 4507, 5639, 6930, 7021, 8224 and
908. There are 720 paired observations: 15 repetitions, both durations and
three receive workloads at 24 kb/s. Each process performs 20,000 operations.
The shorter process intervals alternate baseline/current order between
repetitions. Startup, plans and preparation remain included, as in the earlier
method, so absolute costs should not be compared with the 200,000-operation
control table above. All 48 uninterrupted preflight traces match.

The [synthetic-silence table](measurements/2026-10-10-plc-correlation-cpu-silence.csv)
adds 90 paired observations with the same settings and six matching preflight
traces. Its input is exactly two seconds of zero-valued 48 kHz mono PCM,
converted to FLAC with FFmpeg; it represents a precisely silent stream, not a
noisy microphone. The tables retain all samples and record input/trace hashes.

| Receive workload | Speech, 10 ms | Speech, 20 ms | Silence, 10 ms | Silence, 20 ms |
| --- | --- | --- | --- | --- |
| Unchanged decode control | −0.02% | +0.10% | −1.50% | +2.57% |
| One loss per ten frames | −3.43% | −1.71% | −24.15% | −8.84% |
| Three losses per thirty frames | −1.03% | −0.86% | −4.35% | −8.11% |

Cells are medians of paired process-CPU percentage changes, with 120 speech
pairs or 15 silence pairs per cell. Negative values indicate lower CPU. The
speech isolated-loss interquartile ranges are −8.57%..−0.88% at 10 ms and
−4.73%..+1.53% at 20 ms. Unchanged speech-decode controls span −1.68%..+3.18%
and −3.30%..+3.32% respectively. Small burst differences remain close to this
noise; there is no claimed universal receive-speed improvement.

```sh
python3 tools/measure_cpu.py speech-eight cpu-speech.csv \
  --baseline ./baseline-probe --current ./current-probe \
  --operations decode loss burst --repetitions 15 --decode-frames 20000
```

The silence input is reproducible without an external audio source:

```sh
python3 -c "from pathlib import Path; Path('silence.f32').write_bytes(bytes(48000*2*4))"
mkdir -p silence-corpus
ffmpeg -v error -y -f f32le -ar 48000 -ac 1 -i silence.f32 \
  -c:a flac silence-corpus/silence.flac
python3 tools/measure_cpu.py silence-corpus cpu-silence.csv \
  --baseline ./baseline-probe --current ./current-probe \
  --operations decode loss burst --repetitions 15 --decode-frames 20000
```

### Individual loss callbacks

The [callback table](measurements/2026-10-10-plc-correlation-callbacks.csv)
compares the common `benches/kernels.rs` source linked to each library with the
same compiler flags. Three paired benchmark repetitions alternate execution
order. Each profile uses 100 warmups and 1,000 recorded calls, with four
received packets prepared outside timing, then first loss, continued loss and
recovery measured separately. Inputs are the repeated synthetic two-tone
frames defined in that benchmark. These are not the speech-corpus callbacks.

| Stage | 10 ms median paired change | 20 ms median paired change |
| --- | --- | --- |
| First loss | −7.78% | −7.24% |
| Continued loss | 0.00% | +0.30% |
| Recovery | −0.04% | +0.46% |

Each cell pools nine pairs: three bitrates and three repetitions. The raw table
includes median, p95 and p99 for both builds, including regressions. First-loss
median changes range from −12.45% to +98.27% at 10 ms and −9.52% to −3.88% at
20 ms; some 10 ms control stages also have large outliers. Background desktop
load is uncontrolled. These observations on Apple M5/macOS 27.0, with Rust
1.99.0 and the flags above, establish neither mobile battery use nor callback
deadlines under load. The codec's PCM, delay and packet format are unchanged.
