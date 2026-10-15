# Default-zero decoder gain cost: frozen macOS run

Before: `d5d3971b7b75137df1d1150fb5df9c4ad7062d5e`. After:
`6c4f916f9f2357dae8b388810a5bb50855ca2e9d`, the ordinary merge of D5 into
gain source `a939e81`. The sole production source difference is
`src/opus/decoder.rs`. Every decoder retains default gain zero; the unchanged
bridge calls no gain setter. Nonzero-gain performance is unmeasured.

Root explicitly coordinated the quiet window. Exactly seven frozen existing
matrix workloads run once as three ABBA/BAAB quartets each, four role processes
per quartet: **84 processes**. Each process retains 1,000 owned and 1,000 native
batch durations. All **168,000 batches** exceed 200 microseconds; the observed
minimum is 584,417 ns. Rust 1.99, release opt3/CGU16/no LTO/default CPU/overflow
checks enabled, wrappers/incremental off. The common bridge requests
user-initiated QoS and checks the setter result; it does not query the effective
class. Affinity, clocks and unrelated OS activity remain uncontrolled.

The cases are WB/20 ms mono 8/48 kHz, WB/20 ms stereo 48 kHz, Hybrid-FB/10 ms
stereo 48 kHz FEC-return, CELT-FB/2.5 ms stereo 48 kHz, and CELT-FB/WB/5 ms
mono 48 kHz. No cases are added or substituted after freezing. Every before/
after untimed full JSON matches. All timed processes retain sample counts,
PCM-bit hashes, entropy intervals and recovery diagnostics. Hybrid FEC-return
contains exactly one actual LBRR event per 24-packet traversal, mixed with PLC
and ordinary return; it measures a complete mixed schedule, not an isolated
FEC callback or deadline tail. Its owned consumed LBRR interval differs from
the native Hybrid PLC debug interval, as documented by the bridge; both remain
unchanged between source variants.

Percentages below are median quartet changes in p50/p95/p99 **batch-average**
costs. Each quartet compares geometric means of its two before and two after
roles. `Norm` divides each owned measurement by its interleaved native
measurement; `Direct` uses owned times. Positive means higher candidate cost.

| Workload | Norm p50 % | Direct p50 % | Norm p95 % | Direct p95 % | Norm p99 % | Direct p99 % |
|---|---:|---:|---:|---:|---:|---:|
| WB20 mono 8k | +0.220 | +0.496 | -0.042 | +0.522 | -1.637 | +6.178 |
| WB20 mono 48k | +0.017 | +0.204 | +0.278 | +0.388 | +0.485 | -0.708 |
| WB20 stereo 48k | +0.136 | -1.216 | -0.349 | -3.277 | -2.042 | -5.324 |
| Hybrid FB10 stereo 48k FEC-return | -0.103 | +0.041 | +0.040 | +0.788 | +1.173 | +2.219 |
| CELT FB2.5 stereo 48k | -0.385 | -0.418 | -0.208 | +0.690 | -2.111 | -2.228 |
| CELT FB5 mono 48k | -0.465 | -0.597 | -0.884 | +0.421 | -1.442 | -0.552 |
| CELT WB5 mono 48k | -0.290 | -0.249 | -0.089 | -0.123 | +0.066 | -0.338 |

WB/20 ms mono 8 kHz p50 changes are +0.574%, +0.220%, +0.019% across the three
quartets. Its direct p50 median is +0.496%. The small normalized median p50
changes do not establish a general speedup; CELT controls also vary, with
negative medians around 0.29–0.46%.

All tail regressions remain visible in
[quartets.csv](quartets.csv) and [acceptance/summary.json](acceptance/summary.json),
including direct and normalized p50/p95/p99 for every quartet. In particular:

- WB20 mono 48k normalized p99 is +17.204%, +0.124%, +0.485%. Its p95 is
  positive in all three quartets (+1.942%, +0.115%, +0.278%).
- Hybrid FEC-return normalized p99 is +0.369%, +26.506%, +1.173%, all positive.
  Direct owned p99 is -1.794%, +99.600%, +2.219%. No cause is assigned to that
  variation without evidence.
- WB20 mono 8k has a +1.350% normalized p99 quartet despite its -1.637%
  median. All three direct p99 changes are positive (+6.178%, +19.591%,
  +6.040%); the direct median is +6.178%.
- WB20 stereo has a +1.462% normalized p99 quartet and direct p99 +24.889% in
  that quartet, despite negative medians.
- CELT FB2.5 stereo has normalized p99 +4.334% and direct p99 +11.099% in one
  quartet. CELT FB5 mono has direct p99 +1.493% in one quartet despite all
  three normalized p99 changes being negative.
- CELT WB5 mono normalized p99 is +3.425%, +0.066%, -6.979%; direct p99 is
  +3.356%, -6.619%, -0.338%.

No tail is discarded. This records default-zero API cost; it is not automatic
admission, full speed parity, proof of no tail regressions, individual-frame
deadline acceptance or a nonzero-gain performance claim. The functional API
tradeoff is reviewed separately from those claims. No variants, substitutions,
discarded trials or reruns were used.

Run: 2026-10-15 09:41:45.840 to 09:44:22.531 UTC, Apple M5/macOS 27. Final
source/library/binary/bridge/tool/reference/input/plan/runner integrity passed.
[audit.json](audit.json) independently rechecks all raw process hashes,
batch minimums and p50/p95/p99. `source.json` pins the full source/build and
binary hashes; `plan.json` is the unchanged pre-run plan. The common bridge,
preparation/measurement runners, seven zero-batch proof pairs, six reused input
files, all 84 process stdout/stderr files and completed runtime metadata remain
alongside this report. Source and result hashes in those original manifests
are unchanged.
