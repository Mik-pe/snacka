# Four-lag harmonic pitch scoring

The opt-in harmonic encoder scores up to four distinct eligible integer lags
together. It reuses one chronological prefix-energy sum and preserves every
per-lag f64 accumulation order. Full packet streams and decoded PCM remain
bit-exact on the frozen evaluation matrix. The runtime gate still defaults to
off; harmonic fitting, synthesis, quantization, decoder, wire format and API
are unchanged. The implementation is original safe Rust with no new dependency.

## Arithmetic and ranking

For lag `l`, the scorer uses `pairs = input.len() - l`, the cross product
`sum(x[i+l]*x[i])`, shifted energy `sum(x[i+l]^2)` and prefix energy
`sum(x[i]^2)`. Each sum starts at zero and visits sample indices in ascending
order after conversion to f64. The normalized score still divides the cross
product by the square root of the two energies' product, returns zero below
the existing `1e-18` product threshold, and converts once to f32.

Ascending fundamental frequencies give nonincreasing rounded lags and
nondecreasing prefix lengths. The shared prefix sum extends to each new length
without subtraction or reassociation. Each correlation lane processes the
common prefix first, then its own remaining chronological tail. One-, two- and
three-lag final batches use bounded specializations. Ineligible leading lags
score zero and do not advance the energy prefix.

All 326 fundamental-frequency entries, including entries that round to an
already scored lag, are replayed in their original ascending order. The
correlation threshold remains 0.55. The top eight finalists keep the original
strict score comparison and lower-frequency tie order; ranking only distinct
lags would change those finalists and is deliberately avoided. Eligibility
still requires at least one third of the window after the lag. The 10 ms
encoder consequently still excludes fundamentals below 120 Hz.

## Linked cost and storage

The [actual instructions](measurements/2026-10-11-harmonic-pitch-loops.txt) and
[counts](measurements/2026-10-11-harmonic-pitch-loops.csv) come from pinned normal
Rust 1.99.0 release probes on Apple M5. The pitch routine inlines its bounded
specializations and grows from 706 to 2,288 linked instructions. Vector
conversions and products feed scalar chronological additions: the routine
contains zero vector f64 additions and zero fused multiply-add instructions.
These static counts cover the entire routine, including tails and checks;
they are not the number of instructions executed per frame. Other compilers
and targets may generate different code.

The [section table](measurements/2026-10-11-harmonic-pitch-sections.csv) records
`__TEXT,__text` growth of 6,272 bytes and `__DATA_CONST,__const` growth of
288 bytes; `__TEXT,__const` remains unchanged. Signed file size grows by
16,496 bytes, including signing metadata, and is not a code-size measurement.
Build flags are opt-level 3, 16 codegen units and overflow checks, without
LTO, native-CPU or profiling options.

| Linked storage | Baseline | Four-lag scorer |
| --- | --- | --- |
| Pitch function's own frame | 192 B | 416 B |
| Analysis function's own frame | 1,520 B | 1,520 B |
| Encoder's own frame | 41,952 B | 41,952 B |
| Decoder's own frame | 25,360 B | 25,360 B |
| Conservative internal encoder chain | 43,584 B | 43,584 B |
| Conservative internal decoder chain | 26,576 B | 26,576 B |

The pitch frame grows by 224 bytes. Its encoder path owns 41,952 + 416 =
42,368 bytes, below the analysis path's 43,472 bytes and the unchanged
conservative sorting bound. The
[emitted frames](measurements/2026-10-11-harmonic-pitch-stack.csv) verify the
unchanged encoding, decoding, analysis and sorting frames. The
[footprint table](measurements/2026-10-11-harmonic-pitch-footprint.csv) identifies
both complete probes. Bounds use the pinned [historical call-chain audit](harmonic-resources.md)
and exclude caller buffers, OS/libm/memory routines, runtime and panic/unwind
paths. They are not thread high-water measurements or a recommended thread
stack size. There is no new persistent stream state or processing heap cache.

## Behavioral identity

All 61 release tests pass, including allocation tracking, independent harmonic
packets, malformed-packet state preservation and stream recovery. The original
uncached chronological scorer remains an independent test oracle. Batched
scores match its exact f32 bits in 37,164 comparisons across all 326 pitches:
18 voice/noise cases across the two processing window lengths, plus 96 boundary
and adversarial cases. All 114 finalist sets match, covering empty and very
short windows, zero and signed zero, tiny values, very large finite values,
constant/alternating samples and batch tails of zero through four lanes.
Existing counting-allocator coverage verifies enabled harmonic encoding and
decoding at both durations without processing allocations; constructor
allocation remains allowed.

The public verifier checks
[1,296 enabled complete traces](measurements/2026-10-11-harmonic-pitch-traces.csv)
against the [frozen quality reference](measurements/2026-10-11-harmonic-speech-frozen-traces.csv):
72 utterances, clean and seeded 20/10 dB noise, both durations and 12/24/32 kb/s.
Whole packet and aligned PCM hashes, sample/frame/byte counts, harmonic counts,
finite bounded output and the 120-sample drain match. Quality metrics were not
recomputed because the scored output PCM is identical. These are previously
evaluated inputs, including speakers originally reported as held out.

