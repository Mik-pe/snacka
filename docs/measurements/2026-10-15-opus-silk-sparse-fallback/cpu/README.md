# Frozen M5 sparse-fallback cost and admission evidence

Status: **functional bounded fix accepted after local integrated checks**. Root reviewed the complete functional and CPU evidence and accepted this disclosed cost/coverage tradeoff. Source-only integration commit `76224786eef73423a51b16f17c126ec07c534d24` passes 380 release tests across 18 suites, formatting and strict release Clippy. Exact-head package and direct-main push remain separate; remote publication is not claimed by this report. This is a coverage correction with measured old-path overhead, not a performance win or perceptual-parity claim.

Production source is `8f723ba1f35c053e1cc840f38a4b20446a127dde`, baseline `cca87dd757d96623b91bff81d34a021e5a3e2792`. Documentation commits do not change that production source. The frozen premeasurement manifest SHA256 is `8aa1a4bb595df7526c8909e203a53421f31b2d9f668a2ab1674f33c74adfe47b`; callback freeze SHA256 is `8e1968a1564dc51f760cf64977a9bce447c40a271b196af2e84b10410e556220`.

Exactly 122 measured role processes ran once:120 throughput processes and2 individual-callback processes. Every raw row is retained. No runs were repeated, discarded or replaced, and codec source remained frozen. Post-run integrity checks confirm all 28 pinned measurement files, prepared inputs and native library unchanged. The independent reviewer recomputed all 30 ABBA effects and checked hashes/counts/contexts.

## Old-success throughput controls

Apple M5, macOS, direct Rust 1.99 toolchain, opt3, CGU 16, default target CPU, overflow checks enabled, LTO/incremental/compiler wrappers disabled. Both before/after roles use the same settings and explicitly set/query user-initiated QoS 25, relative priority 0. First attempted mbx-shim builds were excluded; only the clean targets were measured. The actual verbose build commands are archived.

Each control has three ABBA quartets (baseline,candidate,candidate,baseline). Each process validates 1024 callbacks on 64 changing generated voice frames, warms 128 pairs, calibrates both operations to at least 200 µs per batch, then records 1000 alternating owned/native batches. These are batch-average throughput percentiles, not individual callback tails. Percent change is the candidate/baseline paired-native-normalized cost in each quartet; positive means slower.

| Control | Repeat1 | Repeat2 | Repeat3 | Median cost change | Native workload |
|---|---:|---:|---:|---:|---|
| nb10m12 | +2.723% | +1.066% | +4.197% | +2.723% | matched geometry |
| nb10s24 | +1.935% | +1.161% | -0.516% | +1.161% | matched geometry |
| mb20m16 | +1.866% | -0.282% | +0.204% | +0.204% | matched geometry |
| wb10m24 | +0.675% | +0.198% | +1.706% | +0.675% | matched geometry |
| wb20m12 | +1.175% | +1.336% | +1.579% | +1.336% | matched geometry |
| wb20s48 | +1.214% | +1.612% | +1.203% | +1.214% | matched geometry |
| wb20m24fec | +1.002% | +0.725% | +0.651% | +0.725% | matched geometry; unequal FEC availability |
| wb60s48fec | +1.264% | -0.575% | +0.731% | +0.731% | matched geometry; unequal FEC availability |
| hybrid20m32fec | +0.318% | +3.135% | +0.414% | +0.414% | matched geometry; unequal FEC availability |
| celt20m32 | +0.060% | -1.147% | +0.102% | +0.060% | matched geometry |

The largest median regression is NB 10 ms mono 12 kb/s, +2.723%, with all three quartets positive (+1.066..+4.197%). WB 20 ms mono 12 kb/s and stereo 48 kb/s are also consistently slower by about 1.2–1.6%. The unchanged CELT control median is +0.060%. These effects are retained; this does not satisfy a strict no-regression claim. Root accepted them as the cost of removing previously observable valid-input failures.

Native reference is public libopus 1.6.1 at complexity 10/CBR. Ordinary SILK explicitly uses `OPUS_APPLICATION_RESTRICTED_SILK=2052`, unlike the older VOIP-based reference runs; Hybrid uses 2048 and CELT 2051. All ten controls actually retain matched codec mode/bandwidth/channels and validated packet size/duration, including collected timed TOCs. The three FEC controls request 20% native loss but have different actual LBRR availability: owned 1023 of 1024 versus native 101 (WB 20 ms mono), 738 (WB 60 ms stereo), 103 (Hybrid). Their absolute native parity ratios are suppressed. Their native pair is used only to normalize before/after CPU drift. Ratios across older reference applications are not directly interchangeable. Matching bytes/geometry does not imply matching speech quality.

## Individual callback observations

These measurements are separate from throughput. One process per role runs 73 profile families with 16 freshly constructed contexts per family:56 clipping profiles,1 retained-LBRR carrier, 2 original refusals, 4 mapped/projection compositions, 10 normal controls. Every complete deterministic history repeats unchanged. Prepared PCM is byte-identical across roles (9692160 bytes once per prepared profile set), with aggregate and per-profile SHA256 in the callback freeze.

Only one public encode/compose call lies between clock reads. Construction, reset, PCM generation, allocation, parsing, golden packet/range checks, hashes and file I/O remain outside that interval. No warmup or first-output sample is discarded. All processing/constructors ran on a configured 1 MiB thread and returned the requested QoS. Existing separate hooks establish zero callback allocations.

