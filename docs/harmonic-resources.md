# Harmonic speech resource audit

This is the historical four-lane implementation snapshot integrated in main
`b1e55b22c2882d80bafc4f84ad9f850792c369a1`. Its raw measurements and source hashes
remain pinned. See the [eight-lane analysis audit](harmonic-analysis.md) for the
subsequent analysis optimization and the [pitch scoring audit](harmonic-pitch.md)
for the later correlation-loop optimization and compiled costs.

These measurements compare the opt-in harmonic implementation with the preceding
version 7 core, commit `449518da02e3cc768fff273f069cd46c6f9ea535`. Probe and source
hashes identify the measured implementations. The optional encoder changes
packet selection; unchanged PCM is proved separately against its frozen quality
reference. Disabled encoding is checked against the preceding core with strict
complete packet/PCM traces during the default CPU comparison.

## Compiled storage

The [footprint table](measurements/2026-10-11-harmonic-speech-footprint.csv) records
normal Rust 1.99.0 release probes on aarch64 macOS. Flags are opt-level 3,
16 codegen units and overflow checks, without LTO, native-CPU or profiling flags.
Actual emitted prologues include saved registers, page-probe loops and local
storage; counts come from `otool -tvV`. Full
[section sizes](measurements/2026-10-11-harmonic-speech-sections.csv) and relevant
[function frames](measurements/2026-10-11-harmonic-speech-stack.csv) are retained.

| Bound | Previous core | Harmonic implementation |
| --- | --- | --- |
| Encoder's own frame | 18,720 B | 41,952 B |
| Decoder's own frame | 6,080 B | 25,360 B |
| Conservative internal encoder chain | 20,352 B | 43,584 B |
| Conservative internal decoder chain | 7,296 B | 26,576 B |

The fixed frame increases apply even when harmonic encoding is disabled; the
runtime gate can avoid initializing or using scratch but does not remove the
compiled frame. These are conservative bounds on the compiled code's own
successful internal call paths, including standard-library sorting helpers.
They exclude the caller's stream objects and buffers, OS/libm/memory routines,
thread runtime and panic/unwind paths. They are not measured thread high-water
marks or a recommended thread stack size. Other targets or compiler settings
require their own audit.

`Encoder::encode` tail-calls `encode_frame`. Four-lane rendering is inlined into
harmonic synthesis. Supported transforms use half-length complex FFTs of 240
or 480 samples, with six or seven nested 144-byte `split` frames. A decoder's
20 ms inverse path therefore owns 25,360 + 64 + 144 + 7×144 = 26,576 bytes.
Forward, harmonic synthesis and inverse transforms occur sequentially, so their
scratch requirements are not added together.

