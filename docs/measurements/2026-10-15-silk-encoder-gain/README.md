# Exact SILK encoder gain cancellation, 2026-10-15

The eight selected SILK/Hybrid encoder cases improved paired p50 by
1.43–2.56%, with improvement in all 24 case/repetition comparisons. The two
CELT controls did not benefit. This ten-case measurement does not establish
full encoder or codec performance parity.

Before is main `5fc1b43d535d7d8277be456a6ea8fb15c473f916`. After is
`552d0c4380ba77b00b2e4b548dce34fc14f34815`; only
`src/opus/silk/quantize.rs` differs. Process-specific public-encoder sampling on
the preceding `b87f2f8` source attributed 2,336 of 6,080 Hybrid FB/40 ms/mono
leaf samples (38.4%) to the SILK scalar quantizer. Those encoder hot paths are
unchanged in the measured baseline. Profiling used symbols and concurrent host
work, and supplies attribution rather than throughput evidence.

The quantizer previously formed `reconstructed = excitation * gain / 32768`
then divided `reconstructed` by the same gain for predictor history. Every
legal Q8 excitation has at most 23 significant bits and each standardized gain
has at most eight. Their product needs at most 31 of f64's 53 bits; all values
remain normal and bounded. The exact, representable quotient is
`excitation / 32768`, including signed zero. The target-to-pulse division and
reconstruction used for distortion remain unchanged. Own production disassembly
confirms one removed division per predictor-order specialization (eight to six
`fdiv` instructions across the containing function).

## Recorded CPU results

Apple M5, macOS 27, Rust 1.99.0, separately installed libopus 1.6.1. Both
snapshots use production release defaults with explicit 16 codegen units,
overflow checks, no LTO or native-CPU flags, and disabled wrappers/incremental
compilation. The same frozen benchmark bridge requests macOS user-initiated
QoS. Other local codec workers were CPU-idle throughout this accepted run.
Affinity, frequency and unrelated OS activity are uncontrolled.

Each of three repetitions reverses before/after order. Each executable uses
64 changing voiced PCM frames, 128 warmups and 1,000 alternating owned/libopus
batches lasting at least 200 microseconds. Native packets are checked for
matching wire geometry and byte budgets; native decode checks owned entropy
ranges. Percentiles describe paired batch-average throughput, not individual
callback deadlines. Change is `100 * (after_ratio / before_ratio - 1)`;
the table uses the median of the three changes and the median native ratio.

| Encoder case, 48 kHz input | p50 change | p95 change | p99 change | After/native p50 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Hybrid FB 40 ms mono, 32 kb/s | -1.98% | -2.35% | -1.96% | 0.781 | 0.835 | 0.864 |
| Hybrid FB 20 ms stereo, 64 kb/s | -1.98% | -1.75% | -2.77% | 0.810 | 0.854 | 0.866 |
| Hybrid FB 10 ms stereo, 64 kb/s | -1.96% | -2.04% | -2.41% | 0.778 | 0.913 | 0.928 |
| Hybrid SWB 10 ms stereo, 64 kb/s | -1.43% | -1.36% | -1.39% | 0.765 | 0.909 | 0.928 |
| Hybrid SWB 40 ms stereo, 64 kb/s | -1.94% | -2.21% | -2.88% | 0.789 | 0.834 | 0.854 |
| Hybrid FB 60 ms stereo, 64 kb/s | -1.94% | -1.75% | -2.51% | 0.762 | 0.850 | 0.888 |
| SILK WB 20 ms mono, 24 kb/s | -2.56% | -2.46% | -2.75% | 0.357 | 0.471 | 0.524 |
| SILK NB 20 ms mono, 12 kb/s | -2.46% | -2.93% | -2.80% | 0.295 | 0.358 | 0.398 |
| CELT FB 2.5 ms stereo, 64 kb/s, control | -0.08% | -0.21% | +0.56% | 0.588 | 0.603 | 0.631 |
| CELT WB 5 ms mono, 32 kb/s, control | +0.40% | +0.38% | +1.04% | 0.364 | 0.372 | 0.389 |

