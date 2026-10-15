# Mapped and projection CELT phase control

Source **`567d1b6de5c41593ce1a6437d633a1f01947f60f`**, based on public main
**`ec43d0c352a36a9a70ebe1fbd76d57efbf70a3e7`**. Both decoder families now expose
`set_phase_inversion(bool)` and `phase_inversion()`. The setter forwards to
every private ordinary decoder, including mono and unmapped streams, before
mapping/demixing. The getter reads **stream 0**, matching the observed public
native aggregate control. Reset retains all preferences.

Fresh coupled streams enable inversion; mono streams disable it. A mixed
layout can therefore start with `[true, false, false]` while its aggregate
getter returns `true`. Setting that returned value makes every stream uniform
and can change the mono streams' initial policies. The getter does not claim
uniformity. No mutable child escape, extra field/cache, constructor/default,
reset, DSP, mapping or demixing change was added. This exposes an existing
synthesis policy, rather than a stored control without processing behavior.

## Native-first contract

The installed public headers permit ordinary decoder CTLs on multistream and
projection states (`opus_multistream.h:66–71`, `opus_projection.h:61–63`).
`opus_defines.h:744–766` defines inversion disable/get; it explains the mono
mixing benefit and stereo tradeoff. These headers alone do not define how an
aggregate getter selects a nonuniform child state. Only public headers and
black-box calls were used; no external implementation bodies were read.

Before source implementation or native execution, the separate native-only
[oracle](native/oracle.py), [manifest](native/manifest-before-run.json), exact
libopus 1.6.1/tool/header hashes, and existing **30-byte**
`src/opus/testdata/stereo-phase.opus` were frozen. The oracle emits **1,081**
[raw events](native/events.jsonl): 30 control instances across all five rates,
both families, mono/mixed/more-coupled layouts, plus 60 PCM trajectories and
600 codec calls. It performs no encoding, timing, speech generation or quality
scoring. Transport framing repeats the unchanged existing packet across streams.

All setters, getters, borrowed-child state queries and resets succeed. Initial
child defaults match their mono/stereo output geometry; setting either value
changes every child, including an unmapped one, and reset retains it. Explicit
native-only first/last child changes establish that the aggregate getter reads
stream 0. All 720 mapped/projection snapshots agree with that child; the other
160 snapshots are ordinary-decoder controls. No child is destroyed separately.
The public variadic fixed argument prefixes, promoted integer values, pointer
slots and buffer lifetimes were independently reviewed.

Every codec call returns the expected samples/channel with finite returned
PCM and an untouched suffix. Policy changes PCM in 160/200 matched comparisons
at 12/16/24/48 kHz; this particular 8 kHz witness is insensitive. At 12 kHz the
first mono packet changes by 0.00579839 peak. Mixed default PCM equals neither
uniform policy in those four rates, so the setter changes actual output even
when its value equals the initial aggregate getter. Final entropy ranges do
not change across the 200 policy triples. Sixty reset-decode outputs equal the
first-decode output in their own native trajectories.

The receipt binds all **600 raw f32 step files, 356,400 bytes**, retained in
`/tmp/snacka-opus-mapped-phase-oracle-20261015/`; individual hashes and sizes
remain in [native/receipt.json](native/receipt.json). This folder retains the
oracle, raw event log and compact summaries, avoiding duplicate PCM files.

## Functional proof and the retained test failure

The final local release/reference suite passes **390 tests in 18 suites, zero
failed and zero ignored**. This includes all 20 native-call tests, five
allocation tests and both stack tests: the duplex codec test and the relevant
public Opus construction/reset test, each using a 1 MiB thread stack. Strict release all-target Clippy,
formatting and diff checks pass. Exact flags, commands and log hashes are in
[functional/gates.json](functional/gates.json). This is macOS ARM64 local
validation, not remote CI or other-platform acceptance.

The new public regression has three mapped/projected layouts × five rates ×
three policies (default/disabled/enabled) × ten steps: **450 owned/native pairs**.
It covers coupled/mono defaults, aliases, silence, all native child getters,
normal decode, PLC, CELT absent-FEC fallback, following decode and reset.
Every step checks counts, finite PCM, suffixes and per-implementation entropy
range invariance. At every step, each owned composed output is **bit-exact**
against independently controlled ordinary owned decoders. The square Q15
projection oracle uses independent f64 formulas: its chosen rows contain one
Q15 product or two half-scale terms, making one final f32 rounding exact for
this matrix. This does not assert general native float demix operation order.
The first/reset native waveform witness also retains the existing tolerance.

A private mixed-layout test follows the omitted mono stream's actual scratch
PCM through all ten steps against an ordinary owned decoder, across five rates
and both policies. The existing allocation counter changes both controls,
resets, decodes, conceals and recovers actual LBRR with **zero processing
allocations**. The new phase fixture's recovery is specifically CELT fallback;
that test does not claim actual LBRR coverage by itself.

