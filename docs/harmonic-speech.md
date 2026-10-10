# Harmonic speech experiment

At 12 kb/s, the native codec can spend much of its packet on locating separate
spectral coefficients. The opt-in encoder also considers a voiced model: explicitly
encode a fundamental frequency and signed cosine/sine amplitudes for its
harmonics. The decoder renders these amplitudes as a dense waveform, then uses
the existing transform and overlap path. Ordinary scalar and pulse modes remain
available for unvoiced speech, noise and transients.

This is an original implementation from mathematical definitions. No external
codec implementation was copied or translated. There are no new dependencies,
model weights, processing allocations, prediction history or transform delay.
The public codec remains independent of transport and audio devices.

Enable it with `Config::with_harmonic_speech(true)` or the monitor's
`--harmonic` flag. The normal configuration keeps this more expensive search
disabled. The decoder accepts every defined profile without another setting.

```sh
cargo run --locked --release --features live-audio --example monitor -- --bitrate 12000 --harmonic
```

## Model and packet budget

Experimental profiles 14 and 15 specify 20 and 10 ms respectively, within the
version 7 header. Earlier version 7 decoders reject these profiles. Applications
must negotiate support for the extended profile set; version 7 alone does not
establish that both endpoints understand harmonic packets. Profiles 6 and 7
remain invalid.

The first field is nine raw bits, `f0 - 75`, for an integer fundamental frequency
from 75 through 400 Hz. Raw values above 325 are invalid. Four gains follow on
the existing gain grid. Their precision is `G = clamp(floor(240P/N), 5, 8)`, where
P is payload bytes and N is the transform size, not a count of harmonic
parameters. For each harmonic h with `h*f0 <= 8000`, encode its cosine amplitude
then sine amplitude. Group harmonics by frequency into 0–1, 1–2, 2–4 and 4–8 kHz;
the uppermost group includes exactly 8 kHz.

For each group, pad its parameter count up to the first supported dimension in
4, 8, 16, 20, 40, 80 and 160. The total padded dimension is at most 320.
Parameters are stored in ascending harmonic order, cosine then sine; padding
comes last. A nonzero decoded padding entry is invalid. Gains and the deterministic
pulse allocator select pulse counts without further side information.

Each group's pulse vector q has L1 norm K. Decode amplitudes as
`gain * q / sqrt(sum(q_i^2))`; K=0 reconstructs zero. Dimensions 20 and above
use the existing signed-class representation where its generated bound selects
it. Dimensions 4, 8 and 16 use unsigned magnitude compositions and signs,
with conservative cost
`ceil(log2(binomial(D+K-1,K))) + min(D,K)` for K>0, and zero for K=0.
The canonical generator emits and checks these three additional tables.

Allocation subtracts nine pitch bits, four G-bit gains and four joint entropy
rounding/termination bits from `8P`. Every allocated band's conservative pulse
cost also fits the remaining budget. The two header bytes and two checksum
bytes remain outside P. At 12 kb/s, complete packets still occupy 15 bytes per
10 ms or 30 bytes per 20 ms. The model does not receive free pitch, phase or
envelope information.

Let `p=(N-120)/2` and t=0 through N+119. Construct
`wave[p+t] = sum_h(a_h*cos(2*pi*h*f0*t/48000) + b_h*sin(2*pi*h*f0*t/48000))`;
all other entries are zero. Apply the defining forward MDCT, then the ordinary
inverse transform and 120-sample overlap. The oscillator uses double-precision
rotations, initially cosine one and sine zero, accumulating waveform samples
as f32 in ascending harmonic order. Four independent oscillator lanes preserve
that state update order and single-precision summation order. Phase is determined entirely by the paid
cosine/sine amplitudes. No previous pitch or excitation is required to decode
the current frame. Gain bounds imply waveform magnitude below 39.375 before
the transform, apart from tiny floating-point error; final PCM is clipped to
the existing [-1,1] range.

The encoder considers this model only at target rates up to 16 kb/s. It computes
normalized lag correlation from the current input and 120 available history
samples, requires correlation at least 0.55, and retains the eight greatest
scores, breaking ties toward lower f0. Lag is the nearest integer to 48000/f0;
adjacent frequencies with the same integer lag reuse the exact same score. All
326 frequency entries and their tie order are retained. At least one third of
this analysis window must remain after the lag. This
excludes fundamentals below 120 Hz from the 10 ms encoder's search, although
the decoder accepts all defined frequencies. Longer causal pitch analysis is
a possible later experiment, not part of these frozen results.