All active p50/p95 repetitions improve; all active median p99 changes improve.
Individual active p99 regressions remain: FB/60 ms/stereo +2.41% in repetition
0, FB/40 ms/mono +0.32% and FB/20 ms/stereo +0.40% in repetition 2. The first
CELT control's p99 changes range from +0.26% to +6.15%; the second control's
p50 is higher in all three repetitions (+0.20%, +0.40%, +0.62%). These controls
bound interpretation of small changes; no CELT speed improvement is claimed.
Every selected after/native percentile is below one in every repetition.

[summary.csv](summary.csv) retains ranges and native ratios;
[repetitions.csv](repetitions.csv) retains all 90 percentile/repetition rows.
This selected set excludes three earlier M5 CELT tail misses (NB 5 ms mono and
stereo, FB 2.5 ms mono), other signals/rates, and durations above 60 ms. The
existing timing oracle explicitly limits its reference encoder to 60 ms;
80/100/120 ms remain covered by correctness checks. No timing was discarded or
repeated after this accepted run.

## Exactness and existing refusals

The unit test compares all 13,368,960 combinations of 64 gains, 34,815 signed
pulses, three offsets and two modulation signs, plus explicit signed zeros.
The release suite passes 258 unit and 46 relevant integration/native,
allocation and stack tests; strict all-target Clippy and rustfmt also pass.

The frozen before/after trace probe compares all 142 current encoder
geometries across voiced, noise, transient, silence, full-scale clipping and
signed-zero signals (96 logical frames each). It also traverses all eight
development AN4 clips in nine selected geometries. Stereo speech duplicates
mono; each clip is followed by a wrapped prefix, not a zero drain. Evaluation
clips are excluded. Configurations cover ordinary/FEC encoding and continuous
or halfway-reset streams; independent decoders exercise ordinary decode,
periodic PLC return, and skipped loss followed by FEC recovery and current
decode. Packet/input/PCM bits, encoder/decoder ranges, sample counts, metadata,
recovery outcomes and post-refusal continuation agree exactly in all 924 cases:
2,800 encoder traces, 288,686 successful packets, 866,058 output intervals and
3,150 actual FEC recoveries per variant.

The baseline also refuses valid generated full-scale/noise inputs at some
coded budgets. These 15,328 refusals across 222 traces match exactly, including
the error discriminant and complete post-error packet buffer/encoder metadata.
The probe retries the same logical frame at half amplitude until encoding
succeeds; observed retries need at most quarter amplitude. It does not claim
the refused original input encoded successfully. These are an existing
functional gap, separate from this optimization. Counts, amplitude fallback
levels and error hashes remain in [refusals.csv](refusals.csv);
[equivalence.json](equivalence.json) records totals, input hashes and limits.

[source.json](source.json) binds all source/library/binary/bridge/tool hashes,
commands, build/runtime environment, raw result digests and profiling
provenance. [processes.jsonl](processes.jsonl) records all 60 timing processes.
Raw timing CSV/logs and exact trace JSON remain outside Git under
`/tmp/snacka-opus-goal-quantize-5fc1b43`. The recorded proof archived canonical
JSON after asserting both roles equal; successful per-role stdout was not
retained. The reproduction probe preserves both outputs. Final integrity
checks confirm all source and binary hashes remained unchanged.

## Reproduce

Use a fresh directory outside the checkout, direct Rust 1.99.0 and a separately
installed `libopus.dylib`. The optional speech corpus must contain the recorded
`manifest.json` and eight development WAVs; their source and PCM digests are in
`equivalence.json`. Without it, the proof runs all 852 generated cases.

```sh
export SNACKA_BENCH_CHECKOUT="$PWD"
export SNACKA_BENCH_ARTIFACTS=/tmp/snacka-encoder-gain-reproduction
export SNACKA_RUST_TOOLCHAIN_BIN="$HOME/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin"
export SNACKA_REFERENCE_LIBRARY=/opt/homebrew/lib/libopus.dylib
python3 docs/measurements/2026-10-15-silk-encoder-gain/recorded-build.py
python3 docs/measurements/2026-10-15-silk-encoder-gain/recorded-proof.py \
  --corpus /tmp/snacka-opus-goal-speech-corpus-20261015
# Run only after coordinating a CPU-idle window for all local codec workers.
python3 docs/measurements/2026-10-15-silk-encoder-gain/recorded-run.py
```

The scripts record fresh hashes and refuse to overwrite results. Relocated
source/target paths can change binary digests; the archived manifest identifies
the binaries actually measured. Builds and correctness checks can run under
load; accepted throughput measurements require the coordinated quiet window.
