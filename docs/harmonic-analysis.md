# Eight-lane harmonic analysis

The opt-in speech encoder fits eight independent harmonics in each analysis
batch. Each harmonic keeps its original f64 recurrence order, terminal phase,
Gram solve and f32 conversion. Candidate ranking, synthesis, pulse coding,
packet format, decoder and API are unchanged. The gate still defaults to off.
This implementation is original safe Rust; no external implementation or new
dependency is included.

## Compiled work and storage

Rust 1.99.0 on Apple M5 emits four two-element f64 NEON vectors in the sample
loop, compared with two in the preceding implementation. The loop executes
25 instructions for eight harmonics, compared with two 15-instruction loops.
It uses separate multiply/add/subtract operations, with no fused multiply-add
or stack access inside that loop. Register spills around trigonometric
initialization remain outside it. The
[loop table](measurements/2026-10-11-harmonic-analysis-loops.csv) and
[instructions](measurements/2026-10-11-harmonic-analysis-loops.txt) retain this
linked-code observation; other compilers and targets may emit different code.

Eight lanes can waste more work at the last batch: a 400 Hz fundamental has
20 harmonics, so analysis evaluates 24 lanes instead of 20. The wider scratch
also raises register pressure. The measurements below test whether those costs
are outweighed on this host and workload.

The [section table](measurements/2026-10-11-harmonic-analysis-sections.csv) shows
that `__TEXT,__text` decreases by 128 bytes; the two recorded constant sections
are unchanged. Signed file size increases by 16 bytes, which includes signing
metadata and should not be treated as a code-size result. Normal release builds
use opt-level 3, 16 codegen units and overflow checks, without LTO, native-CPU or
profiling options.

| Linked storage | Four lanes | Eight lanes |
| --- | --- | --- |
| Analysis function's own frame | 1,200 B | 1,520 B |
| Encoder's own frame | 41,952 B | 41,952 B |
| Decoder's own frame | 25,360 B | 25,360 B |
| Conservative internal encoder chain | 43,584 B | 43,584 B |
| Conservative internal decoder chain | 26,576 B | 26,576 B |

The analysis frame grows by 320 bytes. Its encoder path owns 41,952 + 1,520 =
43,472 bytes, below the unchanged conservative sorting path by 112 bytes.
The [emitted frames](measurements/2026-10-11-harmonic-analysis-stack.csv) verify
that the encoder and gain-sort frames remain unchanged. Bounds use the pinned
[historical call-chain audit](harmonic-resources.md); they exclude caller buffers,
OS/libm/memory routines, runtime and panic/unwind paths. They are not thread
high-water measurements or a smaller-stack claim. The
[footprint table](measurements/2026-10-11-harmonic-analysis-footprint.csv) identifies
both complete probes. Processing remains allocation-free in the exercised
ordinary and enabled harmonic paths, verified by the existing counting allocator.

## Behavioral identity

All 60 release tests pass, including allocation tracking and independently built
harmonic packets. The existing analysis fixture checks every one of the 326
supported pitches, both durations and five input patterns. Exact f32 output bits
match the scalar recurrence order, and separate direct dot products check the
least-squares amplitudes. Every eight-lane tail residue from one through seven
is covered without duplicating tests.

The public verifier checks
[1,296 complete enabled traces](measurements/2026-10-11-harmonic-analysis-traces.csv)
against the [frozen quality reference](measurements/2026-10-11-harmonic-speech-frozen-traces.csv):
72 utterances, clean and seeded 20/10 dB noise, 10/20 ms and 12/24/32 kb/s.
Whole packet streams and aligned decoded PCM hashes, frame/byte counts, harmonic
counts, bounded finite output and the 120-sample drain all match exactly.
Quality metrics were not recomputed because output PCM is identical to the
previously scored reference. These are previously evaluated inputs, including
those originally reported as held out.

An additional [48 default traces](measurements/2026-10-11-harmonic-analysis-default-traces.csv)
match the immutable four-lane
[default reference](measurements/2026-10-11-harmonic-analysis-default-reference.csv)
on the eight original development utterances, both durations and all three
bitrates. Harmonic frame counts are zero. This establishes exercised default
packet/audio identity; disabled-mode CPU was not timed for this change.

## Paired encoder CPU

