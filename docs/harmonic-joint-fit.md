# One harmonic joint-fit trial

The opt-in harmonic encoder now tries one additional fit for the pitch whose
original quantized render had the best transform score. At 12 kb/s, average
intelligibility proxies improve on the measured speech selections. This costs
more encode CPU: the final balanced desktop comparison shows median paired
increases of 54.37% at 10 ms and 35.44% at 20 ms. Harmonic search remains disabled
by default.

This is an original Rust implementation of the mathematical model below. No
external codec implementation was copied or translated. There are no new
library dependencies, trained weights, processing allocations or predictor
history. The [harmonic wire format](harmonic-speech.md) and decoder are unchanged:
existing version 7 decoders that implement profiles 14/15 accept these packets
without a new setting or negotiation requirement.

Enable the experiment with `Config::with_harmonic_speech(true)`, the monitor's
`--harmonic` option or **H** key:

```sh
cargo run --locked --release --features live-audio --example monitor -- --bitrate 12000 --harmonic
```

The existing eligibility gate permits harmonic trials through 16 kb/s. Quality
and CPU evidence here covers **12 kb/s only**; it does not establish benefit at
14 or 16 kb/s. Default-OFF and harmonic-enabled 24/32 kb/s packet and PCM outputs
remain exactly unchanged. Complete packets still occupy 15/30 bytes at 10/20 ms,
including the four framing/checksum bytes, plus one separately counted drain
packet. The same 120-sample (2.5 ms) transform delay and frame buffering remain.

## Fit and local objective

First evaluate all eligible original harmonic candidates (up to eight) in their
unchanged order, alongside the ordinary scalar and pulse candidates. Record the
pitch with the best **original quantized render** score, retaining the first
candidate on equal scores. Recompute its analysis in existing scratch and attempt exactly one
joint correction; skip it if there was no eligible original render.

Let N be 480 or 960, L=N+120, and A contain cosine/sine columns for the harmonics
of the selected pitch. The actual stored f32 overlap ramp is converted to f64.
A folding operator T describes the defining short-overlap MDCT energy. For
an input or residual vector e, its rows are:

```text
leading t=0..59:   r[t]*e[t] - r[119-t]*e[119-t]
middle t=120..N-1: e[t]
trailing t=0..59:  r[119-t]*e[N+t] + r[t]*e[N+119-t]
```

The ideal coefficient-error energy is `(2/N)*||T*(x-A*a)||²`. Set
`b=Aᵀ*Tᵀ*T*x`, `G=Aᵀ*Tᵀ*T*A`, and residual gradient `g=b-G*a`.
Analytic complex moments compute its harmonic blocks without a persistent or
dense Gram matrix. Warm-start from the original separate harmonic fits and
perform exactly two block-coordinate passes, ascending then descending. Each
2×2 step uses `delta=G_hh⁻¹*g_h`, adds it to that harmonic's parameters and
updates every affected gradient block.

This is a bounded correction, not a converged global least-squares solve.
Finite/positive determinant and minimum-eigenvalue checks refuse unsafe blocks.
A conservative pair-norm limit of 1024 provides numerical rollback; it is not
a clipping operation or perceptual weight. Any refusal leaves the original
candidate set available. Supported pitches are 75–400 Hz with at most 106
harmonics; the unchanged 10 ms eligibility gate excludes pitches below 120 Hz,
so that encoder path has at most 66 harmonics.

Quantize the corrected amplitudes with the existing paid gains and pulse-vector
budget, render through the actual f32 forward transform, and select it only if
its full-spectrum squared coefficient error strictly improves on the overall
original winner. This preserves the original **per-frame coefficient objective
floor**. It does not guarantee better overlapped/clipped PCM, SNR, STOI, ESTOI,
spectral distance or subjective sound for every clip. No free pitch, phase,
envelope fields or additional bitstream profile are introduced.

## Frozen quality