For each retained f0, fit each harmonic's two amplitudes by a separate 2x2 least
squares solve against the available input. Fit gains and quantize the amplitude
vectors using the existing pulse quantizer. Render the quantized candidate
through the actual forward transform and choose it only when full-spectrum
squared coefficient error improves on the ordinary candidates. Separate fits
are approximate: harmonics are correlated over short windows, and this is not
a joint or orthogonal projection.

For a harmonic with angle `w=2*pi*h*f0/48000` and L analysis samples, the
implementation derives its dot products from the second-order recurrence
`s_t=x_t+2*cos(w)*s_(t-1)-s_(t-2)`, initially zero. After L samples, set
`X=exp(-i*(L-1)*w)*(s_(L-1)-exp(-i*w)*s_(L-2))`. The cosine and sine dot
products are `Re(X)` and `-Im(X)`. Set
`Z=sin(L*w)/sin(w)*exp(i*(L-1)*w)`; the Gram entries are
`cc=(L+Re(Z))/2`, `ss=(L-Re(Z))/2` and `cs=Im(Z)/2`. Solve this same 2x2
system for the two amplitudes. Supported w lies in (0,pi/3], so the denominator
is nonzero. Eight independent harmonics run together without changing each
recurrence or its final arithmetic order. Direct double-precision trigonometric
fixtures check the defining fit for every supported pitch and duration.

## Frozen speech results

The research algorithm was frozen at `53d0aaf` before broader evaluation. The
subsequent recurrence and grouped arithmetic changes preserve every full packet
and PCM trace in all 1,296 known/new cases. A separate scalar-order synthesis fixture
preserves float bits for all 326 pitches, both durations and five
sparse/dense/zero/subnormal patterns. The
baseline is the version 7 codec from main `fb78710`. The frozen native drivers
receive identical PCM, packet/drain budgets, sample counts and 120-sample delay.
Eight development clips first established whether to continue. Broader known
data then covers 64 clips from 32 speakers, three input conditions, both frame
durations and 12/24/32 kb/s: 1,152 paired cases.

The new selection contains eight clips from four speakers absent from all 32
known voices: 4970, 4992, 61 and 8555. Its deterministic selection policy was
frozen before fetching or measuring: use the first two FLAC members of the
first four remaining speakers in official archive order. No algorithm changes
follow the known or new scores. The 144 new cases and all individual regressions
must accompany any release using these results.

