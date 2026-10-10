# Paired speech CPU measurements

CPU optimization must preserve packet meaning, mathematical accuracy and
bounded processing. `tools/measure_cpu.py` compares two independently built
`cpu_probe` executables, using the same probe source and compiler flags.
The probe is an offline example, not part of the library runtime.

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
24 kb/s instead of requiring equal bytes. It does not establish equivalent
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
