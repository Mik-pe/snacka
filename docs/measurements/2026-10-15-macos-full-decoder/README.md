# M5 decoder exploration: fixed 4340 baseline

This bundle reports public baseline `4340e8d16a9251cf83a6635fe01c64dc1f0aa469`, not later API-delivery heads. At launch, `dccf36b` differed only in documentation; Cargo, source and bridge were identical. Manifest SHA: `3406ad4a01c95411afb4f82e469c1f90944a087e939166ef5cb327a4465daa12`. Rejected experiment 1a was not reused.

All 2,130 cases completed once: zero failures, timeouts or unattempted cases. The 142 original inputs were never regenerated. Source, binary, native-library, input and observation pins passed afterward. Root independently verified 6,390 raw files, 426,000 durations, 19,170 quantile fields and 66,030 stable untimed fields.

## Build and clock protocol

The library uses Rust 1.99 on arm64 macOS/M5: ordinary Cargo release, default `lto=false`, opt3, 16 codegen units, overflow checks, default CPU, direct compiler, no wrapper or incremental compilation. Cargo emitted no `-C lto`: local ThinLTO across this crate’s codegen units is permitted. This is **not explicit `-C lto=off`** or intercrate ThinLTO. The separate unchanged bridge uses direct rustc `-O -C overflow-checks=yes`, default 16 nonincremental codegen units and likewise permits local ThinLTO. Native libopus 1.6.1 is a black-box oracle; production remains owned Rust.

Root owned and authorized the run, 2026-10-15 11:43:56.102458–11:58:18.699828 UTC. Rust `Instant` batches exclude construction, reset, I/O, validation and range checks. A 100 ms paired warmup precedes calibration. Common repeats double until each codec’s minimum of three calibration batches reaches 600 µs; calibration times are not retained. Both decoders then reset once, execute eight warmup batches and 100 alternating-order measured pairs. State remains continuous between repetitions and batches.

All observed durations meet the actual 200 µs floor: minimum 395.333 µs, maximum 137.289209 ms. Each batch repeats 1–32 streams, containing 24–768 public operations. The p50/p95/p99 values are the 50th/95th/99th of 100 **batch averages**; ratios divide marginal quantiles. This is EXPLORATORY100, not 1,000-batch acceptance or individual callback/deadline evidence. Scheduling, affinity and frequency were uncontrolled. QoS setter success was asserted; effective class was not queried. Selected environment was recorded; the remainder was inherited. The retained host snapshots contain selected process names, rather than a full host inventory. They cannot rule out other compiler or background load, or prove continuous isolation.

## Descriptive results

Group values are unweighted medians of case ratios, not workload weights, pooled timings, uncertainty intervals or statistical passes. Separate owned/native medians are not their quotient.

| Mode | Cases | Median p50 ratio | p95 ratio | p99 ratio |
|---|---:|---:|---:|---:|
| SILK | 630 | 1.13777 | 1.13529 | 1.12663 |
| CELT | 1,080 | 1.06850 | 1.08098 | 1.08612 |
| Hybrid | 420 | 1.03111 | 1.05143 | 1.05018 |

All 210 SILK-WB cases exceed native p50 (median 1.23189), as do all 84 Hybrid 24 kHz cases (1.25024) and 84 Hybrid 48 kHz cases (1.17763). The corresponding Hybrid 16 kHz cases fall below one (.89550). These suggest sampling shared WB, stereo and output-render work; they do not establish causality.

| Quantile | Largest retained ratio case | Ratio | Owned/native µs per operation |
|---|---|---:|---:|
| p50 | SILK-WB-20ms-2ch.bit-8000-decode | 1.393772 | 17.990 / 12.907 |
| p95 | CELT-SWB-60ms-2ch.bit-12000-fec-return | 2.665972 | 1,645.668 / 617.286 |
| p99 | Hybrid-SWB-40ms-2ch.bit-48000-fec-return | 3.682063 | 552.776 / 150.127 |

All tails remain intact, including large tails on both codecs. Fixed order, one process per case, scheduler effects and 100-batch resolution prevent attributing individual spikes to codec-caused callback stalls. Nothing was trimmed, replaced or rerun.

## Geometry, recovery and waveform limits

Filenames match actual mode, bandwidth, packet duration and channels. Packet duration is not elementary codec-frame duration; packed packets contain multiple frames, and SILK has internal blocks. Streams are homogeneous; mapped/projection, transitions and channel expansion are outside scope.

Each stream has 24 actions. PLC-return conceals actions 8/9. FEC-return conceals 8, recovers 9 from packet 10, then decodes 10 normally. Timing covers the whole mixture. Counters describe initial untimed validation, not every timed callback: 47,925 coded, 355 implicit-PLC, 2,130 explicit-PLC and 710 recovery operations. Even decode-only includes implicit PLC in 70 SILK mono cases across 14 streams.

Only 70/710 recoveries consume owned LBRR: SILK 50/210, Hybrid 20/140, CELT 0/360. The other 640 fall back; all 90 twenty-millisecond FEC cases fall back. The flag observes owned `Recovery::Fec`, not complete packed-frame recovery. Actual native LBRR consumption is uninstrumented. All 140 Hybrid recovery debug intervals differ; these are diagnostics, while ordinary/PLC range gates pass. No absolute FEC-path parity claim follows.

Unshifted PCM does not establish waveform or perceptual equivalence. For decode-only, Hybrid error/native-RMS medians at 8/12 kHz are .90321/.88922, versus .00121/.00142/.00142 at 16/24/48 kHz. SILK decode-only at 8 kHz is .88104 with mixed bandwidths/implicit PLC. Alignment, delay, cause and perceptual significance are unmeasured. Pure CELT decode-only has maximum absolute error ≤1.0431e-7 across 360 cases. Speed does not establish quality parity.

## Files and provenance

- `all-cases.csv`: all 2,130 IDs, statuses, three owned/native quantiles and ratios, validation FEC/fallback/operation mix, repeats and batch operation counts.
- `groups.csv`: three mode and 45 mode/bandwidth/output-rate groups.
- `provenance.json`: SHA references to full raw records, their hash index, frozen sources/scripts, reports, independent analysis and root audit. This is an external-data index, not self-contained raw reproduction.
- `files.sha256.json`: this small bundle’s hashes. Large arrays and PCM remain external.

No new codec run, timing, corpus, optimization, retuning, merge, push or PR accompanied publication. The original README SHA `f1faac94ac1fabd7c267b02e2d8f39fffa8bf64a8172c5d1b615bcf7547c5057` remains in external publication history.