Audio is [LibriSpeech test-clean](https://www.openslr.org/12), CC BY 4.0.
Attribution: Vassil Panayotov, Daniel Povey, Guoguo Chen and Sanjeev Khudanpur,
*LibriSpeech: an ASR corpus based on public domain audio books*, ICASSP 2015.
No audio or transcripts are redistributed. The existing seeded noise and paired
metric-dither method is unchanged. STOI/ESTOI compare against clean speech; SNR
and spectral distance compare against the actual encoder input. Higher SNR,
STOI and ESTOI and lower spectral distance are favorable.

| Selection | Input | Frame ms | SNR change dB | STOI change | ESTOI change | Spectral distance change dB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Known 64 | clean | 10 | +0.448395 | +0.009707 | +0.013416 | -0.252109 |
| Known 64 | noise20 | 10 | +0.403886 | +0.010694 | +0.013027 | -0.256408 |
| Known 64 | noise10 | 10 | +0.227409 | +0.012960 | +0.012093 | -0.356973 |
| Known 64 | clean | 20 | +0.196110 | +0.004441 | +0.007532 | -0.178187 |
| Known 64 | noise20 | 20 | +0.163924 | +0.005075 | +0.007961 | -0.155935 |
| Known 64 | noise10 | 20 | +0.085954 | +0.007081 | +0.008367 | -0.347323 |
| New 8 | clean | 10 | +0.985581 | +0.017116 | +0.027123 | -0.318016 |
| New 8 | noise20 | 10 | +0.882718 | +0.018505 | +0.026660 | -0.391706 |
| New 8 | noise10 | 10 | +0.472092 | +0.026808 | +0.026562 | -0.599394 |
| New 8 | clean | 20 | +0.333892 | +0.006411 | +0.011376 | -0.214962 |
| New 8 | noise20 | 20 | +0.277928 | +0.006444 | +0.010963 | -0.182346 |
| New 8 | noise10 | 20 | +0.132348 | +0.008336 | +0.009934 | -0.498148 |

The table covers 12 kb/s. At 24/32 kb/s, all 864 known/new PCM hashes remain
identical to the baseline. All 1,296 paired packet/drain budgets and delays
match. Across known data, 48 STOI, 57 ESTOI and eight spectral-distance cases
regress; worst changes are -0.013431, -0.010569 and +0.152250 dB. No SNR case
regresses. New data has five STOI and eight ESTOI regressions, with worst changes
-0.003905 and -0.003691; no SNR or spectral-distance case regresses.

The 48 clean clips with matched existing external references yield native mean
STOI 0.861414 at 12 kb/s/10 ms versus 0.949883 for the external low-delay
reference, a remaining deficit of 0.088469. At 20 ms the values are 0.921898 and
0.960611, a remaining deficit of 0.038713. These reuse previously measured
reference rows after checking input hashes, sample counts, clipping, byte budgets
and delay; no new external encoder run is claimed. The gap shrinks modestly but
remains substantial. Read English and synthetic noise do not establish real
microphone, multilingual, music or subjective listening acceptance.

## Validation and remaining work

Independent fixtures reconstruct sinusoids and both transform equations directly,
without production harmonic helpers. Separate packet builders compute their own
pulse costs, class priors, allocation and subset coding. Malformed pitch, gains
and nonzero padding preserve output, nonzero overlap, the next decode and
concealment. The counting allocator explicitly reaches harmonic packets and
observes zero processing allocations. All 60 release tests and 15 monitor tests
pass locally; final CI also checks the supported desktop and mobile targets.

The 144 seeded loss cases use the frozen candidate as its own uninterrupted
reference: eight development clips, three input conditions, both durations and
independent 5%/40 ms/80 ms burst loss at 12 kb/s. Repeat outputs match; all
packet/drain budgets match; received samples after their 120-sample recovery
prefix exactly match uninterrupted decoding. This proves bounded recovery,
not improved concealment quality relative to other codecs.

The final integrated encoder and decoder reproduce every frozen packet and PCM
hash in all 1,296 cases, including the adjacent-lag score reuse. The compiled
ARM64 encoder has a 41,952-byte own stack frame and the decoder 25,360 bytes.
Conservative compiled internal call-path bounds are 43,584 and 26,576 bytes,
respectively. These are assembly-derived bounds, not measured high-water marks;
they exclude caller-owned objects, OS/library routines and panic unwinding.
Both fixed frames also exist when harmonic search is disabled.
At most 106 harmonics and eight fully fitted candidates are evaluated per frame.
The decoder adds sinusoidal rendering and one forward transform before the
ordinary inverse. Paired process CPU must account for the probe's 256-frame
packet preparation; a decode count slope can separate fixed preparation from
additional decoding. The [eight-lane analysis audit](harmonic-analysis.md) measures
the current analysis implementation. The historical [resource audit](harmonic-resources.md) reports separate encoder costs,
decoder count slopes and disabled-mode comparisons. These desktop measurements
do not establish mobile callback deadlines or battery use.


## Reproducible evidence

Individual quality rows include regressions and input hashes:
[known 64](measurements/2026-10-11-harmonic-speech-quality-known.csv),
[new eight](measurements/2026-10-11-harmonic-speech-quality-unseen.csv).
Source FLAC hashes are recorded for the
[known selection](measurements/2026-10-11-harmonic-speech-known-input.sha256) and
[new selection](measurements/2026-10-11-harmonic-speech-unseen-input.sha256).
The new selection is now evaluated data; future tuning needs another holdout.

[Seeded loss rows](measurements/2026-10-11-harmonic-speech-loss.csv) describe
the frozen research driver.
[Frozen full trace hashes](measurements/2026-10-11-harmonic-speech-frozen-traces.csv)
and [final integration hashes](measurements/2026-10-11-harmonic-speech-integration-traces.csv)
connect that quality evaluation to the integrated encoder and decoder.
[Compiled section sizes](measurements/2026-10-11-harmonic-speech-sections.csv)
include the complete probe, rather than just codec functions.

To check the final probe against every frozen trace without repeating the
quality metrics, use the original selected FLACs in `corpus`:

```sh
python3 tools/check_frozen_traces.py corpus \
  docs/measurements/2026-10-11-harmonic-speech-frozen-traces.csv results.csv \
  --current ./current-probe --harmonic
```

The verifier checks input hashes, frame quotas, aligned PCM and complete packets.
It replaces `results.csv` only after every row matches. It discards audio
diagnostics unless explicitly requested with `--diagnostics`.

[Source provenance](measurements/2026-10-11-harmonic-speech-source.csv) records
the quality baseline, frozen research implementation, preceding main and
integrated sources separately. Historical measurement drivers use their original
CLI: the research driver enables this model without a flag, while the integrated
driver requires `--harmonic`. Final trace identity connects the changed drivers.

[Driver hashes](measurements/2026-10-11-harmonic-speech-binaries.csv) identify
the measured executables; they are not expected hashes for all rebuilds.
The integrated rows identify this change's source contents, while historical
rows identify the snapshots used for the frozen comparisons.
The [complete evidence manifest](measurements/2026-10-11-harmonic-speech-artifacts.sha256)
includes quality, integration, loss and resource artifacts.