**The 16 contexts are repeated deterministic histories inside one host process, not16 independent host/scheduler replicates.** Baseline and candidate each ran in one process. Normal-callback p50 differences cannot establish a causal speed change; use the repeated ABBA controls above for old-path overhead. These are observed absolute deadline margins on this quiet M5 host, not universal real-time bounds.

| Role | Successful calls | Expected first-refusal errors | First successful outputs | Post-reset successful calls | Period overruns |
|---|---:|---:|---:|---:|---:|
| Baseline |10688|1008|192|0|0|
| Candidate |21392|0|1168|64|0|

Baseline errors are 944 `Output` and 64 composed `Encode(Output)`. It stops at the first known failure; failure cost is not successful encoder throughput. Of these 1008 refusals, 976 occur on the first callback and 32 follow the unchanged WB-speech/FEC-carrier prefix. Candidate provides all planned outputs. Packet/range goldens are checked outside every timed callback.

All 33088 observed intervals are within their configured period. The largest absolute candidate call is 23.568 ms on WB 120 ms stereo, 19.640% of that period. The largest fraction is 2.735 ms on WB 10 ms stereo, 27.350% of 10 ms, leaving 72.650% observed margin. They are different samples.

| Candidate profile | Calls | Period | p50 | p99 | Maximum | Maximum/period |
|---|---:|---:|---:|---:|---:|---:|
|original-nb-refusal|1536|10ms|0.319ms|0.568ms|0.589ms|5.886%|
|original-wb-refusal|1200|20ms|0.168ms|1.009ms|1.661ms|8.303%|
|fec-carrier-nb10-20k|224|10ms|0.032ms|0.524ms|0.559ms|5.589%|
|clip-wb-10ms-2ch-input48000-24000|128|10ms|1.486ms|2.721ms|2.735ms|27.350%|
|clip-wb-120ms-2ch-input48000-24000|128|120ms|18.614ms|23.040ms|23.568ms|19.640%|
|mapped-fallback-12000|256|10ms|1.019ms|1.735ms|1.766ms|17.661%|
|mapped-fallback-16000|256|10ms|1.007ms|1.700ms|1.712ms|17.123%|
|projection-fallback-12000|256|10ms|1.322ms|2.239ms|2.261ms|22.607%|
|projection-fallback-16000|256|10ms|1.327ms|2.205ms|2.209ms|22.094%|

Complete per-profile/phase raw rows and summaries are archived. Mixed-duration aggregate percentiles are descriptive distributions and must not be treated as 10 ms callback deadlines. The source bound remains 96 extra causal trials per failed native stage (72 when originally unvoiced/inactive), at most 1152 at 120 ms stereo, with reused fixed scratch. An observed maximum does not prove a worst-case bound for all valid PCM, host load or scheduler conditions.

## Remaining gates and quality limits

The full 924-case proof preserves 267374 packets from 2578 entirely successful baseline traces, plus 1530 successful-prefix packets from 222 previously failing traces. The candidate completes all 288686 full-amplitude packets with no refused trace. Those tails after old failures are independently validated rather than called bit-identical baseline continuations. Existing native, FEC, stereo, grouped, mapped/projection, allocation,1 MiB stack and terminal append-error/reset evidence remains in the parent report.

The fallback cannot guarantee every valid low-budget input: retained prior LBRR or minimum syntax can still leave no fitting candidate. Deferring prior LBRR would require a separate complete causal plan/cache restart and was not added. Standalone appends to an existing entropy writer retain the late-Output/reset contract.

Quality parity remains open. The original NB clipping diagnostic has native-decoded candidate SNR 1.166 dB versus 4.588 dB for a matching NB 10 reference. WB 6 kb/s candidate remains WB 20 ms but native reference chooses NB 20 ms (TOC 8/11): that 1.213 versus 4.060 dB diagnostic is explicitly UNMATCHED in bandwidth, not a WB parity comparison. Native-rate peer attribution shows that most NB 48 kHz maximum error comes from the existing unclamped interpolator overshooting ±1, not a packet/range mismatch; no decoder change was made in this source. No fresh corpus, reserved Swedish sample, perceptual acceptance or quality tuning was used.

Root functional admission follows the local integrated-check checkpoint above. Package/push and remote checks are separate from this source-bound measurement. The patch is not advertised as full functional, speed or perceptual parity.

## Reproduction and retained evidence

`PLAN.md`, `manifest.json`, executed adapters, coordinator, compiler logs, all 120 throughput rows plus original stdout/stderr, both individual CSVs/summaries/input indexes, and postprocessing source are archived here. Large prepared PCM, libraries and binaries stay outside Git at `/tmp/snacka-opus-silk-sparse-cpu-20261015` and `/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015/controls/callback-latency`, bound by hashes. All original verification attempts remain external; the callback README records the prefreeze assertion/control additions.

The executed timing commands were:

```sh
python3 /tmp/snacka-opus-silk-sparse-cpu-20261015/run-throughput.py --measure
./callback-baseline --measure --role baseline --repeats 16 --output measured-baseline
./callback-candidate --measure --role candidate --repeats 16 --output measured-candidate
```

Commands run serially after explicit root quiet authorization. They are recorded for reproduction in fresh output paths; existing measured outputs must not be overwritten. `aggregate.py` only reads saved rows; it performs no codec calls or new timing.