The original source `2799fbe7d9a475c794a8953b5c3b697af19476f8` failed its new
phase test at the old `same_celt` helper's `finite(owned)` guard requiring
`abs(PCM) < 8`. Both owned and native `is_finite()` assertions had already
passed. The repeated inter-coded fixture grows far beyond eight; that helper
bound is not the floating decoder API contract. The waveform tolerance was
not reached by the failing call. The exact original failure, source ID,
commands and logs are retained under [functional/attempt-1/](functional/attempt-1/gates.json).
An intermediate test-only repair at `9e98c7a2bedc13b08c034c5dd8d2fbb69d073812`
passes its gates and is retained under `functional/attempt-2/`. The final test
adds independent exact composition checks throughout and saves **every one of
450 native/owned waveform deltas**, while keeping the original first/reset
waveform witness. No production code, threshold or DSP was retuned after the
initial candidate. Production controls are identical across these test commits.

The [complete waveform deltas](functional/waveform-deltas.json) are diagnostics,
not a native waveform-parity assertion. Below, steps 0–5 decode the repeated
packet, step 6 is PLC, step 7 requests unavailable CELT FEC, step 8 decodes the
following packet, and step 9 resets before decoding. Each row is a maximum
across 45 owned/native pairs; own/native peaks can come from different pairs.

| Step | Maximum owned peak | Maximum native peak | Maximum absolute delta |
|---:|---:|---:|---:|
| 0 | 0.052098222 | 0.052098207 | 1.4901161e-08 |
| 1 | 0.5977564 | 0.5977564 | 1.1920929e-07 |
| 2 | 4.3209395 | 4.320939 | 9.536743e-07 |
| 3 | 23.819836 | 23.819836 | 3.8146973e-06 |
| 4 | 107.281296 | 107.28128 | 3.0517578e-05 |
| 5 | 409.34125 | 409.34122 | 6.1035156e-05 |
| 6 | 7.9246755 | 347.94254 | 340.40787 |
| 7 | 3.9810097 | 1.1807361e-06 | 3.9810088 |
| 8 | 1282.5282 | 159.69882 | 1122.8293 |
| 9 | 0.052098222 | 0.052098207 | 1.4901161e-08 |

Before loss, all saved waveform deltas stay below 0.0002, despite finite peaks
above 409. PLC and following histories show a large existing native waveform
gap, including maximum delta **340.40787** at PLC and **1122.8293** on the
following decode; reset returns to a maximum delta of 1.49e-8. Exact owned
composition throughout and unchanged processing source establish that the new
forwarding controls do not introduce this gap. This artificial repeated-packet
trajectory does not establish general speech or PLC quality, full native PCM
parity, or quality acceptance. No quality corpus was consumed or retuned.

## Default behavior, layout and generated code

Relative to ec43, only the two owner files change in production, adding the four
control methods. [functional/processing-source-audit.json](functional/processing-source-audit.json)
proves that all **29 existing decoder methods** and both field blocks are
byte-identical, including constructor/reset/processing bodies. The new setters
only update existing ordinary CELT preferences. A default caller gains no new
processing branch/pass, storage or state initialization.

The separate **opus-only** production build uses direct Rust 1.99, opt3,
CGU16, overflow checks on, no incremental/wrappers, default target CPU and no
extra ISA. Cargo `lto=false` permits local ThinLTO across CGUs; neither the
library nor unchanged common bridge requests `-C lto=off` or cross-crate LTO.
The previous `89e2b209` production artifacts are reused as baseline because
Cargo/src/common bridge bytes equal ec43. Exact build commands and source,
tool, library, binary and native pins are retained under `functional/production/`.

All **2,130 existing ordinary-decoder** 142-stream × five-rate × three-schedule
cases run once with `BATCHES=0`. Mapped/projection routing is separately covered
by the new composition tests and unchanged processing source/codegen. Complete parsed result JSON is **exactly unchanged** versus
the pinned `89e2b209` baseline: PCM-bit hashes, sample counts, entropy ranges,
FEC diagnostics and existing waveform-error statistics. No warmup, calibration,
timers or new inputs are entered. [The binding/summary](functional/untimed-comparison/summary.json)
retains zero failures and unchanged input/library/binary integrity. Full raw
rows remain external with their exact hash in `external-artifacts.json`.

Layout stays ordinary **4,280 bytes**, mapped **336**, projection **672**, all
alignment eight. Rust type layout is not an external ABI guarantee.
[codegen/receipt.json](codegen/receipt.json) pins both release rlibs and raw
`nm`/`otool` dumps. The 17 existing emitted mapped/projection functions have
identical static instruction sequences after normalizing function addresses,
branch destinations, unresolved self-relocation slots and local LLVM suffixes.
[codegen/comparison.json](codegen/comparison.json) preserves exact rules and
per-function fingerprints. Immediates, instruction counts and stack operands
remain unchanged. Raw object bytes are not claimed identical; local names and
relocation positions shift. This normalization does not independently prove
linked relocation targets. Together with unchanged field/method source, it
supports no new default-path source processing work; **no CPU cost was measured
and no speed or deadline claim is made**.

## Reproduction

Inspect the retained native manifest and oracle before execution; it invokes
only the exact pinned public native library. Native-only child changes are
oracle diagnostics; the production API exposes no child mutability. Recreate
the source commits and original witness paths, then run the retained functional
commands with the recorded Rust/native flags in a fresh artifact directory.
The preparation/replay scripts preserve historical paths intentionally; they
are reproducibility records, not a request to reuse a completed output folder.
No CPU timing script is added. `evidence-files.json` hashes each retained file;
production binaries, full 2,130-row proof and large disassemblies remain outside
Git and are bound by `external-artifacts.json`. No main merge, push, new remote
CI, or full Opus parity is claimed by this evidence commit.
