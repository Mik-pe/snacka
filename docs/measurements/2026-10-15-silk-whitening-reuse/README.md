# SILK whitening reuse, 2026-10-15

The owned decoder reuses exact, unscaled whitening numerators across overlapping
voiced subframe windows. It preserves each subframe's original gain, clipping,
LTP scaling and LPC coefficient boundary. Unvoiced calls and partial intervals
contained within one subframe use the original uncached path.

The **after** snapshot is combined commit
`fb5dc6cb295f6fcb561b142d48a8e49f9a9312d0`. **Before** is that identical
snapshot with only `src/opus/silk/synthesis.rs` restored from
`e831eca6ec987eb1110ba9a15d39b9e2a21eddbd`. Neither snapshot is a whole-library
comparison against the older commit. The implementation first landed as
`77d5d06e42559d1b62059aeff2e5df45e8dbc0f7`.

Production release builds use direct Rust 1.99.0 tools, 16 codegen units, no
LTO, the default target CPU, overflow checks, no debug override, no incremental
compilation and no rustc wrappers. Both Rust flag environment variables are
explicitly empty. The bridge uses matching optimization settings. The full
[source manifest](source.json) records commands, environment, source/library/
binary hashes, reference-library hash and the original timing-runner hash.

On Apple M5 / macOS 27, each of the **10 selected cases** ran three repetitions
in reversed before/after order, with 1,000 alternating owned/libopus timing
batches of at least 200 microseconds per executable and user-initiated QoS.
These percentiles describe continuous cached-stream batch averages, not frame
deadline tails. Other local workers were idle during timing. libopus 1.6.1
consumed the same packets and loss schedule.

| Case | Normalized p50 change | After/libopus p50 |
| --- | ---: | ---: |
| SILK WB 20 ms mono, 8 kHz | -4.07% | 1.326 |
| SILK WB 20 ms stereo, 48 kHz | -2.03% | 1.235 |
| SILK WB 20 ms mono, 48 kHz | -4.46% | 1.139 |
| Same, PLC return | -4.96% | 1.090 |
| Same, FEC return | -4.56% | 1.091 |
| SILK MB 20 ms mono, 8 kHz | -3.48% | 1.127 |
| SILK NB 20 ms mono, 48 kHz | -2.02% | 0.987 |
| Hybrid FB 20 ms stereo, 48 kHz | -0.76% | 1.159 |
| Hybrid FB 20 ms mono, 8 kHz | -4.29% | 0.905 |
| CELT FB 2.5 ms mono, 48 kHz, inert control | -0.45% | 1.040 |

Changes are medians of each repetition's after/native ratio divided by its
before/native ratio, minus one. The reported after/native ratios are separate
medians, so dividing the table's rounded medians does not reproduce the change.
All active cases improved p50 in every repetition. The unchanged CELT control
moved between -1.13% and +0.14%, indicating the noise scale.

[Every repetition and percentile](repetitions.csv) includes owned/native
nanoseconds, ratios and normalized change; [the summary](summary.csv) includes
median p50/p95/p99 changes and native ratios. Tail results are mixed: Hybrid
stereo median normalized p99 increased 0.15%, with individual changes from
-20.86% to +3.06%. SILK WB stereo had individual p95/p99 increases of about
0.04%; WB mono 48 kHz had one p99 increase of 0.65%. Most target native ratios
remain above one. NB mono median ratios are below one at all three percentiles,
but one repetition reached native ratios 1.00069 at p95 and 1.01488 at p99.
This evidence does not establish decoder speed parity.

The [untimed equivalence check](equivalence.json) compared complete validation
JSON before and after for all 142 generated streams, five output rates and
decode/PLC/FEC schedules: 2,130 matching cases, including 70 actual FEC cases.
PCM hashes, sample counts, entropy ranges and recovery diagnostics match.
This broad correctness check does not expand the timed coverage or establish
reference quality, physical-device behavior, encoder speed or x86 performance.
Profile/debug builds and the ungated experiment are excluded from these results.

Reproduce from a checkout containing the recorded commits, with the separately
installed reference library and a quiet host:

```sh
export SNACKA_BENCH_CHECKOUT="$PWD"
export SNACKA_BENCH_ARTIFACTS="$(mktemp -d /tmp/snacka-whitening.XXXXXX)"
export SNACKA_RUST_TOOLCHAIN_BIN="$(dirname "$(rustup which --toolchain 1.99.0 rustc)")"
export SNACKA_BENCH_REFERENCE=/opt/homebrew/lib/libopus.dylib
python3 docs/measurements/2026-10-15-silk-whitening-reuse/recorded-build.py
python3 docs/measurements/2026-10-15-silk-whitening-reuse/recorded-run.py --full-validate
python3 docs/measurements/2026-10-15-silk-whitening-reuse/recorded-run.py
```

The runners regenerate the deterministic packets through the frozen reference
generator and keep snapshots, binaries, streams and raw results outside Git.
They verify frozen source and binary hashes before running. Compare regenerated
stream hashes with those recorded in `source.json`; changing the reference
build or architecture can change encoder packet bytes. The recorded scripts
reproduce the build and measurement method; their own hashes are listed
separately from the original one-off runners used for this report.