Encoder sorting covers at most 84 selected indices. Its recursion limit is
at most `2*floor(log2(84)) = 12`, giving at most thirteen 96-byte quicksort
frames. Conservatively including the 336-byte small-sort network, its 32-byte
insertion helper and a 16-byte entry frame adds 1,632 bytes. This overcounts a
released tail-call frame. Selection over at most 960 entries has smaller bounds:
the deterministic fallback recurses on `floor(n/12)`, reaching at most three
224-byte frames; recursive pivot selection divides sections by eight. These
limits use the compiler's pinned official
[sort entry](https://github.com/rust-lang/rust/blob/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4/library/core/src/slice/sort/unstable/mod.rs),
[quicksort](https://github.com/rust-lang/rust/blob/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4/library/core/src/slice/sort/unstable/quicksort.rs),
[selection](https://github.com/rust-lang/rust/blob/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4/library/core/src/slice/sort/select.rs)
and [pivot](https://github.com/rust-lang/rust/blob/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4/library/core/src/slice/sort/shared/pivot.rs)
implementations, combined with emitted specialization sizes.

Transform plans and workspaces allocate at construction. Harmonic processing
uses bounded stack arrays and borrowed buffers, and reuses transform storage.
Allocation tracking covers `alloc`, `alloc_zeroed` and `realloc` after construction
and explicitly verifies that harmonic encoding and decoding occur at both frame
durations. Processing has no heap allocations in the exercised paths; stream
construction is not allocation-free.

## Decoder slope method

A decode probe prepares the same 256 packets before processing its requested
frame count. With an expensive encoder, this fixed preparation can dominate
short decode runs. `tools/measure_decoder_slopes.py` retains raw process totals
at 20,000 and 40,000 frames and estimates
`(CPU_40000 - CPU_20000) / 20000`. Fixed setup cancels algebraically when it is
unchanged. This estimates the incremental cost on the repeated 256-packet
workload, rather than claiming that setup or packet preparation is free.

Each clip/duration has three repetitions. Executable order alternates; count
order also alternates, with an ABBA order bracketing each pair. Raw totals,
wall times, flags, inputs, full trace hashes and evaluator/probe hashes are
retained. Noise between separate processes can produce noisy or nonpositive
slopes; those values are preserved and flagged. This is process CPU on an
uncontrolled desktop, not callback-tail, mobile-energy or worst-case timing
proof. The generic paired process method is described in [CPU measurements](cpu.md).

```sh
python3 tools/measure_decoder_slopes.py corpus results \
  --baseline ./baseline-probe --current ./current-probe --harmonic \
  --bitrate 12000 --repetitions 3 --short-frames 20000 --long-frames 40000
```


## Measured CPU costs

All three comparisons use eight previously evaluated LibriSpeech utterances from
four speakers, recorded in the [input manifest](measurements/2026-10-10-speech-v2-input.sha256),
both durations and three repetitions. The host is Apple M5, aarch64, macOS 27.0
build 26A428, Python 3.14.8 and FFmpeg 9.0.2. Rust 1.99.0 compiler commit is
`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4`, with LLVM 23.1.1. Other agent builds,
tests, scoring and timings were paused throughout these runs; desktop load and
power remain uncontrolled. [Commands and probe hashes](measurements/2026-10-11-harmonic-speech-cpu-commands.txt),
[current sources](measurements/2026-10-11-harmonic-speech-source.sha256) and
[baseline sources](measurements/2026-10-11-harmonic-speech-baseline-source.sha256)
identify the experiment. Raw tables retain input, evaluator, source-manifest and
probe hashes and both harmonic flags. Summaries weight each clip/repetition
equally and retain means, medians, all ranges and slower repetition/clip counts.
Paired percentage medians need not equal ratios of absolute CPU medians.

### Encoder enabled versus disabled in the same binary

The [48 encoder pairs](measurements/2026-10-11-harmonic-speech-enabled-encoder-cpu.csv)
use the same current executable on both sides, at 12 kb/s and 2,000 frames per
process. Only the current invocation enables `--harmonic`. This compares the
optional encoder's actual cost without different library or CLI implementations.
It deliberately permits the separately evaluated packet/audio changes.

| Duration | Disabled median CPU/frame | Enabled median CPU/frame | Median paired change |
| --- | --- | --- | --- |
| 10 ms | 9.093 µs | 146.291 µs | +1499.66% |
| 20 ms | 19.331 µs | 363.341 µs | +1765.36% |

All 48 pairs cost more CPU. Paired increases span +1,131.067% to +2,423.993%.
Mean CPU/frame is 9.284→147.785 µs at 10 ms and 19.341→372.424 µs at 20 ms.
The [summary](measurements/2026-10-11-harmonic-speech-enabled-encoder-cpu-summary.csv)
retains all variation. The enabled medians are about 1.46% and 1.82% of their
frame periods on this host. Those ratios are average throughput context, not
callback deadline or mobile battery acceptance.

### Decoder incremental cost with encoder preparation removed algebraically

Here the previous main probe prepares ordinary packets and the current probe
prepares harmonic-enabled packets, both at 12 kb/s. The
[192 raw runs](measurements/2026-10-11-harmonic-speech-decoder-raw.csv) produce
[48 paired slopes](measurements/2026-10-11-harmonic-speech-decoder-slopes.csv)
using the 20,000/40,000-frame method above. This is a different comparison from
the same-binary encoder measurement.

| Duration | Previous median slope/frame | Enabled median slope/frame | Median paired change | Slower repetitions |
| --- | --- | --- | --- | --- |
| 10 ms | 2.937 µs | 3.529 µs | +21.99% | 23/24 |
| 20 ms | 7.487 µs | 9.023 µs | +19.56% | 21/24 |

Mean slopes are 2.944→3.640 µs at 10 ms and 7.962→8.798 µs at 20 ms.
Mean paired changes are +23.269% and +12.681%. No estimated CPU slope is
nonpositive. However, individual paired changes span −85.698% to +64.547%,
showing noise from subtracting separate process totals. All eight clip medians
increase at 10 ms and seven at 20 ms. The
[summary](measurements/2026-10-11-harmonic-speech-decoder-slopes-summary.csv)
retains these regressions and the apparent improvements; neither outliers nor
fixed encoder setup are silently discarded. The packet mix is the prepared
256-frame cycle, not the worst possible harmonic packet sequence.

### Default behavior versus preceding main

The [288 default pairs](measurements/2026-10-11-harmonic-speech-default-cpu.csv)
compare the current executable with harmonic encoding disabled against preceding
main, at 12/24/32 kb/s. Each process encodes 10,000 or decodes 20,000 frames.
Strict complete packet/PCM traces match for every input, duration and bitrate.
Decoding setup here uses the ordinary encoder on both sides.

| Operation | Duration | Bitrate | Median paired change | Slower repetitions | Slower clip medians |
| --- | --- | --- | --- | --- | --- |
| Decode | 10 ms | 12 kb/s | +0.098% | 16/24 | 6/8 |
| Encode | 10 ms | 12 kb/s | -0.200% | 11/24 | 4/8 |
| Decode | 20 ms | 12 kb/s | +0.041% | 12/24 | 4/8 |
| Encode | 20 ms | 12 kb/s | -1.433% | 5/24 | 1/8 |
| Decode | 10 ms | 24 kb/s | +0.101% | 13/24 | 4/8 |
| Encode | 10 ms | 24 kb/s | -0.228% | 11/24 | 4/8 |
| Decode | 20 ms | 24 kb/s | +0.011% | 12/24 | 5/8 |
| Encode | 20 ms | 24 kb/s | +0.247% | 13/24 | 4/8 |
| Decode | 10 ms | 32 kb/s | +0.275% | 15/24 | 4/8 |
| Encode | 10 ms | 32 kb/s | +0.207% | 18/24 | 7/8 |
| Decode | 20 ms | 32 kb/s | -0.205% | 9/24 | 2/8 |
| Encode | 20 ms | 32 kb/s | +0.146% | 14/24 | 5/8 |

Cell medians span −1.433% to +0.275%; means span −1.249% to +0.685%.
Individual pairs span −8.607% to +14.010%, with 149/288 increasing. These small,
mixed changes establish no broad default speedup or slowdown at this precision.
The fixed stack growth remains even when encoding is disabled. The
[summary](measurements/2026-10-11-harmonic-speech-default-cpu-summary.csv) includes
all absolute CPU means/medians and ranges. Existing ordinary packet/audio
behavior is preserved in the exercised traces; quality gains require enabling
the separately evaluated [harmonic model](harmonic-speech.md).

All published evidence is identified by the
[artifact manifest](measurements/2026-10-11-harmonic-speech-resource-artifacts.sha256).
