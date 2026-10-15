# Ordinary last-packet sample status, 2026-10-15

`Decoder::last_packet_samples()` closes the ordinary
[GET_LAST_PACKET_DURATION](https://opus-codec.org/docs/opus_api-1.6/group__opus__decoderctls.html)
status gap. The value is samples per channel at the configured output rate,
initially zero and cleared by reset. Each successful ordinary decode records
the whole packet duration, including all frames. PLC records its returned
interval. Recovery records the whole requested interval, including any PLC
prefix before the FEC tail. Internal prefixes do not publish intermediate
status. Gain/phase controls preserve it.

Every returned error preserves the preceding successful status. This is also
an explicit owned contract for late numeric `Corrupt`: codec history may have
advanced and an output prefix may have been written, so the existing reset
rule still applies. Native early-error observations below do not establish
native late-fault behavior. Mapped/projection aggregate duration getters remain
unimplemented; their ordinary children retain this single status owner.

## Source and functional binding

Implementation/test commit **`89e2b209184b43de3e69c37ad33f6b00976254f9`** is
based on public main **`dccf36b564afb819e5f206f24ec8a2aa34121ef6`**. This evidence
commit changes documentation/artifacts only. Only `src/opus/decoder.rs` changes
in production: one `usize` status field, getter, initialization/reset, and
successful outer-call writes. The old concealment body becomes
`conceal_unrecorded`; the old recovery body becomes `recover_unrecorded` with
three internal conceal calls redirected to avoid publishing prefixes. DSP,
resampling, entropy, final output and gain arithmetic remain unchanged.

The native oracle was frozen with its public native library, tools and eight
existing packet witnesses **before** compile or execution. It runs ordinary
public calls and duration CTLs only: 80 instances (eight streams × five output
rates × mono/stereo outputs), 960 codec calls, and 1,121 retained JSONL rows.
All 720 successes report returned sample counts; all 160 fresh/reset reads
report zero. All 240 early failures preserve the prior 120 ms status:
malformed framing `-4`, capacity refusal `-2`, and invalid PLC duration `-1`.
Every CTL succeeds; returned PCM is finite and output suffixes stay untouched.
The multi-frame probe is 48 implicit 2.5 ms frames totaling 120 ms; the separate
40 ms recovery probe includes prefix plus tail. These are duration/status
observations, not proof of LBRR presence in every native recovery.

The owned/native regression extends existing gain/FEC histories and the
short-frame witnesses across all five rates and both outputs. It includes
actual SILK LBRR after a nonzero PLC prefix, FEC fallback, 120 ms multi-frame
packets, short implicit PLC, reset, suffixes, overflow/refused geometry, gain
zero restoration and phase controls. A private cached-factor fault injection
checks the owned late-`Corrupt` status contract; it is not a native finite-gain
overflow oracle. Existing processing allocation and bounded-stack tests pass.

Source-bound release checks pass **388 tests in 18 suites, zero failures and
zero ignored**, including 19 native-call tests, five allocation tests and the
stack gate. Strict release all-target Clippy, formatting and diff checks pass.
[functional/gates.json](functional/gates.json) records exact commands, flags and
logs. This is macOS ARM64 local evidence, not remote CI or other-platform
acceptance. No quality corpus, retuning, fresh input generation or score was
used.

For production comparison, baseline `4340e8d16a9251cf83a6635fe01c64dc1f0aa469`
Cargo/src/common bridge bytes equal `dccf36b`; the existing source, rlib and
bridge binary are reused. The candidate is built separately with **opus only**,
not the reference-test feature. All **2,130** existing 142-stream × five-rate ×
three-schedule cases run with `BATCHES=0`: complete parsed result JSON equals
the retained baseline for every case, including PCM-bit hashes, counts,
entropy ranges, FEC diagnostics and waveform-error diagnostics. This preserves
existing waveform behavior; it does not claim native PCM identity. All inputs
and binary/library hashes remain unchanged. Full candidate rows are preserved
externally at `/tmp/snacka-opus-decoder-last-samples-evidence-20261015/production/untimed-comparison/rows.jsonl`
(SHA256 `fd37eb733ec3249ccd592193d86c977b53b7f4c0bd046c9d284e44f931de1d31`);
compact [binding/summary](functional/untimed-comparison/summary.json), replay
script and both build manifests are retained here.

## Layout and generated code

The pinned production layout grows **4,272 → 4,280 bytes**, alignment eight.
Rust type layout is not a stable external ABI. Full `nm`/`otool` dumps are
preserved externally and hash-bound in [codegen/receipt.json](codegen/receipt.json).
AArch64 static instruction counts change: `decode_parsed` 1,010 → 1,011;
`recover_parsed` 451 → 442 (the helper is inlined there); public `conceal`
162 → 26 plus its new 162-instruction unrecorded body; reset 175 → 176.
`write_output` (6,417), `render_lowband` (558) and `conceal_frame` (352) retain
their instruction counts, while operand/address text differs. Neither source
body equality nor instruction count establishes equal cost. The getter has no
standalone library symbol in this build. [codegen/summary.json](codegen/summary.json)
retains symbol names and fingerprints; this prompted the selected cost check.

## Selected M5 cost

The exact frozen [source](cpu/source.json), [plan](cpu/plan.json) and
[runner](cpu/run.py) run once after root coordinated a quiet window. Both sides
use Rust 1.99, opt3, 16 CGUs, overflow checks on, debug0, no incremental/wrappers,
default Apple Silicon target CPU, and no extra ISA flags. Cargo `lto=false`
permits local ThinLTO across the crate CGUs; it is not `-C lto=off`. Neither the
library nor direct-rustc common bridge requests `lto=off` or cross-crate LTO.
Exact build commands and binary/library/tool/native hashes are retained.

The unchanged common bridge `c6adb009e8e889adf358385d1eeb9c69f557289c60d068acb5bae8b2159f7aeb`
uses continuous cached streams and alternating owned/native batches. It excludes
construction/reset, IO, validation, final-range and getter calls from timers.
Default gain remains zero. Seven existing workloads run three alternating
ABBA/BAAB quartets each: **84 serial role processes, 21 quartets, 168,000 raw
native/owned batches**, all at least 200 µs. Every expected PCM/count/range/FEC
check passes, all processes exit zero, and final frozen-file integrity passes.
All outputs, stderr files, individual arrays and percentiles are retained under
[cpu/acceptance/](cpu/acceptance/manifest.json); minimums, sizes and role metadata
are summarized in [cpu/raw-summary.json](cpu/raw-summary.json). `PASS` means
data validation only. There were no variants, retries, substitutions, discarded
roles or selective tails. Preparation status fields remain historical; the
completed manifest records 12:16:05–12:18:40 UTC.

The runtime removes inherited `DYLD_`/`Malloc` overrides then sets
`SNACKA_BENCH_QOS=user-initiated`, `MallocNanoZone=0`, and `RUST_LOG=warn`, matching
the full-M5 protocol. The bridge asserts the QoS setter returned zero; it does
not query the effective class and uses no affinity. Other host environment is
inherited as documented; no complete environment snapshot is claimed.

The Hybrid case is a **mixed 24-packet traversal**: 22 coded calls, one PLC and
one verified actual FEC event at packet index nine. Its owned consumed entropy
interval is `441686442`; native debug interval `17047488` may include Hybrid PLC
RNG and is reported with that qualification. It does not isolate FEC latency or
measure the 40 ms prefix-plus-tail path. Functional tests cover that path.

Positive percentages below mean increased cost. Each quartet compares the
geometric mean of two after versus two before process percentiles. Normalized
values first divide owned cost by the co-measured native cost. Direct values
retain host/context effects; neither is silently substituted for the other.
All three quartets and their medians are shown for every percentile. Reported
ns/operation values are batch averages; their p95/p99 are not individual
callback quantiles.

### p50

| Workload | Native-normalized Q1/Q2/Q3 (%) | Median (%) | Direct owned Q1/Q2/Q3 (%) | Median (%) |
|---|---:|---:|---:|---:|
| SILK WB20 mono 8k | +0.475 / +0.058 / +0.342 | +0.342 | +0.151 / +0.235 / +0.191 | +0.191 |
| SILK WB20 mono 48k | +0.285 / +0.549 / -0.423 | +0.285 | +0.211 / +0.357 / -0.024 | +0.211 |
| SILK WB20 stereo 48k | -0.125 / -0.880 / +0.595 | -0.125 | -0.144 / -0.816 / +0.380 | -0.144 |
| Hybrid FB10 stereo 48k FEC-return | -0.137 / +0.368 / +0.354 | +0.354 | +0.147 / +0.421 / +0.553 | +0.421 |
| CELT FB2.5 stereo 48k | +0.175 / +0.177 / -0.309 | +0.175 | -0.105 / +0.025 / -0.250 | -0.105 |
| CELT FB5 mono 48k | -0.313 / -0.069 / -0.320 | -0.313 | -0.550 / -0.072 / -0.312 | -0.312 |
| CELT WB5 mono 48k | +0.069 / +0.014 / -0.580 | +0.014 | +0.352 / -0.009 / -0.472 | -0.009 |

### p95

| Workload | Native-normalized Q1/Q2/Q3 (%) | Median (%) | Direct owned Q1/Q2/Q3 (%) | Median (%) |
|---|---:|---:|---:|---:|
| SILK WB20 mono 8k | +0.838 / +0.339 / +0.666 | +0.666 | +0.957 / +2.326 / +0.676 | +0.957 |
| SILK WB20 mono 48k | +0.131 / +0.515 / -0.454 | +0.131 | +0.609 / -0.035 / -0.464 | -0.035 |
| SILK WB20 stereo 48k | +0.494 / +0.427 / -0.039 | +0.427 | +1.654 / -4.175 / -0.741 | -0.741 |
| Hybrid FB10 stereo 48k FEC-return | -0.404 / +0.767 / +0.157 | +0.157 | -0.295 / +1.150 / +1.310 | +1.150 |
| CELT FB2.5 stereo 48k | +0.325 / +0.194 / +0.565 | +0.325 | -0.050 / -0.258 / +3.972 | -0.050 |
| CELT FB5 mono 48k | -0.387 / +0.810 / +0.051 | +0.051 | -0.530 / +1.347 / -0.027 | -0.027 |
| CELT WB5 mono 48k | +0.170 / -0.274 / -0.871 | -0.274 | +0.608 / -0.135 / -0.241 | -0.135 |

### p99

| Workload | Native-normalized Q1/Q2/Q3 (%) | Median (%) | Direct owned Q1/Q2/Q3 (%) | Median (%) |
|---|---:|---:|---:|---:|
| SILK WB20 mono 8k | +1.634 / +1.480 / -0.239 | +1.480 | +2.340 / +27.381 / +2.720 | +2.720 |
| SILK WB20 mono 48k | +0.812 / +2.342 / -0.884 | +0.812 | +4.083 / -1.608 / +1.909 | +1.909 |
| SILK WB20 stereo 48k | +1.302 / -2.728 / +2.243 | +1.302 | +4.442 / -8.414 / -2.018 | -2.018 |
| Hybrid FB10 stereo 48k FEC-return | -0.455 / -0.907 / -1.661 | -0.907 | -3.233 / +2.634 / +0.859 | +0.859 |
| CELT FB2.5 stereo 48k | -0.800 / -1.328 / -2.808 | -1.328 | -1.793 / -2.688 / +8.363 | -1.793 |
| CELT FB5 mono 48k | -0.930 / -1.218 / -0.341 | -0.930 | -0.785 / -14.799 / -0.122 | -0.785 |
| CELT WB5 mono 48k | -0.484 / -2.272 / +0.366 | -0.484 | +1.872 / -2.964 / -3.661 | -2.964 |

Normalized p50 medians range from -0.313% to +0.354%; direct p50 medians from
-0.312% to +0.421%. WB mono 8 kHz has positive normalized p50 in all three
quartets and normalized p99 median +1.480%, direct +2.720%. Its second direct
p99 is +27.381%. CELT FB2.5 has third-quartet direct p99 +8.363% despite all
three normalized p99 changes being negative. WB mono 48 kHz normalized/direct
p99 medians are +0.812%/+1.909%; WB stereo normalized p99 median is +1.302%.
All these tails remain part of the record. The API is a coverage addition;
this evidence establishes neither a general speedup, no tail regressions,
full libopus speed/quality parity, hard real-time deadlines, other hardware,
or nonzero-gain performance. No production retuning followed measurement.

## Reproduction and artifact scope

`functional/native/manifest-before.json` binds the eight existing witnesses in
`../2026-10-15-opus-decoder-gain/functional/`; no new corpus was generated.
Compile the retained native-only source with its pinned Rust command and
`-L native=/opt/homebrew/lib`, then pass that witness directory as its argument.
The full compile/run receipts and all error rows remain retained. Recreate the
source commits and inspect `functional/gates.py`, `prepare_production.py` and
`compare_untimed.py` before executing in a fresh artifact directory; historical
paths intentionally identify the original run. The baseline manifest and full
rows remain at `/tmp/snacka-opus-full-m5-explore-4340e8d-20261015/` and are bound
by the new production and cost manifests. Six selected input files are copied
byte-for-byte under `cpu/inputs/` with original paths/hashes preserved.

The separate CPU runner requires new explicit quiet-window coordination; its
argument does not grant authorization to rerun. It retains failed roles and
marks their quartets/cases invalid without retry. There is no hard per-process
watchdog in this frozen runner. Production libraries/binaries and large full
proof/disassembly dumps remain external, identified by exact hashes rather
than copied into Git. [evidence-files.json](evidence-files.json) hashes all
retained files. No main merge, push, new CI or publication is claimed here.
