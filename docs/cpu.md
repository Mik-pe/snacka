# Paired speech CPU measurements

CPU optimization must preserve packet meaning, mathematical accuracy and
bounded processing. `tools/measure_cpu.py` compares two independently built
`cpu_probe` executables, using the same probe source and compiler flags.
The probe is an offline example, not part of the library runtime.

The [version 6 gain-precision comparison](adaptive-gains.md#cpu-cost) records
low-rate quality gains and their additional CPU cost.

## Method

On Unix, the Python standard library's `resource.getrusage(RUSAGE_CHILDREN)`
measures user plus system CPU consumed by each completed child process. Divide
by the requested frame count. This includes process startup, reading PCM,
plan/input construction and validation; decoding also includes preparation of
256 packets. These costs are amortized, not subtracted. FFmpeg conversion and
exact-trace verification happen outside the measured processes. The evaluator
also records wall time, including scheduling and waiting.

Each probe prepares 256 mono frames from a clip's 48 kHz f32 PCM. Frame starts
are `(index*frame_samples) % clip_samples`; a frame crossing the end is padded
with zeros. Encoding cycles these frames with continuous stream history.
Decoding prepares 256 sequential packets and cycles them without resets;
sequence wrap remains correct. Inputs, buffers and plans are constructed before
the fixed operation loop. `black_box` observes input and output.
This is a repeated speech workload, not an installed call or a new quality test.

For each clip, duration and operation, perform three paired runs, alternating
which executable goes first. The default is 50,000 encoded or 200,000 decoded
frames per process, at 24 kb/s and 10/20 ms. No affinity, clock frequency, power
mode or concurrent desktop workload is controlled. Process CPU measures compute
cost; it does not establish callback deadlines, latency under load or battery
consumption. Wall time is retained to expose scheduling variation.

Before measuring each clip, compare exact traces at 12/24/32 kb/s and both
durations. A trace contains, for each ordinary frame and final drain, a
little-endian u16 packet length, the complete packet including CRC, and valid
PCM samples as little-endian f32. Frames are zero-padded at the clip end.
The trace retains priming and drain samples. It is an offline verification
artifact; it is not a transport/container format. By default, a mismatch aborts
evaluation. An encoder change evaluated separately for quality can use
`--allow-trace-changes`; this records both trace hashes and their identity at
each requested measurement bitrate instead of requiring equal bytes. It does not establish equivalent
quality. Only trace and input hashes are published, without audio or transcripts.

## Reproduction

Build the current and baseline libraries separately using the same release
profile and Rust toolchain. Build **both probes with identical flags**, from the
current example source and each separately built library:

```sh
cargo build --release --locked --lib
rustc --edition=2024 -C opt-level=3 -C codegen-units=16 \
  -C overflow-checks=yes examples/cpu_probe.rs \
  --extern snacka=/path/to/baseline/target/release/libsnacka.rlib \
  -L dependency=/path/to/baseline/target/release/deps -o baseline-probe
rustc --edition=2024 -C opt-level=3 -C codegen-units=16 \
  -C overflow-checks=yes examples/cpu_probe.rs \
  --extern snacka=target/release/libsnacka.rlib \
  -L dependency=target/release/deps -o current-probe
python3 tools/measure_cpu.py corpus results.csv \
  --baseline ./baseline-probe --current ./current-probe
```

The corpus directory must contain original FLAC files. The script uses FFmpeg
for conversion and otherwise only the Python standard library. `resource`
requires a Unix host; the Rust library and probe remain portable.

`--operations decode loss burst` selects complete receive workloads with
concealment and recovery. Their fixed loss patterns, speech scores and current
CPU observations are documented in [packet-loss evaluation](loss.md).
The [pitch-search comparison](loss.md#pitch-search-work-and-exact-output)
records complete PCM identity and paired CPU/callback observations.

## Bounded pulse exchanges, version 5

The baseline is commit `c450d0424d4795076ab5d154f92523b736e17ec8`. After the
initial pulse projection and greedy assignment, the encoder performs at most two
improving exchanges per band. Each exchange moves one pulse, preserving the
packet's budget and input-aligned signs. The decoder and version 5 format are
unchanged. This deliberately spends additional encoder CPU on reducing vector
error; it is not a throughput optimization. Speech scores and regressions are
reported in [quality evaluation](quality.md).

The [raw paired table](measurements/2026-10-10-apple-m5-refinement.csv) uses one
previously evaluated clip from each of four additional speakers: 1221, 2830,
4077 and 4446. Input hashes are in the
[additional-input manifest](measurements/2026-10-10-refinement-additional-input.sha256).
The method above is unchanged, with `--allow-trace-changes`, three paired
repetitions, 50,000 encoded and 200,000 decoded frames per process, and 24 kb/s.
Each repetition alternates execution order. Encoded packets and decoded PCM
change; byte identity is not expected. Decoding uses each encoder's own packets,
so entropy and selected-model workload can differ even though the decoder source
is unchanged.

Both probes use the same example source, Rust 1.99.0, opt-level 3, 16 codegen
units, overflow checks enabled and no LTO, explicit SIMD or native-CPU flags.
Libraries use the same release profile. Host: Apple M5, aarch64, macOS 27.0 build
26A428; Python 3.14.8 and FFmpeg 9.0.2. Desktop load and power are uncontrolled.

| Operation | Frame duration | Mean baseline CPU per frame | Mean current CPU per frame | Mean paired CPU change |
| --- | --- | --- | --- | --- |
| Encode | 10 ms | 12.274 µs | 12.810 µs | +4.521% |
| Encode | 20 ms | 29.147 µs | 30.054 µs | +3.274% |
| Decode | 10 ms | 4.395 µs | 4.431 µs | +0.851% |
| Decode | 20 ms | 10.554 µs | 10.567 µs | +0.206% |

Means weight each clip/repetition equally; the change column averages individual
paired percentage changes. Individual runs vary, and these small decoder
changes do not establish a speed difference. This measures process CPU on
repeated read speech, not installed mobile battery use or individual callback
deadlines under load. The quality improvement is small and needs listening
validation. Codec and evaluation sources are recorded in the
[source manifest](measurements/2026-10-10-refinement-source.sha256).

## Cached pulse priorities, version 5

The baseline is commit `8089bc873d5d6122ee49a37cdbaea5e8bc13ac19`. The change caches
each band's next bit cost and integer priority denominator, updating only the
band that receives a pulse. Four zero gain codes return zero pulses immediately.
Allocation order, exact cross products, ties, format, frame budgets and delay
are unchanged. No new persistent state, library dependency or processing
allocation is introduced.

A six-second sampling profile of a repeated two-tone 20 ms/24 kb/s encoder loop
on Apple M5 attributed about 15% of samples to pulse allocation. The profile is
an exploratory synthetic workload, not an application call. It motivated the
cached calculation; speech measurements below test complete operations.

The [raw CPU table](measurements/2026-10-10-apple-m5-priorities.csv) uses the eight
previously evaluated additional LibriSpeech clips from four speakers in the
[version 5 input manifest](measurements/2026-10-10-pulse-unseen-input.sha256).
Their licensing, attribution and official archive paths are in
[quality evaluation](quality.md). No speech-quality algorithm was tuned here.
Both probes used the same source, Rust 1.99.0, opt-level 3, 16 codegen units,
overflow checks enabled and no LTO, explicit SIMD or native-CPU flags. Libraries
used the same release profile. The host was Apple M5, aarch64, macOS 27.0 build
26A428. Python was 3.14.8 and FFmpeg 9.0.2. Desktop load and power were uncontrolled.

| Operation | Frame duration | Mean baseline CPU per frame | Mean current CPU per frame | Mean paired CPU change |
| --- | --- | --- | --- | --- |
| Encode | 10 ms | 13.738 µs | 13.448 µs | −1.908% |
| Encode | 20 ms | 34.950 µs | 32.725 µs | −6.016% |
| Decode | 10 ms | 4.886 µs | 4.877 µs | +0.078% |
| Decode | 20 ms | 11.586 µs | 10.887 µs | −5.347% |

The table gives equal weight to each clip/repetition. The change column is the
mean of individual paired percentage changes, rather than the ratio of column
means. There are 96 pairs. Seventeen report higher CPU cost; changes range from
−24.800% to +45.364%. These large individual fluctuations limit the precision of
small differences. In particular, 10 ms decoding has no established benefit.
The result supports a bounded encoding optimization, not universal speed,
callback deadlines or a mobile battery claim. It cannot be compared directly
with the earlier synthetic batch-throughput table.

All 48 full-clip packet/PCM trace pairs at both durations and three bitrates are
byte-identical, including final drain. Trace hashes at 24 kb/s and original
input hashes are recorded in the raw table. Thus the measured change preserves
these clips' encoded bytes and reconstructed audio exactly; it does not provide
new quality gains. A separate development comparison of 693,000 gain/budget
combinations also produced identical allocations, covering both transform sizes,
all three bandwidths and every legal packet byte budget. This is additional
sampled evidence, not exhaustive proof over all four-code gain combinations.

All 41 debug/release tests pass, including independent all-budget pulse packets,
invalid-gain/history refusal and allocation tracking at both durations and both
spectral models. A requested 30-second AddressSanitizer packet fuzz run with
seed 6716 completes 3,737 inputs without a reported failure. Source portability is checked for iOS, iOS Simulator and
Android; installed-device evidence remains open. Sources are recorded in the
[manifest](measurements/2026-10-10-priorities-source.sha256).

## Reusing pulse candidates, version 6

The baseline is commit `241dcd08350d75be37e11d797346e91a52707f15`.
The encoder evaluates three bandwidths. Their first three frequency bands have
identical coefficient ranges and gain codes. Compute those gains once per frame,
and retain each band's quantized shape and norm while its allocated pulse count
is unchanged. The fourth band's range changes, so it is always recomputed.
The cache is local to the frame; it adds no persistent stream state or heap
allocation. The error sum retains its original coefficient order and tie rules.
Power-of-two gain rounding uses a mask; the capped amplitude bounds its sum to
215, within u8. The version 6 format, decoder and transform delay are unchanged.

The [paired CPU table](measurements/2026-10-10-candidate-reuse-cpu.csv) contains
288 pairs: eight previously evaluated LibriSpeech clips from four speakers,
two durations, three bitrates, encoding/decoding and three alternating-order
repetitions. Each process encodes 10,000 or decodes 20,000 frames. Both probes
use the current `examples/cpu_probe.rs`, Rust 1.99.0, opt-level 3, 16 codegen
units, overflow checks and no LTO or native-CPU flags. Host: Apple M5, aarch64,
macOS 27.0 build 26A428; Python 3.14.8 and FFmpeg 9.0.2. Desktop load and power
are uncontrolled. Process CPU includes the amortized preparation described above.

| Operation | Duration | Bitrate | Median baseline CPU/frame | Median current CPU/frame | Median paired change |
| --- | --- | --- | --- | --- | --- |
| Encode | 10 ms | 12 kb/s | 9.148 µs | 8.320 µs | -8.785% |
| Decode | 10 ms | 12 kb/s | 3.351 µs | 3.326 µs | -0.219% |
| Encode | 10 ms | 24 kb/s | 13.293 µs | 11.974 µs | -8.498% |
| Decode | 10 ms | 24 kb/s | 4.521 µs | 4.520 µs | -0.437% |
| Encode | 10 ms | 32 kb/s | 16.774 µs | 15.073 µs | -9.301% |
| Decode | 10 ms | 32 kb/s | 5.499 µs | 5.397 µs | -1.581% |
| Encode | 20 ms | 12 kb/s | 20.858 µs | 18.807 µs | -8.954% |
| Decode | 20 ms | 12 kb/s | 7.944 µs | 7.906 µs | -0.534% |
| Encode | 20 ms | 24 kb/s | 32.332 µs | 29.848 µs | -9.485% |
| Decode | 20 ms | 24 kb/s | 10.788 µs | 10.563 µs | +0.023% |
| Encode | 20 ms | 32 kb/s | 40.444 µs | 38.666 µs | -7.531% |
| Decode | 20 ms | 32 kb/s | 12.379 µs | 12.387 µs | -1.368% |

Medians weight each clip/repetition equally. The paired-change median is taken
from individual pairs; it need not equal the ratio of the two CPU medians.
Eight of 144 encoder pairs cost more CPU; their changes range from −26.722% to
+20.875%. Decoder changes range from −27.711% to +22.229%, with 56 of 144
pairs increasing. The small decoder differences do not establish an improvement.
The encoder result supports reduced compute cost on this host and workload;
mobile power, callback tails under load and performance against other libraries
remain unmeasured here.

Reproduce the paired runs after separately building the two libraries and probes
as above:

```sh
python3 tools/measure_cpu.py corpus results.csv \
  --baseline ./baseline-probe --current ./current-probe \
  --bitrates 12000 24000 32000 --repetitions 3 \
  --encode-frames 10000 --decode-frames 20000
```

The [full trace table](measurements/2026-10-10-candidate-reuse-traces.csv)
compares complete packets and reconstructed PCM, including priming, zero-padded
final frames and drain. All 2,352 speech pairs are byte-identical: the 56 clips
from 28 speakers already evaluated for [adaptive gains](adaptive-gains.md),
clean and the same deterministic 20/10 dB noise from `tools/evaluate_speech.py`,
both durations and rates 12000, 12345, 12800, 14400, 16000, 24000 and 32000.
Input and trace hashes are retained per pair. These are existing evaluation
inputs, not new held-out quality results. Their licensing and attribution are
in [quality evaluation](quality.md).

Another 56 identical trace pairs cover four 4,801-sample synthetic signals at
the same durations and rates: silence; constant `1e-40`; impulses of +1 at
indices divisible by 137, otherwise −1 at indices divisible by 211, otherwise
zero; and `min(1, i/4800) * sin(0.271*i)`. Each is rounded to f32 before encoding.
This samples transitions and extreme levels; it is not exhaustive proof over
all inputs. Unchanged packets and PCM preserve the existing version 6 quality
results for these speech cases; there is no new quality gain claim.

All 46 debug and release tests pass, including every legal packet byte budget,
independent inverse-cosine reconstruction, malformed packet/history handling
and zero processing allocations at both durations and all gain precisions.
Live-audio clippy passes. The [source manifest](measurements/2026-10-10-candidate-reuse-source.sha256)
identifies the codec and evaluation sources.


## Narrow priority operands, version 7

The baseline is commit `fb78710741c174e6903d0aae0f169b7ff6293c54`.
Allocation retains its numerator, denominator and best priority as u64, widening
only the operands of the exact u128 cross products. Their values, eligibility
and strict tie ordering are unchanged, including zero-cost increments. Any two
u64 values multiply within u128. This adds no cache, persistent state or heap
allocation and preserves the version 7 format and reconstructed audio.

The [paired CPU table](measurements/2026-10-11-u64-priorities-cpu.csv) contains
288 pairs on the same eight development clips from four speakers, two durations,
three bitrates and three alternating-order repetitions. Each process encodes
10,000 or decodes 20,000 frames. The host is Apple M5, aarch64, macOS 27.0
build 26A428. Both probes use Rust 1.99.0 (`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4`,
LLVM 23.1.1), opt-level 3, 16 codegen units, overflow checks and no LTO,
native-CPU or profiling flags. Python 3.14.8 and FFmpeg 9.0.2 prepare the inputs.
Other codec builds, tests and scoring jobs were paused during measurement;
desktop load and power remain uncontrolled. Process CPU includes setup and,
for decoding, preparation of 256 encoded frames, as described above.

| Operation | Duration | Bitrate | Median paired CPU change | Slower repetitions | Slower clip medians |
| --- | --- | --- | --- | --- | --- |
| Encode | 10 ms | 12 kb/s | -1.046% | 9/24 | 4/8 |
| Decode | 10 ms | 12 kb/s | -0.135% | 11/24 | 3/8 |
| Encode | 20 ms | 12 kb/s | -0.915% | 7/24 | 2/8 |
| Decode | 20 ms | 12 kb/s | +0.204% | 14/24 | 5/8 |
| Encode | 10 ms | 24 kb/s | -1.172% | 7/24 | 2/8 |
| Decode | 10 ms | 24 kb/s | -1.631% | 4/24 | 0/8 |
| Encode | 20 ms | 24 kb/s | -0.495% | 7/24 | 1/8 |
| Decode | 20 ms | 24 kb/s | -0.424% | 9/24 | 2/8 |
| Encode | 10 ms | 32 kb/s | -1.264% | 6/24 | 1/8 |
| Decode | 10 ms | 32 kb/s | -0.987% | 8/24 | 2/8 |
| Encode | 20 ms | 32 kb/s | -0.845% | 5/24 | 0/8 |
| Decode | 20 ms | 32 kb/s | -1.189% | 5/24 | 0/8 |

Eleven of twelve cell medians decrease by 0.135–1.631%; 12 kb/s, 20 ms decoding
increases by 0.204%. Multiple cells contain slower repetitions and clip medians.
Individual paired changes span −22.052% to +44.567%. The small differences and
outliers limit precision: this supports modest observed savings on this host,
not a universal speed or battery claim. Medians weight each pair equally and
need not equal the ratio of independently aggregated CPU medians. The
[summary](measurements/2026-10-11-u64-priorities-summary.csv) retains every cell's
CPU medians, repeat ranges, clip-median ranges and regression counts.
Reproduce with the `tools/measure_cpu.py` command above and these two sources.

ARM64 disassembly reduces the allocator from 280 to 274 instructions: two
redundant high-word `madd` operations and four zero-high-word moves disappear.
Exact products still use `umulh` and `mul`. The allocator's 368-byte stack
frame, executable text (293,308 bytes) and measured read-only constants
(72,304 bytes) are unchanged. The executable file increases by eight bytes
because its longer output name changes the ad-hoc signing identifier, not
runtime code or data. The [footprint table](measurements/2026-10-11-u64-priorities-footprint.csv)
records both executable hashes and sizes; hashes identify the exact measured
probes, whose names are not required to reproduce the build.

All 2,688 [speech trace pairs](measurements/2026-10-11-u64-priorities-speech-traces.csv)
are byte-identical: the 64 previously evaluated clips from
[pulse-class evaluation](pulse-classes.md), clean and deterministic 20/10 dB
noise from `tools/evaluate_speech.py`, both durations and bitrates 12000, 12345,
12800, 14400, 16000, 24000 and 32000. Full traces include headers, CRC, priming,
zero-padded final frames, PCM and drain. Input/trace hashes are retained per pair.
These are existing evaluation inputs, not new held-out quality evidence.

Another 160 [synthetic trace pairs](measurements/2026-10-11-u64-priorities-synthetic-traces.csv)
are identical at both durations and bitrates 12000, 12001, 12345, 12800, 14400,
16000, 24000 and 32000. Each fixture contains 24,013 mono samples at 48 kHz,
with index `i = 0..24012`. Evaluate the following Python expressions in order,
then round each sample to little-endian f32 without normalization. `sin` and
`pi` come from `math`; the four-tone sum follows the listed frequency order.

| Fixture | Sample expression |
| --- | --- |
| Silence | `0.0` |
| DC | `0.7` |
| Alternating | `0.75 if i % 2 else -0.75` |
| Sine 440 | `0.7 * sin(2*pi*440*i/48000)` |
| Chirp | `0.65 * sin(2*pi*(200*i/48000 + 21000*(i/48000)**2))` |
| White noise | Consecutive `random.Random(730193).uniform(-0.8, 0.8)` draws from one generator |
| Impulses | `0.95 if i % 1007 == 0 else 0.0` |
| Tiny | `2**-22 * sin(i*0.069)` |
| Four tones | `0.17 * sum(sin(2*pi*f*i/48000) for f in [150,1200,3500,16000])` |
| Interrupted | `0.8 * sin(i*0.039) if i % 7301 < 1900 else 0.0` |

All 48 debug/release tests pass, including packet budget, malformed input,
history refusal and zero processing allocations. An independent uncached
exact-rational allocator oracle matches 240,933 gain/budget/profile cases,
covering every supported packet length and six duration/bandwidth profiles.
It covers the Cartesian gain grid `[0,1,2,128,208]^4` plus every gain code in
balanced and contrasting band combinations. This samples ties and zero-cost
plateaus; it is not exhaustive proof over all gain combinations or audio inputs.
Live-audio clippy and the canonical table generator check pass. The
[source manifest](measurements/2026-10-11-u64-priorities-source.sha256) identifies
codec, probe and evaluator sources; the
[artifact manifest](measurements/2026-10-11-u64-priorities-artifacts.sha256)
identifies the published evidence. Audio licensing and attribution are in
[quality evaluation](quality.md). This change provides no new quality gain.
