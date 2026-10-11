# Swedish recording diagnostics

The published opt-in [joint harmonic fit](harmonic-joint-fit.md) improves mean
objective quality proxies on this fixed Swedish recording selection at 12 kb/s,
under a newly declared whole-record input preparation step. All 96 V3 pairs have
complete metrics and pass the 76 predeclared gates. This is a diagnostic result
on sixteen already exposed recordings, not an independent held-out test or a
listening result. Harmonic search remains disabled by default. No codec behavior
or public API changes accompany this report.

## Population and source

The source is [Google FLEURS, Swedish `sv_se`, revision
70bb2e84b976b7e960aa89f1c648e09c59f894dd](https://huggingface.co/datasets/google/fleurs/blob/70bb2e84b976b7e960aa89f1c648e09c59f894dd/README.md),
licensed **CC BY 4.0**. Attribution: Alexis Conneau et al.,
[*FLEURS: Few-shot Learning Evaluation of Universal Representations of Speech*](https://arxiv.org/abs/2205.12446).
The official corpus contains read speech. No audio or transcripts are
redistributed here.

Before reading individual metadata rows or audio, the selection rule was fixed:
from the original Swedish test TSV, take the first eight `FEMALE` and first eight
`MALE` rows in recorded metadata order, then merge them back by ordinal. There
are no length, silence, quality or result exclusions and no replacements. The
selected ordinals are **1,2,3,4,5,6,7,8,9,10,11,18,21,28,32,39**. Their sixteen
recordings have ten distinct utterance metadata IDs. Those IDs are not speaker
identities; the speaker count and recording independence are unknown. The test
metadata contains 759 rows: 123 female and 636 male labels, with no `OTHER` row.
The [selection](measurements/2026-10-11-swedish-diagnostic/selection.json) preserves
ordinals, IDs, official labels, declared counts, original filename/archive-member
association, original row hashes and WAV hashes. Transcription fields are
removed. The loader's `ignored_field` remains semantically unproven and is also
removed as opaque metadata; no new meaning is assigned to it.

Pinned downloads were byte-verified before parsing/extraction: test TSV
437,092 bytes (Git blob `0adf1457a95fb5355d15e132324dedbe67226fb2`), original test
archive 440,702,047 bytes (SHA256
`3792fd432675e16d85a67f5caf9927ad608aefc2484d738f26704f75584a5a6f`).
[Download provenance](measurements/2026-10-11-swedish-diagnostic/download-provenance.json)
retains stable official URLs and sizes/hashes; transient signed redirects and
transport headers remain private. No Parquet derivative or dataset loader was
used. Header-only inspection subsequently verified all sixteen selected WAVs:
RIFF/WAVE, format 3 IEEE float32, mono 16,000 Hz; data, `fact` and TSV frame counts
agree. Header agreement is not audio-quality acceptance or proof of source peaks.
The [header observations](measurements/2026-10-11-swedish-diagnostic/source-header-observations.json)
separate actual fields from declared metadata.

## Three separate histories

| Attempt | Outcome | Retained population |
| --- | --- | --- |
| V1 | Import failure before download or audio access: local `select.py` shadowed Python's standard extension | Original failure/log hashes retained; no quality rows produced |
| V2 | Complete lifecycle, quality gate FAIL: seven converted recordings exceeded magnitude1 and failed admission before codec/metrics | All 96 slots retained;54 complete and 42 protocolFAIL;76 gates: 36 PASS / 40 FAIL |
| V3 | New prospective amplitude rules, one separately authorized diagnostic pass: lifecycle and quality gates PASS | All 96 complete; 76 gates PASS; 72 descriptive rows complete |

The narrow V1 module rename fixed import resolution without changing selection
rules. It did not constitute a scored V1 run. V2 performed one pinned direct WAV
to 48 kHz mono f32le conversion per recording. All sixteen retained converted
buffers were finite, but ordinals 3,7,10,18,21,28,32 had magnitudes above 1. Their
peaks ranged up to 1.0658440589904785. The failure was the evaluator's admission
rule. Header inspection cannot establish whether those peaks were already in
the original float samples or arose from resampling.

V2 stays **failed**. Its 54 computed rows and 10 observed individual metric
regressions remain intact; incomplete population means stay blank. V3 does not
fill V2's 42 failed slots, pool its successful rows or regain an unseen-data
claim. V3's protocol was fixed after this diagnostic failure and before any new
normalization or score; codec sources, executables and metric functions stayed
frozen. [V1 failure](measurements/2026-10-11-swedish-diagnostic/v1-import-failure.json),
[V2 raw rows](measurements/2026-10-11-swedish-diagnostic/v2-quality.csv) and
[V2 gates](measurements/2026-10-11-swedish-diagnostic/v2-gates.csv) retain the history.

## V3 input and numerical contract

V3 reuses the exact sixteen raw conversion files retained by V2. There is no new
WAV decode, conversion or corpus selection. For each recording, promote its
nonempty finite f32 samples exactly to f64 and determine the absolute peak P.
If P<=1 (including zero), keep every raw byte, including signed zero/subnormals.
Otherwise compute one f64 gain `g=1/P`, multiply each sample in f64, round once to
little-endian f32 with nearest-even rounding, and verify finite magnitude<=1.
There is no amplification, clean clipping, per-frame gain, second normalization
or reconstruction-side rescaling. Empty/malformed/nonfinite input remains a
retained failure. The same normalized clean buffer supplies both encoders, both
durations and all three conditions. Seven recordings were attenuated; nine
retained byte identity. No nonzero sample underflowed to zero in this run.

The canonical seeded Gaussian helper derives noise energy from that normalized
clean recording, adds the scaled noise in f64, then explicitly clamps mixed
samples to[-1,1] and packs f32. Its unchanged seeds and exact helper bytes/counts
were verified with a separate local-RNG replay. There were **801 clipped mixed
samples across 32 noisy condition buffers**, each shared by both durations. These
are not 32 independent recordings. Pre-clamp peaks, upper/lower counts,
saturation, energy/scale hex values and hashes are retained in the
[noise audits](measurements/2026-10-11-swedish-diagnostic/v3-terminal.json).
Gaussian noise is unbounded; nominal 20/10 dB does not guarantee that achieved SNR
after clipping. No clipping is concealed as normalization.

Whole-record peak preparation uses lookahead and can attenuate an entire
recording because of a single peak. It changes absolute level, fixed numerical
thresholds, quantization decisions, reference/noise amplitude and clipping
incidence; it cannot undo existing distortion. It is **evaluator preprocessing,
not a real-time codec feature**. Its result cannot be pooled with unnormalized
V2 or treated as improved source quality. Perceptual-scale invariance is not
assumed for the codec or numerical metrics.

STOI/ESTOI reference the normalized clean signal; SNR and spectral distance
reference the actual clean/noisy encoder input. The frozen paired seed and
STOI-then-ESTOI call order are unchanged. PySTOI 0.4.1's internal ESTOI perturbation
uses f64 epsilon 2^-52 in spectral row/column normalization; it is not time-domain
encoder PCM dither. Its source/function ASTs and package metadata are bound.
Warnings, short/silent inputs and nonfinite measures must remain blank
UNEVALUABLE, never default scores or reduced denominators. This run had none.

## Results and limitations

The comparison is published main `56a347c` joint fit versus pre-joint main `91fef4a`,
**12 kb/s only**, with harmonic search enabled for both. Current measured binaries
were built from runtime `038a5b8`, whose sixteen pinned source/config/example/helper
files match published 56; they were not rebuilt from 56 for this test. Source roles
and executable hashes are in the [source proof](measurements/2026-10-11-swedish-diagnostic/source-proof.csv)
and [binary roles](measurements/2026-10-11-swedish-diagnostic/binaries.csv).

Both sides receive identical conditioned PCM, 48 kHz mono sample counts,
15/30-byte normal packet and separately counted drain budgets at 10/20 ms, and
120-sample alignment. Endpoint values are canonical six decimals; deltas and
gates use Decimal subtraction of those endpoints. Table means use all sixteen
planned recordings and are rounded half-even to six decimals below; full means and 72
all/female/male descriptive rows remain in the raw CSV. Higher SNR/STOI/ESTOI and
lower spectral distance are favorable.

| Input | Frame ms | SNR change dB | STOI change | ESTOI change | Spectral distance change dB |
| --- | ---: | ---: | ---: | ---: | ---: |
| clean | 10 | +0.213634 | +0.010546 | +0.010852 | -0.122833 |
| clean | 20 | +0.206601 | +0.005464 | +0.007196 | -0.109261 |
| noise20 | 10 | +0.182423 | +0.008516 | +0.007814 | -0.107158 |
| noise20 | 20 | +0.144029 | +0.005707 | +0.006496 | -0.108401 |
| noise10 | 10 | +0.081150 | +0.006916 | +0.003903 | -0.176149 |
| noise10 | 20 | +0.055059 | +0.006954 | +0.003367 | -0.395356 |

All twelve condition/duration STOI/ESTOI mean gates pass (clean strictly positive,
noisy nonnegative). All 64 clean individual floors pass at >= -0.01. The complete
76 gates still require every planned endpoint: no unavailable recording may be
replaced or removed. All 96 rows are protocolPASS/metricCOMPLETE.

There are **13 individual regressions: one STOI and twelve ESTOI**, no SNR or
spectral-distance regressions. Worst STOI is −0.000029 (record 4/noise20/10 ms);
worst ESTOI is −0.004016 (record 2/noise10/20 ms). Clean individual minima are
+0.000018 STOI and −0.000256 ESTOI. All regressions remain in the raw evidence.
Positive means do not guarantee improvement for every input or human preference.
This narrow recording test does not establish speaker-general, conversational,
live-microphone, native 48 kHz, phone/battery, multilingual or subjective acceptance,
or superiority to another codec. No new CPU benchmark was run; the existing
[joint-fit CPU cost](harmonic-joint-fit.md#cost-and-validation) remains relevant.

## Evidence and verification

One scoring pass completed; there was no retuning, source/helper/executable
change, input replacement or rerun. The independent data audit verified all
96 rows, 76 gates, 72 descriptives, 13 exact regressions, 16 normalization and 32
noise byte joins, and 192 full traces. It checked 225,720 normal packets plus 192
drains and 144,460,800 aligned samples, including CRC/sequence/version/profile,
15/30-byte budgets and 120-sample alignment. All checks passed. These trace and
sample counts are repeated contract checks, not additional independent people
or perceptual observations.

- [V3 all endpoints](measurements/2026-10-11-swedish-diagnostic/v3-quality.csv), [all regressions](measurements/2026-10-11-swedish-diagnostic/v3-regressions.csv), [76 gates](measurements/2026-10-11-swedish-diagnostic/v3-gates.csv) and [72 descriptives](measurements/2026-10-11-swedish-diagnostic/v3-descriptives.csv).
- [Separate V2 endpoints](measurements/2026-10-11-swedish-diagnostic/v2-quality.csv), [regressions](measurements/2026-10-11-swedish-diagnostic/v2-regressions.csv), [descriptives](measurements/2026-10-11-swedish-diagnostic/v2-descriptives.csv) and [terminal failure](measurements/2026-10-11-swedish-diagnostic/v2-terminal.json).
- [V3 protocol](measurements/2026-10-11-swedish-diagnostic/v3-protocol.json), [raw reuse bindings](measurements/2026-10-11-swedish-diagnostic/v3-raw-conversion-inventory.json), [terminal/provenance](measurements/2026-10-11-swedish-diagnostic/v3-terminal.json) and [independent data audit](measurements/2026-10-11-swedish-diagnostic/v3-independent-data-audit.json).
- [Source-header observations](measurements/2026-10-11-swedish-diagnostic/source-header-observations.json), [selection](measurements/2026-10-11-swedish-diagnostic/selection.json) and [original 488-artifact hash ledger](measurements/2026-10-11-swedish-diagnostic/v3-private-artifact-hashes.json). The 16 normalization and 32 noise JSON files are verbatim numerical records; hash ledgers do not redistribute PCM or trace payloads.
- [Projection registry](measurements/2026-10-11-swedish-diagnostic/projections.csv), [injective location roles](measurements/2026-10-11-swedish-diagnostic/location-roles.csv) and [public artifact manifest](measurements/2026-10-11-swedish-diagnostic/artifacts.sha256).

The eight V2/V3 numerical CSVs are byte-exact. Path-bearing JSON replaces each
complete private location with its own unique public role, including dictionary
keys and command arguments. The projection registry retains original and public
SHA256, record counts and every removed field. Only selected transcription and
opaque ignored fields and transient signed HTTP/transport fields are removed;
metrics, source/binary identities, regression rows, seeds, budgets and audit
counts are preserved. Public copies are projections of original evidence, not
claims that the projected bytes equal the original private hash. Original
metadata/evaluator packages and raw evidence remain immutable.

This documentation commit does not rerun audio or change runtime/tools/tests.
Historical measurement bytes and report text remain intact; the README link
only requires updating live README/nested-manifest hashes through the older
analysis, pitch and joint bundles.