An additional [48 default traces](measurements/2026-10-11-harmonic-pitch-default-traces.csv)
match the preceding eight-lane
[default reference](measurements/2026-10-11-harmonic-analysis-default-traces.csv)
on the eight development utterances, both durations and all three rates.
Harmonic counts are zero. Disabled-mode CPU and decoder CPU were not timed
for this change.

## Paired encoder CPU

The [64 raw pairs](measurements/2026-10-11-harmonic-pitch-cpu.csv) show lower
encoder process CPU at both durations on this workload. Added columns record
order and provenance while preserving every original value. The
[capture record](measurements/2026-10-11-harmonic-pitch-cpu-capture.csv) hashes
the canonical table before those additions; the
[summary](measurements/2026-10-11-harmonic-pitch-cpu-summary.csv) retains absolute
CPU/wall means and medians, paired ranges, regressions and every clip-duration cell.
Percentage summaries aggregate the published per-pair percentages rounded to
four decimal places; absolute summaries use the recorded nanoseconds.

| Duration | Baseline median CPU/frame | Current median CPU/frame | Median paired change | CPU regressions |
| --- | --- | --- | --- | --- |
| 10 ms | 131.877 µs | 108.982 µs | −17.3715% | 0/32 |
| 20 ms | 362.161 µs | 297.619 µs | −20.8833% | 0/32 |

Individual CPU changes span −38.9328% to −0.6086% at 10 ms and −39.5407%
to −1.3776% at 20 ms. Every one of the sixteen clip-duration CPU medians
improves. Wall paired medians change by −17.5873% and −19.6688%, with
ranges −43.8592% to +19.5156% and −42.9716% to +0.4922%, respectively.
**Wall time regresses in one of thirty-two pairs at each duration.** Those rows
and the large CPU gains remain in the evidence; no observations are filtered.
These measurements do not promise a universal speedup or remove scheduling noise.

The unchanged `tools/measure_cpu.py` measures child process user plus system
time divided by 10,000 encoded frames. Both frozen probes receive `--harmonic`.
The eight original development utterances from four voices come from the
[input manifest](measurements/2026-10-10-speech-v2-input.sha256). All pairs use
12 kb/s, both durations and four repetitions. Each clip/duration has two
baseline-first and two current-first pairs. Other agent builds, tests, quality
scoring and timing are paused during the single final run. Desktop load,
scheduling and power remain uncontrolled.

The probe cycles through its first 256 prepared frames. Process totals include
startup, input loading and encoder/transform-plan construction. This is average
process throughput on that prepared cycle, not whole-utterance CPU, callback-tail
latency, mobile energy or an unseen-quality result. Full trace verification
separately covers each complete utterance. Paired percentage medians need not
equal ratios of absolute CPU medians. Other architectures are not performance
measured here. See the general [CPU method](cpu.md) for these limitations.

## Earlier screen, retained separately

The [earlier 24-pair screen](measurements/2026-10-11-harmonic-pitch-screen-cpu.csv)
uses the first four development clips from two voices, 12 kb/s, both durations,
2,000 encoded frames and three repetitions. Each clip/duration runs in AB, BA,
AB order, so execution order is unbalanced 2:1. Both probes explicitly enable
harmonic encoding. The current screen probe hash starts `04ed46ee`, while the
final probe starts `8f5733c1`; they share the frozen source and compiler settings
but have different signed executable identities. Full
[screen settings](measurements/2026-10-11-harmonic-pitch-screen-settings.csv) pin
both hashes and the source/evaluator hashes. Source was not retuned after this
screen, and no screen rows are pooled with the final run.

CPU paired medians were −19.7516% at 10 ms and −17.2541% at 20 ms; wall
paired medians were −19.5990% and −17.1232%. All 24 CPU and wall pairs were
faster, and every clip-duration CPU median decreased. The retained individual
CPU ranges are −38.4786% to −14.8278% and −23.0555% to −15.5152%. The
[screen summary](measurements/2026-10-11-harmonic-pitch-screen-summary.csv)
keeps every clip-duration cell and metric, including the −38.4786% CPU outlier.
The final protocol increases frame count and balances order; it runs once,
without deleting observations or repeating until a preferred result appears.

## Reproduction and provenance

The baseline is main
[`765b7e10a0a40c8c1a7c8bc9d64e796c7d730c63`](https://github.com/Mik-pe/snacka/tree/765b7e10a0a40c8c1a7c8bc9d64e796c7d730c63),
with the immutable probe from the [eight-lane analysis audit](harmonic-analysis.md).
Current source is identified by its
[content manifest](measurements/2026-10-11-harmonic-pitch-source.sha256), not an
unpublished revision. [Source and executable provenance](measurements/2026-10-11-harmonic-pitch-source.csv)
connect both binaries to their measured sources. The host is Apple M5, aarch64,
macOS 27.0 build 26A428, Python 3.14.8 and FFmpeg 9.0.2. Rust 1.99.0 uses
compiler commit `b940084d7eb6a299eb4bfeb8e34901bc051e7ac4` and LLVM 23.1.1.

[Commands](measurements/2026-10-11-harmonic-pitch-commands.txt) record builds,
trace checks, timing and linked inspection.
The [artifact manifest](measurements/2026-10-11-harmonic-pitch-artifacts.sha256)
covers this page and all new evidence. Historical quality, resource and
analysis raw/source hashes remain pinned; only updated navigation/document
hashes and hashes of affected artifact manifests change.
