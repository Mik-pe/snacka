# Individual encode callback adapter (frozen, verification only)

Production source is `8f723ba1f35c053e1cc840f38a4b20446a127dde`, based on `cca87dd`. Current documentation head `cc4b686516b5527299830177c3ce21ca76f7e7f8` has no source/Cargo difference from that production commit. This external adapter does not modify the encoder. `freeze.json` pins adapter/source, binaries, CLEAN libraries, prepared PCM, packet/range goldens, compiler identity, profile list and exact counts.

The final **73 profiles** comprise 56 clipping matrix cases plus one FEC carrier (57), two original refusal inputs, four mapped/projection compositions, and ten old-success controls copied exactly from the configurations and 64-frame generated voice formula in `/tmp/snacka-opus-silk-sparse-cpu-20261015/throughput.rs` (absolute source path is in the manifest). Each repeat constructs a fresh context and plays the identical complete prepared input history. Composition cases reset immediately before packet8, matching their existing functional proof. Mapped/projection profiles use the existing independently verified gathering/mixing and frozen complete transport packets.

Final `--verify` ran one context per profile and did **not** call `Instant::now` or record durations:

| Role | Profiles | Accepted callbacks | First refusals | Normal accepted controls |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 73 | 668 | 63 | 640 |
| Candidate | 73 | 1337 | 0 | 640 |

Every successful callback matched the frozen packet bytes and available encoder final range. The ten normal profiles matched one shared **baseline** archive for all640 packets/ranges in both roles. Original saved packet archives without a `.ranges` companion validate bytes only in this adapter; prior native/range functional proofs remain separately documented. Refusals must occur exactly at the existing golden prefix boundary and have the expected `Output` / `Encode(Output)` error class. The baseline stops there; it has no inferred successful tail. Candidate input is never rescaled or retried.

Both processing threads requested and read back macOS `QOS_CLASS_USER_INITIATED=0x19` (25), relative priority0, set/get return0. Each entire prepare/process/export run completed on a configured **1MiB** Rust thread. This demonstrates that configured optimized processing stack bound, not peak stack consumption. This adapter adds no allocation claim; existing functional control hooks separately prove allocation-free API calls.

The only future timed interval surrounds one public `encode` call, using an individual `Instant` start/elapsed pair. Inputs, construction, packet allocation, reset, final-range reads, parsing, golden checks, hashing, record insertion and all file I/O are outside that interval. CSV records each actual callback's period, duration and overrun; phase summaries separate first successful, post-reset successful, steady successful, and refusal calls. Percentiles use nearest rank. Refusal latency is descriptive failure cost and is never counted as successful throughput. There is no warm-up exclusion, batching, tail extrapolation or retry.

With the approved16 fresh contexts per profile, planned callback process count is **2 total** (one baseline and one candidate; ordinary, composition and normal cases are integrated in each):

| Role | Fresh contexts | Accepted intervals | Refusal intervals | Total intervals |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 1168 | 10688 | 1008 | 11696 |
| Candidate | 1168 | 21392 | 0 | 21392 |

These are individual callback deadline observations; they are separate from the parent's paired normal/reference throughput experiment. Timing has **NOT RUN**. Do not execute `--measure` until the root explicitly grants the quiet window.

## Exact build and verification commands

The direct toolchain is Rust1.99.0 (`b940084d7`, aarch64 Apple Darwin, LLVM23.1.1). CLEAN library builds and adapter builds use opt3, CGU16, overflow checks on, embedded bitcode off, stripped debug info, and no added targetCPU/LTO/incremental flags. No mbx compiler shim or wrapper is used.

Run `build-and-verify.sh NEW_OUTPUT_SUFFIX` to reproduce compilation, shared baseline normal golden creation, and verification in new output directories. Existing verification results are retained. Absolute library/dependency/compiler paths are in that script. The source code contains `--measure`, but that script invokes only untimed preparation and `--verify`.

After explicit timing authorization, the frozen binaries' planned commands are:

```sh
./callback-baseline --measure --role baseline --repeats 16 --output measured-baseline
./callback-candidate --measure --role candidate --repeats 16 --output measured-candidate
```

Run each to completion before starting the next, under the root's reserved CPU quiet window. These commands were not executed during preparation.

`inputs.f32` concatenates exact native-endian-independent little-endian prepared float values once per profile history, including baseline inputs after its refusal (which the baseline never attempts). `inputs.csv` specifies each packet's offset/count. `freeze.json` contains aggregate and per-profile SHA256, packet counts, per-role expected accepted/refused counts, and golden archive hashes. Both roles' complete prepared PCM and indexes matched exactly.

The original63-profile prefinal verification is retained under `verify-63-prefinal-{baseline,candidate}` with its old compiled binaries. It passed baseline28 accepted/63 refusals and candidate697 accepted/0 refusals. A subsequent independent review identified that the error class was only recorded, not asserted; all actual saved errors already matched expected Output classes. Before final freeze the adapter added the class assertion, ten normal controls and exact input exports. Final73-profile verification reran the unchanged original63 and the added ten. No outcome was discarded and no production source changed.
