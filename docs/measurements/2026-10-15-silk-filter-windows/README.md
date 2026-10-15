# Explicit SILK filter windows, 2026-10-15

Four selected active complete-decoder cases improve normalized p50 by
4.71–7.50%, with improvement in every active p50 and p95 repetition. WB/20 ms
mono to 8 kHz now has a median after/native p50 ratio of **1.231**. This
eight-case run does not establish full decoder speed parity; every case's
median after/native ratio remains above one, and several tail/control
regressions remain.

Before is actual main `cca87dd757d96623b91bff81d34a021e5a3e2792`.
After is `c9707899a71469343efa4476617a91ab06ba0e2d`; only
`src/opus/silk/resample.rs` differs, including its new differential test.
The production change prepares the same four FIR history slices with an
explicit array literal. The existing mono/internal-frame-at-least-320 gate,
ring writes, phase, clock, delay, scalar remainder, and FIR accumulation order
are unchanged. Short frames and stereo retain their existing routes.

Earlier process-specific public-decoder sampling attributed 890 of 4,500 leaf
samples (19.8%) to the batched resampler and another 125 (2.8%) to its
`array::map` drain helper. That symbol-enabled profile used an earlier source
with the same resampler preparation and concurrent host work. It supplies
attribution, not throughput evidence. The accepted compiler output removes the
helper and changes the main FIR loop from one scalar tap per iteration to
**four taps per bulk iteration**. Its eight vector2d multiplies calculate 16
products, followed by 16 scalar additions preserving each output's original
tap order. The bulk loop has 65 instructions, one loop branch and no bounds
guards or stack accesses. The usual 46-tap plan uses 11 bulk iterations for
44 taps, then two scalar remainder iterations.

The preserved original [codegen.json](codegen.json) extracted only that after
**scalar remainder**: 16 instructions per tap with no bounds guards, versus
the before loop's 24 scalar instructions and four bounds guards per tap. The
initial report mistakenly described this as the complete after FIR loop.
[codegen-context.json](codegen-context.json) corrects the interpretation and
records both bulk and remainder instructions. Full containing-function
assembly is preserved in [assembly-before.txt](assembly-before.txt) and
[assembly-after.txt](assembly-after.txt). The containing function **grows from
404 to 507 instructions**, while the separate 57-instruction helper disappears.
This clarification changes no source, binary, input or timing results.

## Recorded CPU results

Apple M5, macOS 27.0, Rust 1.99.0, separately installed libopus 1.6.1. Both
snapshots use normal production release settings: opt level 3, debug info 0,
16 codegen units, no LTO, default target CPU, disabled wrappers/incremental,
and **overflow checks enabled by Cargo.toml**, with no overflow-check env
override. RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS are empty during compilation.
The same frozen bridge requests macOS user-initiated QoS. Other local codec
workers were CPU-idle throughout the accepted run; affinity, frequency and
unrelated OS activity are uncontrolled.

Each case uses 24 cached native-generated packets and three repetitions,
reversing before/after role order by case and repetition. Each executable
records 1,000 alternating owned/libopus batches, all at least 200 microseconds,
after warmup and calibration. Timers exclude constructors, resets, IO,
validation and final-range requests. Ratios divide the owned percentile by the
native percentile of batch-average operation times; they are not percentiles
of per-batch ratios or individual callback deadlines. Change is
`100 * (after_ratio / before_ratio - 1)`. Each table entry is the median of
three changes or after/native ratios.

| Complete decoder case | p50 change | p95 change | p99 change | After/native p50 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| SILK WB 20 ms mono → 8 kHz, decode | -6.37% | -6.62% | -7.38% | 1.231 | 1.229 | 1.229 |
| SILK WB 20 ms mono → 8 kHz, PLC return | -7.49% | -7.53% | -7.51% | 1.174 | 1.173 | 1.176 |
| SILK WB 20 ms mono → 8 kHz, recovery fallback/return | -7.34% | -6.96% | -8.12% | 1.178 | 1.178 | 1.178 |
| SILK WB 60 ms mono → 8 kHz, decode | -4.71% | -5.01% | -5.06% | 1.344 | 1.288 | 1.285 |
| SILK WB 10 ms mono → 8 kHz, control | +0.27% | +0.46% | +6.28% | 1.309 | 1.313 | 1.482 |
| SILK WB 20 ms stereo → 8 kHz, control | +0.56% | +0.63% | +0.87% | 1.396 | 1.411 | 1.422 |
| SILK WB 20 ms mono → 48 kHz, control | -0.00% | -0.41% | -1.44% | 1.138 | 1.137 | 1.155 |
| CELT FB 2.5 ms mono → 48 kHz, control | -0.09% | -0.09% | +0.44% | 1.031 | 1.029 | 1.038 |