The candidate was frozen before scoring. The baseline is main `91fef4a`;
research runtime `2eb8a40` and integrated runtime content `038a5b8` are identical.
The integrated build reproduces every complete packet/PCM trace in all 432
known and 96 new cases, including canonical roundtrip output on all new cases.

Known data contains 72 previously exposed clips from 36 speakers, with three
input conditions and both durations: 432 pairs. Its eight development clips
are a subset; all 48 development rows repeat exactly, not 48 extra observations.
The new protocol excludes all 40 previously exposed test-clean speakers. From
each of dev-clean and dev-other, it takes the first two eligible F and first two
eligible M readers in official metadata order, then the first two FLAC members
per selected voice in official archive order. No length, silence, transcript,
quality or result exclusion or replacement is allowed. The 16 clips/8 voices
produce 96 pairs. Selection was frozen before audio access or scoring; no model
changes followed any scores. These repeated/noisy cases are correlated clips,
not 96 independent speakers or a human listening study.

Audio is [LibriSpeech](https://www.openslr.org/12), CC BY 4.0. Attribution:
Vassil Panayotov, Daniel Povey, Guoguo Chen and Sanjeev Khudanpur, *LibriSpeech:
an ASR corpus based on public domain audio books*, ICASSP 2015. No audio or
transcripts are redistributed.

Both encoders receive identical 48 kHz mono PCM, packet and separate drain
budgets, sample counts and delay, with harmonic search enabled. Existing seeded
noise and paired metric-dither methods are unchanged. STOI/ESTOI reference clean
speech; SNR and spectral distance reference the actual encoder input. Higher
SNR/STOI/ESTOI and lower spectral distance are favorable. New tables and gates
use Decimal subtraction of canonical six-decimal baseline/current endpoints.
Raw historical delta columns remain verbatim; separate rounding can differ
from endpoint subtraction by at most 0.000001. Derived means are exported at
12 decimal places and rounded to six here, not additional measured precision.

| Selection | Input | Frame ms | SNR change dB | STOI change | ESTOI change | Spectral distance change dB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Known 72 | clean | 10 | +0.198236 | +0.009222 | +0.010392 | -0.173039 |
| Known 72 | noise20 | 10 | +0.171983 | +0.008005 | +0.008482 | -0.102068 |
| Known 72 | noise10 | 10 | +0.084664 | +0.006305 | +0.004499 | -0.145973 |
| Known 72 | clean | 20 | +0.230659 | +0.005938 | +0.008154 | -0.167390 |
| Known 72 | noise20 | 20 | +0.185841 | +0.005877 | +0.007892 | -0.135866 |
| Known 72 | noise10 | 20 | +0.084449 | +0.004764 | +0.003600 | -0.364678 |
| New 16 | clean | 10 | +0.189320 | +0.011615 | +0.014575 | -0.193381 |
| New 16 | noise20 | 10 | +0.162402 | +0.008826 | +0.009513 | -0.090856 |
| New 16 | noise10 | 10 | +0.080211 | +0.008173 | +0.006736 | -0.148182 |
| New 16 | clean | 20 | +0.143258 | +0.004835 | +0.007902 | -0.171975 |
| New 16 | noise20 | 20 | +0.110980 | +0.004377 | +0.007235 | -0.132039 |
| New 16 | noise10 | 20 | +0.057214 | +0.005162 | +0.005760 | -0.339832 |

All six condition means improve in both selections. Known data retains **32
STOI regressions** (worst −0.004788), **39 ESTOI** (−0.017556) and **eight
spectral-distance** regressions (+0.019406 dB). New data retains **three STOI**
(−0.002313), **seven ESTOI** (−0.003617) and **two spectral-distance** regressions
(+0.024989 dB). Neither selection has an observed SNR regression; this is an
empirical result, not a mathematical guarantee. Every regression remains in
the raw evidence.

The predeclared clean-mean STOI/ESTOI gains and nonnegative noisy-mean gates pass
for both durations. New data also passes every clean-clip floor of −0.01; all
96 expected rows have complete metrics and valid packet/PCM contracts. The
known screen did not introduce a posthoc individual-clip gate. Read English and
synthetic noise do not establish live microphone, conversational, multilingual,
music, mobile or subjective listening acceptance, or superiority to another
codec.

Packet-change telemetry is an inference after independent original-only
control proof, not an internal correction-winner counter or new wire flag.
Across known data, corrected packets occur in 19,474/158,757 ordinary frames
at 10 ms and 10,171/79,425 at 20 ms, with no changed drain packet. Full profile
counts, encoded-pitch histograms and packet/PCM hashes remain available.

## Cost and validation

The final paired encoder comparison uses eight clean development clips, both
durations, four repetitions and 10,000 frames per process. Pair order is balanced:
baseline first in repetitions 0/2, current first in 1/3. No retry, filtering or
quality-based selection is performed. Each process cycles its first 256 prepared
frames, covering different utterance spans at 10 and 20 ms; this is not timing
the complete utterance. Process CPU averages include fixed startup, construction
and preparation costs; they are not frame-tail measurements.

| Mode | Frame ms | Median paired CPU change | Median paired wall change | Baseline/current median CPU µs/frame |
| --- | ---: | ---: | ---: | ---: |
| Harmonic ON | 10 | +54.3665% | +54.3113% | 94.031 / 147.331 |
| Harmonic ON | 20 | +35.4353% | +35.0250% | 241.234 / 320.712 |
| Default OFF | 10 | −0.2201% | −0.1745% | 8.085 / 8.083 |
| Default OFF | 20 | −0.0954% | −0.0756% | 18.137 / 18.120 |

Both harmonic medians satisfy the predeclared maximum +100% overhead. All 64
harmonic CPU and wall pairs are slower; worst changes are +67.8512% CPU and
+73.8259% wall. Default mode has 29 CPU and 28 wall regressions despite its
small negative medians; worst changes are +2.0844% and +2.6029%. Its 64 full
packet/PCM traces remain identical. These small default differences do not
establish a speedup. Do not multiply this result by prior optimization gains.

A separate preliminary four-clip screen uses three repetitions and 2,000 frames:
CPU medians +47.39655%/+36.77335%, with every one of its 24 CPU/wall pairs slower.
Its worst CPU/wall changes are +92.8983%/+111.7154%. Its odd repetition count
makes pair order unbalanced; it is retained separately and not pooled into the
final 128 rows.

The actual normal linked ARM64 own stack frames are 41,952 bytes for
`Encoder::encode_frame` and 16,576 for the correction, giving 58,528 bytes on
that normal Rust call path. This assembly-derived bound is not a measured
high-water mark or whole-thread bound: caller-owned objects, platform libm/bzero
routines and panic/unwinding paths are excluded. The complete probe text section
grows from 314,364 to 320,252 bytes (+5,888). Gram blocks are inlined; no new
persistent arrays, dependencies or heap processing are introduced. There is at
most one extra analysis/correction/quantization/render/forward-transform trial.

Canonical probes use a Cargo-built release library and manual O3/CGU16/overflow
checks for examples, without LTO, native-CPU or profiling changes. Executable
hashes identify measured files, not expected hashes for every rebuild. One
macOS ARM64 host does not establish phone CPU, battery, callback deadline or
tail-latency performance.

Local validation passes 70 release tests, nine focused debug joint tests,
15 monitor tests, fmt, all-target Clippy and live-audio Clippy. Independent
stored-window trigonometry checks the defining fold, Gram and RHS; a generic
oracle checks every 326 pitches × both durations × every harmonic pair.
Full-correction/rollback and quantized-floor fixtures execute the added path,
including allocation tracking. Math/domain/safety tests do not establish
perceptual quality at unmeasured rates.

Original-only control proof covers 336 traces; default-OFF proof covers 144 and
harmonic-enabled 24/32 kb/s proof covers 96. The integrated build matches all
432 known and 96 new full packet/PCM hashes. These overlapping contract proofs
are not additional independent quality observations.

## Packet loss

The decoder and concealment algorithm are unchanged. One frozen run uses the
eight already exposed **clean** development clips, both durations and two
periodic masks: isolated 5% (one frame per 20) and paired 5% (two per 40). All
32 cases receive their drain. Each lossy stream is compared with its codec's
**own** uninterrupted stream; baseline and candidate uninterrupted PCM differ.

All 96 structural records pass. Across 64 codec/mask streams, 22,733,520 received
samples after the 120-sample recovery prefix match their own uninterrupted
reference exactly. There are 1,842 ordinary dropped frames, 1,175,280 lost
samples and 166,080 allowed recovery-prefix samples. Outputs remain finite in
[-1,1]. No loss mask or selected clip is replaced. The 340 retained private
artifacts were hash-verified; this public bundle records their hashes without
redistributing audio.

The following means are descriptive, not new superiority gates. Available N is
eight per cohort and 32 pooled, with **no missing SNR or other fields** in this
run. Blank fields are preserved if a metric is unavailable and are never zero.
Common-input SNR and intelligibility compare the same clean input. Lost and
recovery SNR columns use each codec's different **own** uninterrupted reference,
so their differences are not a common-reference quality ranking. Loss raw
endpoints retain their original higher precision; derived tables first round
them to canonical six decimals and then use Decimal subtraction.

| Frame ms | Mask | N | STOI change | ESTOI change | Spectral distance change dB | Common-input SNR change dB | Lost own-reference SNR change dB | Recovery own-reference SNR change dB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | isolated5 | 8 | +0.004920 | +0.007799 | -0.157147 | +0.118927 | -0.031544 | -0.189465 |
| 10 | paired5 | 8 | +0.004249 | +0.007161 | -0.155751 | +0.116757 | +0.096256 | -0.185929 |
| 20 | isolated5 | 8 | +0.007164 | +0.010015 | -0.207655 | +0.114570 | +0.006813 | -0.110353 |
| 20 | paired5 | 8 | +0.007067 | +0.009402 | -0.209072 | +0.101038 | +0.007806 | -0.057279 |
| all | all | 32 | +0.005850 | +0.008594 | -0.182406 | +0.112823 | +0.019833 | -0.135756 |

Lossy common-input comparisons retain two STOI and one ESTOI regression; all
are clip 6930-75918-0007 at 10 ms. Spectral distance and common-input SNR improve
in all 32 cases. Raw own-reference SNR counts retain 15 lost, 15 onset, 19 recovery and 17
whole-stream regressions; these counts use the preserved original
higher-precision fields, including near-zero differences. They are descriptive,
not extra gates or a common-reference ranking. Every exact regression remains
in the loss summary.
Relative to each codec's own no-loss quality, the candidate's additional loss
degradation is slightly worse on average: STOI −0.000303, ESTOI −0.000350,
spectral distance +0.008181 dB and common-input SNR −0.099372 dB versus the
baseline's loss degradation. This supports bounded recovery on these masks and
retains average encoder benefit on most lossy cases; it does **not** demonstrate
improved concealment. Periodic known clean masks do not establish arbitrary
network, noisy/unseen-loss, conversational or mobile behavior.

## Evidence and reproduction

The unique `2026-10-11-harmonic-joint-one-` bundle preserves privacy-clean raw
CSV bytes verbatim. Path-bearing JSON and input metadata use explicitly declared
deterministic projections: original private SHA, public SHA and mapping schema
are retained. No score, statistical column, source/executable identity, argument,
regression or packet-budget field is removed. The shared new-selection protocol
also freezes another research candidate; its 192-row shared plan is retained,
while this document publishes only the joint-fit candidate's 96 results.

- [Known quality](measurements/2026-10-11-harmonic-joint-one-quality-known.csv), [all regression rows](measurements/2026-10-11-harmonic-joint-one-regressions-known.csv) and [packet/pitch telemetry](measurements/2026-10-11-harmonic-joint-one-telemetry-known.csv).
- [New quality](measurements/2026-10-11-harmonic-joint-one-quality-new.csv), [all regressions](measurements/2026-10-11-harmonic-joint-one-regressions-new.csv), [frozen gates](measurements/2026-10-11-harmonic-joint-one-gates-new.csv) and [subset/metadata/voice descriptives](measurements/2026-10-11-harmonic-joint-one-descriptives-new.csv).
- [Known protocol](measurements/2026-10-11-harmonic-joint-one-known-protocol.json), [new protocol](measurements/2026-10-11-harmonic-joint-one-new-protocol.json), [new archive selection](measurements/2026-10-11-harmonic-joint-one-new-selection.json) and [selected input metadata](measurements/2026-10-11-harmonic-joint-one-new-input.csv).
- [Integrated known traces](measurements/2026-10-11-harmonic-joint-one-integration-known.csv), [integrated new traces](measurements/2026-10-11-harmonic-joint-one-integration-new.csv), [default proof](measurements/2026-10-11-harmonic-joint-one-default-traces.csv) and [24/32 kb/s proof](measurements/2026-10-11-harmonic-joint-one-highrate-traces.csv).
- [Final harmonic CPU](measurements/2026-10-11-harmonic-joint-one-cpu-harmonic.csv), [default CPU](measurements/2026-10-11-harmonic-joint-one-cpu-default.csv), [separate rough screen](measurements/2026-10-11-harmonic-joint-one-cpu-rough.csv), [fixed plan](measurements/2026-10-11-harmonic-joint-one-cpu-plan.json) and [terminal evidence](measurements/2026-10-11-harmonic-joint-one-cpu-evidence.json).
- [All loss rows](measurements/2026-10-11-harmonic-joint-one-loss.csv), [loss protocol/mask indices](measurements/2026-10-11-harmonic-joint-one-loss-protocol.json), [loss structure](measurements/2026-10-11-harmonic-joint-one-loss-structure.json), [all means/regressions/availability](measurements/2026-10-11-harmonic-joint-one-loss-summary.json) and [derived endpoint means](measurements/2026-10-11-harmonic-joint-one-loss-means.csv).
- [Source roles/content](measurements/2026-10-11-harmonic-joint-one-source.csv), [measured executable roles](measurements/2026-10-11-harmonic-joint-one-binaries.csv), [resource audit](measurements/2026-10-11-harmonic-joint-one-resource-audit.json), [declared projections](measurements/2026-10-11-harmonic-joint-one-projections.csv) and [complete manifest](measurements/2026-10-11-harmonic-joint-one-artifacts.sha256).

The integrated source content matches frozen research content even though
historical research commit IDs are provenance identifiers, not guarantees of
remote Git reachability. Source/executable roles distinguish main baseline,
frozen research and integration; full trace identity connects their different
binaries. Existing historical measurements and source manifests remain intact.
Only the two older manifests' live README bindings and their nested-manifest
binding are updated for this README link; no historical measurement is rerun.

To verify known frozen traces with the original selected FLACs and a rebuilt
canonical probe, without recomputing quality metrics:

```sh
python3 tools/check_frozen_traces.py corpus \
  docs/measurements/2026-10-11-harmonic-joint-one-integration-known.csv results.csv \
  --current ./current-probe --harmonic
```

Canonical `tools/evaluate_quality.py` can reproduce the metric functions and
paired seeds; its standard matrix additionally includes unchanged 24/32 kb/s.
Only its 12 kb/s rows correspond to these quality tables. Use both harmonic
flags and the pinned baseline/source roles. The loss masks are recorded exactly;
roundtrip uses their explicit frame indices, not a fresh random drop seed.

Neither the proxy tables nor this evidence establish human listening acceptance
or a competitive maximum. The remaining work is to improve speech quality while
reducing the optional search cost and verifying realistic microphone/phone use.
