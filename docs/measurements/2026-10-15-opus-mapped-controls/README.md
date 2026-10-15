# Mapped decoder status and immediate encoder policies, 2026-10-15

Functional API coverage on macOS Apple Silicon. Base public main:
`963b903173ff3a55cb60cfa22b189c5d76e78d48`. Decoder getter commit:
`795a465`; combined source/test head:
`6b50c8ed55eb9a3a978b2ff097f9dd00e171ba00`.
This record does not admit a speed optimization, quality change or full Opus parity.

## Contracts

`MultistreamDecoder::last_packet_samples()` forwards private stream 0's existing
ordinary counter; projection delegates before demixing. Counts use the output
rate, include whole multi-frame packets or the entire requested recovery
interval, and reset to zero. Early refusals preserve them. A successful first
stream followed by a later content error can expose new status despite outer
`Err`; this is not atomic transport metadata. Unmapped stream 0 still owns it.

Both encoder wrappers now accept immediate `reconfigure(&[EncoderConfig])`.
Whole-list ordinary validation/geometry precedes every capacity check and all
mutation. Mapping, channels, rate and duration stay fixed. Original private
packet-buffer **lengths** bound each requested budget, allowing shrink/regrow;
unused neighbouring capacity cannot be borrowed. The active output requirement
is refreshed. Emitted byte budgets still follow actual ordinary packet lengths
plus required delimiters, including fractional clocks; the capacity ceiling is
not an emitted-length promise. Ordinary FEC/DTX/cache/clock rules and previous
packet outcomes are preserved. No fields, queued preparation, DSP or processing
body changes were introduced.

Application changes follow the owned ordinary policy; an explicit mode wins
over application preference. They are not claimed equivalent to native CTL
mutability. Automatic projection design, shared bitrate allocation, extended
float/LSB/complexity/loss controls and native VBR policy remain separate gaps.

## Native-first observations, including exclusions

Every approved original and supplemental oracle ran once after source/tool/
header/library/input freeze, in a new directory. Supplemental layouts are
separate evidence, not replacements or an original all-case pass.

| Run | Accepted construction / attempted | Audio calls | Result and exclusions |
|---|---:|---:|---|
| Original duration V2, mapped | 40/40 | 1,280 parent decodes | 80/80 crossed reset observations select stream 0 |
| Original duration V2, rectangular projection C4/K3 | 0/20 | 0 | Native constructor `-1`; **NOT_COVERED** |
| Supplemental square projection C4/K4, 3 streams/1 coupled | 20/20 | 640 parent decodes | 40/40 crossed reset observations select stream 0 |
| Original encoder, omitted coded mono stream | 0/5 | 0 | Native constructor `-1`; **NOT_COVERED** |
| Original automatic projection encoder | 5/5 | 270 encodes +270 decodes | Finite output/count/suffix checks pass |
| Supplemental fully mapped encoder `[0,1,2,3,255,0]` | 5/5 | 270 encodes +270 decodes | Finite output/count/suffix checks pass |

Selection uses parent20ms decode then child0 CTL reset (`[0,20]`), parent reset,
parent40ms PLC then child1 CTL reset (`[40,0]`). Borrowed children receive only
public queries/reset CTLs, never direct audio calls or separate destruction.
All five rates use the eight frozen old decoder-gain packet banks. Encoder
oracles reuse the existing bounded numeric formula. No fresh speech, timing,
quality corpus or foreign algorithm body is involved.

All 1,920 executed duration calls have independently audited complete120ms PCM
buffers plus seven guard floats. The 180 malformed-framing, short-output and
invalid-PLC errors remain raw (`-4`, `-2`, `-1`). All 360 predetermined valid-
framing content candidates were accepted: native later-content-error coverage
remains **NOT_COVERED**. Private owned crossed resets prove forwarding only.

Native application AUDIO changes after coding returned `-1` for ten original
projection-child sets and fifteen supplemental mapped-child sets; readback
retained VOIP. Aggregate `GET_IN_DTX` returned `-5` in35 rows per encoder run.
These are retained observations, not skipped controls. Requested FEC/DTX alone
does not prove emitted redundancy/suppression. Sequential child controls do not
establish an atomic native list API, automatic matrix equivalence or a shared
bitrate allocator matching owned explicit budgets.