All 12 active p50 and all 12 active p95 changes improve. All four active median
p99 changes improve, but WB/60 ms repetition 0 p99 regresses **18.27%**.
The WB/10 ms control has p99 changes +6.28%, -0.68%, +13.29% (median +6.28%)
and p50 increases in every repetition. The stereo control's p50 changes are
+3.29%, +0.56%, +0.014%, all nonnegative; its median p99 increases 0.87%.
The CELT control's median p99 increases 0.44%, with a +7.49% individual change.
Small control changes and large individual tails limit interpretation; this
run does not establish tail parity or absence of regressions. No timing was
discarded or repeated.

The timed `fec-return` stream records **zero actual FEC events**: it measures
recovery fallback/return. Actual-FEC CPU acceptance remains open. This timing
set covers only these eight geometries/histories, not the full matrix, other
signals, rates, or all durations. The WB/60 ms input has one 60 ms Opus/SILK frame per packet,
with three internal 20 ms SILK intervals.
[summary.csv](summary.csv) retains each percentile's ranges and native ratios;
[repetitions.csv](repetitions.csv) retains all 72 percentile/repetition rows,
including regressions. [summary.json](summary.json) preserves all three values
for every statistic, and [processes.jsonl](processes.jsonl) records all 48
processes.

## Exactness and source integrity

The new unit test compares 43,008 combinations of all 64 ring cursors, 12 clock
states, four coefficient-prefix lengths, seven input lengths and two history
patterns against the unchanged scalar path. It compares output and mirrored
history bits, cursor and clock, including signed zeros, large finite values,
wraps, batching and scalar remainders. All 259 release unit tests and 25
relevant integration/native, allocation, stack and duration-switch tests pass,
as do strict all-target Clippy and rustfmt.

A separate untimed proof compares the entire parsed before/after JSON for all
142 deterministic generated stream geometries, five output rates and three
loss schedules: **2,130 exact complete-decoder cases**, including 70 with actual
FEC. PCM bit hashes, packet ranges, sample counts and recovery diagnostics
match. It preserves both roles' raw stdout/stderr outside Git. This establishes
output preservation for the tested corpus, not exhaustive bitstream or
perceptual-quality parity. [equivalence.json](equivalence.json) records scope,
all 142 stream hashes, proof runner/row hashes, checks and bounds review.

[source.json](source.json) binds the original source/library/bridge/binary,
tool/reference hashes, commands, build/runtime env, all raw timing digests and
adapted reproduction runner hashes. Final integrity checks found no changes.
Original raw timing arrays/logs, binaries, assembly and full proof rows remain
outside Git under `/tmp/snacka-opus-goal-decoder-windows-cca87dd`; they are local
artifacts, not a durable download. The source manifest retains their digests.

The earlier shared-span experiment `7c327ad` was rejected: its new lane indexing
expanded the hot loop from 24 to 30 instructions and all four active p50 medians
regressed. Its eight-case results remain separately preserved locally under
`/tmp/snacka-opus-goal-decoder-span-023d798`. It is excluded from these snapshots
and results; no rejected timing was rerun or substituted.

## Reproduce

Use a fresh artifact directory outside the checkout, Python 3.12 or newer,
direct Rust 1.99.0 and separately installed libopus 1.6.1. Generation uses only
the native public API and verifies all 142 packet hashes; a differently built
reference that emits other packets stops instead of claiming identical input.
The source must contain both recorded Git commits. Relocating files can change
binary hashes; each reproduction writes its own manifest. No codec source
changes or native source inspection are required.

```sh
export SNACKA_BENCH_CHECKOUT="$PWD"
export SNACKA_BENCH_ARTIFACTS=/tmp/snacka-filter-windows-reproduction
export SNACKA_RUST_TOOLCHAIN_BIN="$HOME/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin"
export SNACKA_REFERENCE_LIBRARY=/opt/homebrew/lib/libopus.dylib
python3 docs/measurements/2026-10-15-silk-filter-windows/recorded-build.py
python3 docs/measurements/2026-10-15-silk-filter-windows/recorded-proof.py
# Only after all builds/proofs and other local CPU work have finished:
python3 docs/measurements/2026-10-15-silk-filter-windows/recorded-run.py --quiet-window-confirmed
```

Production function assembly can be inspected with `xcrun otool -tvV` on each
`benchmark-before`/`benchmark-after` binary; the recorded loop excerpts and
counts are in [codegen-context.json](codegen-context.json), alongside the
unchanged original partial [codegen.json](codegen.json) audit.

The committed runners adapt the executed local runners for relocatable paths,
input generation and explicit quiet-window confirmation. Their hashes are
separate from those of the original executed scripts. They were syntax/help
and source-audited for this report; the accepted timings are from the original
runners, with no additional timing run for the documentation.
