# Frozen sparse-fallback macOS CPU acceptance plan

Production candidate `8f723ba1f35c053e1cc840f38a4b20446a127dde`, baseline `cca87dd757d96623b91bff81d34a021e5a3e2792`. No codec source variants. Measurements may start only after an explicit root quiet-window authorization.

## Build and host binding

Both libraries use freshly archived source, direct Rust1.99 toolchain Cargo/rustc, opt3, CGU16, default target CPU, overflow checks on, LTO off, incremental off and empty compiler/workspace wrappers. The first attempted builds still traversed an environment `RUSTC` mbx shim and are excluded. Final artifacts are only `baseline-clean-target`/`candidate-clean-target`; verbose command logs verify the direct compiler. External adapters use the same compiler/optimization/CGU/overflow/LTO settings. Source/library/adapter/binary hashes, compiler version, CPU model, linked native library and host state are frozen in `manifest.json` before measurement.

All timing roles explicitly set/query macOS `QOS_CLASS_USER_INITIATED` (0x19), relative priority0. No targetCPU/native flags, CMake or production foreign-code dependencies. The native black-box reference is installed libopus1.6.1, complexity10, CBR, actual input/channel/frame/packet size equal; restrictedSILK for ordinary SILK, VOIP for Hybrid and restrictedLowDelay for CELT. FEC reference requests20% loss; actual availability is separately reported, not presumed equivalent.

## A. Paired batch throughput, old-success controls

`throughput.rs` records1024 untimed validation callbacks on64 changing generated voice frames before each measured process. It checks full budget, owned decode counts/ranges, packet duration and actual wire TOCs. Its verification mode does not read the clock. During timing all TOCs are accumulated, including warmup/calibration/measured callbacks. Matching means actual codec mode/bandwidth/channels; packet-size/duration checks also run in validation. Invalid native wire is `UNMATCHED` with no parity ratio. FEC availability differs in three controls; native parity ratios are suppressed there even though geometry matches. Their same-native paired ratio is retained only to normalize baseline/candidate CPU drift.

| Case | Owned configuration at48k input |
|---|---|
| nb10m12 | SILK NB10ms mono12kb/s |
| nb10s24 | SILK NB10ms stereo24kb/s |
| mb20m16 | SILK MB20ms mono16kb/s |
| wb10m24 | SILK WB10ms mono24kb/s |
| wb20m12 | SILK WB20ms mono12kb/s |
| wb20s48 | SILK WB20ms stereo48kb/s |
| wb20m24fec | SILK WB20ms mono24kb/s, FEC |
| wb60s48fec | SILK WB60ms stereo48kb/s, FEC |
| hybrid20m32fec | Hybrid FB20ms mono32kb/s, FEC |
| celt20m32 | CELT FB20ms mono32kb/s, untouched control |

Each process warms128 paired callbacks, calibrates both operations to at least200us per batch (target220us), then records1000 alternating owned/native batches at identical packet indexes. Percentiles are batch-average ns/frame and paired ratios, never individual-callback tails. Three independently repeated ABBA quartets per case compare baseline/candidate normalized ratios; retain every raw process and replicate, including regressions. Run cases serially in fixed declared order. Proposed review gate: no consistent material old-success slowdown; parent decides admission from all effects rather than hiding failures in an aggregate. No claim of libopus speed parity when native redundancy workload differs.

## B. Individual callback cost and deadline fractions

A separate adapter times only one public encode/compose API call between two monotonic clock reads. PCM generation, construction/reset, buffer allocation, parsing, output validation, file I/O and percentile calculations stay outside each timed interval. No batched averaging and no native ratio substitutes for the individual distribution. Sixteen fresh encoder contexts repeat the same deterministic traces; first and later callbacks remain separately identifiable. Each row retains raw duration, callback index, configured period, success/error and derived deadline fraction; summary reports p50/p95/p99/max and overruns. These are observed quiet-host callback bounds, not a formal real-time guarantee.

Cover all previously proven57 synthetic refusal profiles (NB/WB,8k/16k native and48k input,mono/stereo,10/20/40/60/80/100/120ms), the exact original NB clipping96-frame and WB6k speech75-frame traces (including frame22), retained-FEC carrier and follow-on ordinary frames, plus four mapped/projection compositions and the ten old-success configurations/64-frame histories listed above. The57 failure matrix comprises56 ordinary clipping profiles andone carrier, so the final individual suite is73 profiles, not74. Baseline stops at its first old refusal and reports that error's cost; it is not called successful encoder throughput and no after-error history is treated as an exact comparison. Candidate must remain successful with the exact previously proved packet hashes. Newly reachable native packet syntax/ranges/PCM were separately validated before timing; native6k WB encoder chooses NB and is explicitly UNMATCHED.

The source cap is96 quantization trials per failed native stage (72 when originally unvoiced/inactive), additional to the unchanged ordinary pipeline. At public120ms stereo there can be12 native coded stages, hence at most1152 fallback quantizations; fixed scratch is reused. The widest/longest traces exercise compound callback cost rather than extrapolating10ms mono measurements. Report any observed callback exceeding its period. Admission still requires root review of absolute deadline margins and all old-success controls.

The final manifest pins122 measured role processes:120 throughput processes (10cases ×3ABBAquartets ×4roles) plus2 individual-callback processes. The latter record33088 intervals: baseline11696 (10688successes/1008firstrefusals), candidate21392successes, with16 fresh contexts for each of73profiles. Prepared PCM is byte-identical across roles, with aggregate and per-profile SHA256.

## Before requesting the quiet window

All adapter sources/binaries must be frozen, both clean library builds completed, verify-only modes and source/output binding checks passed, and helper owned processes CPU-idle. No final timing has yet run. Existing frozen source already passed924cases/2800traces,267374 exact whole-success packets plus1530 exact pre-refusal packets, all288686 candidate full-amplitude packets, original+synthetic native range proofs, composition, zero callback allocations,1MiB stack and terminal append-error/reset checks.