Public specification pointers: [decoder CTLs](https://opus-codec.org/docs/opus_api-1.6/group__opus__decoderctls.html),
[multistream CTLs](https://opus-codec.org/docs/opus_api-1.6/group__opus__multistream__ctls.html),
[encoder CTLs](https://opus-codec.org/docs/opus_api-1.6/group__opus__encoderctls.html).
Installed public headers and libopus1.6.1 bytes are pinned in the retained freezes.

## Owned validation and unchanged defaults

- **395 tests /18 suites /0 failed /0 ignored**, release `opus,reference-tests`;
  strict release all-target Clippy and fmt pass.
- Existing real gain/FEC histories now check counters across all rates,40ms
  concealed-prefix plus actual FEC tail, fallback, continuation and reset.
  Whole100ms packets use packaged `tests/fixtures/opus-short-frame-continuation.bin`.
  Compiled tests never include excluded `docs/measurements` banks.
- Encoder histories compare independent ordinary owners across all five input
  rates,20/120ms, SILK/Hybrid/CELT, fractional CBR/VBR and mode/application changes.
  Full transport bytes use public native padding at the independently derived
  emitted-length target. Late invalid-list cases preserve every configuration,
  clock/cache history and next packet. Original-ceiling regrowth is exercised.
- Reconfigured SILK actually recovers40ms with a nonzero PLC prefix/FEC tail;
  native FEC tail differs from matched native PLC. FEC disable preserves the old
  outcome until a new packet. DTX toggles, partial suppression, reset retention
  and zero Rust processing allocations are checked in both wrappers.
- Source audit: **91 existing method bodies, six owner field blocks and Cargo
  bytes unchanged**. Six owner sizes plus config match the accepted baseline.
  Of45 existing emitted-function normalized instruction texts,44 are exact;
  `algorithmic_delay_samples` differs only in an otool ADRP address comment
  (`0x3000`→`0x4000`). Rust crate disambiguators/LLVM suffixes and branch addresses
  are normalized; linked relocation-target equality is not claimed.
- The separate opus-only production build uses Rust1.99, explicit16CGU,
  opt3/overflow-on, default CPU/local ThinLTO, no wrappers or LTO override.
  **2,130/2,130 complete BATCHES=0 JSON results match phase567d, source-equivalent
  to public963**, including PCM/ranges/counts and metadata:70 actual FEC events,
  710 recoveries/640 fallbacks. All142 old inputs; no timers/warmups/calibration.

Earlier failed attempts are retained: five mistaken getter assertions on an Ogg
test wrapper (compile-only), a compact-packet test oracle that omitted existing
CBR padding, one Clippy constant-chunk style lint, missing named Cargo target
context before compilation, and an unrun implicit-CGU artifact rejected before
the accepted explicit16CGU proof. No production arithmetic or thresholds were
retuned; no native profile or corpus case was silently replaced.

## Reproduction and artifact index

`evidence-index.json` pins the compact files and external raw manifests. The
`native-original`/`native-supplement` directories retain exact oracle sources,
freeze manifests, geometry-only diffs and summaries. Frozen draft protocols
describe their pre-data status; executed observations above supersede no raw row.

External complete evidence roots (raw PCM/packet files stay outside Git):

```text
/tmp/snacka-opus-mapped-controls-20261015
/tmp/snacka-opus-mapped-controls-supplement-20261015
```

Use each frozen manifest's exact commands with a **new** output directory.
For owned checks: Rust1.99 `cargo test --locked --release --features
opus,reference-tests`, native linker search path `/opt/homebrew/lib`; Clippy
adds `--all-targets -- -D warnings`. The production replay source, baseline
binding, manifest and summary are retained separately; `--check-plan` verifies
pins before an explicitly authorized `--proof-run-confirmed` BATCHES=0 replay.
This artifact contains no performance or general native waveform acceptance.