The [64 raw pairs](measurements/2026-10-11-harmonic-analysis-cpu.csv) show lower
encoder process CPU at both durations on this workload. Added provenance/order
columns preserve every original observation; the
[capture record](measurements/2026-10-11-harmonic-analysis-cpu-capture.csv) hashes
the canonical table before those additions. The
[summary](measurements/2026-10-11-harmonic-analysis-cpu-summary.csv) retains
absolute CPU/wall means and medians, paired ranges and every clip-duration cell.

| Duration | Baseline median CPU/frame | Current median CPU/frame | Median paired change | CPU regressions |
| --- | --- | --- | --- | --- |
| 10 ms | 153.454 µs | 126.020 µs | −19.4228% | 0/32 |
| 20 ms | 427.245 µs | 365.647 µs | −15.8803% | 0/32 |

Individual paired CPU changes span −24.7404% to −12.8919% at 10 ms and
−23.4756% to −10.7116% at 20 ms. Every one of the sixteen clip-duration CPU
medians decreases. Wall-time medians change by −19.4647% and −16.1713%, with
ranges −24.6474% to −5.4206% and −23.4363% to −10.2963%, respectively. There
are zero wall regressions in these 64 pairs. These observations do not promise
a universal speedup or eliminate scheduling noise.

The unchanged `tools/measure_cpu.py` measures child process user plus system
time, divided by 10,000 encoded frames. Both frozen probes receive `--harmonic`.
The eight original development utterances from four speakers come from the
[input manifest](measurements/2026-10-10-speech-v2-input.sha256); all runs use
12 kb/s, both durations and four repetitions. Each clip/duration has two
baseline-first and two current-first pairs. Other agent builds, tests, scoring
and timings are paused during the single final run; desktop load, scheduling
and power remain uncontrolled. All rows, including regressions, are retained.

The probe cycles through its first 256 prepared input frames. Process totals
also include startup, input loading and encoder/transform-plan construction.
This is average process throughput on that prepared cycle, not whole-utterance
CPU, callback-tail latency, mobile energy or an unseen-quality result. Full trace
verification separately covers each whole utterance. Paired percentage medians
need not equal ratios of absolute CPU medians. See the general
[CPU method](cpu.md) for these limitations.

## Earlier screen, retained separately

Before the final run was specified, a shorter
[24-pair screen](measurements/2026-10-11-harmonic-analysis-screen-cpu.csv) used
the first four development clips, 2,000 frames and three repetitions. Its
current probe hash starts `8dc14d59`; the final probe starts `b28af61e`. Both
are built from the same source content and compiler settings, with different
signed executable identities. [Screen settings](measurements/2026-10-11-harmonic-analysis-screen-settings.csv)
retain their full hashes and the source/evaluator hashes. These rows are not
pooled with the final 64 pairs.

CPU paired medians were −20.5726% at 10 ms and −17.4770% at 20 ms, with zero
CPU regressions. Wall time regressed in two of twelve 10 ms pairs and one of
twelve 20 ms pairs. Wall ranges were −50.9113% to +12.3802% and −31.3276% to
+17.5177%, respectively. The [screen summary](measurements/2026-10-11-harmonic-analysis-screen-summary.csv)
retains those negative cases. The final matrix increased frame count and
balanced execution order; it was run once, rather than repeated until noise
or regressions disappeared.

## Reproduction and provenance

The baseline is main
[`b1e55b22c2882d80bafc4f84ad9f850792c369a1`](https://github.com/Mik-pe/snacka/tree/b1e55b22c2882d80bafc4f84ad9f850792c369a1),
with its immutable integrated four-lane probe. The current implementation is
identified by its [source content manifest](measurements/2026-10-11-harmonic-analysis-source.sha256),
not an unpublished source revision. [Source and probe provenance](measurements/2026-10-11-harmonic-analysis-source.csv)
connect both binaries to the pinned baseline and measured current files.
The host is Apple M5, aarch64, macOS 27.0 build 26A428, Python 3.14.8 and FFmpeg
9.0.2. Rust 1.99.0 uses compiler commit
`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4` and LLVM 23.1.1.

[Commands](measurements/2026-10-11-harmonic-analysis-commands.txt) record builds,
trace checks, timings and linked-code inspection. The
[artifact manifest](measurements/2026-10-11-harmonic-analysis-artifacts.sha256)
covers this page, current source provenance and all new raw evidence. The earlier
[harmonic quality and resource artifacts](measurements/2026-10-11-harmonic-speech-artifacts.sha256)
retain their historical raw/source hashes; only hashes of updated navigation
and explanatory documents change.
