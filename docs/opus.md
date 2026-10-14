# Owned Opus support

Enable `opus` for standard Opus packets and Ogg Opus files. The feature adds no
dependencies. Snacka owns the safe Rust encoder and decoder, sharing its native
entropy and FFT/MDCT kernels. Production builds require no external codec,
C, FFI, CMake or build script.

## Codec and packet APIs

`Encoder` writes SILK, Hybrid and CELT from normalized interleaved f32 PCM at
48 kHz. `EncoderConfig` selects channels, bitrate, bandwidth, duration,
application, rate policy, in-band FEC and DTX. Voice mode chooses SILK or Hybrid;
audio and low-delay preferences choose CELT. An explicit mode is available.
Invalid mode/bandwidth/duration combinations are rejected during configuration.

The encoder supports one or two channels and 2.5/5/10/20/40/60 ms packet
choices, with SILK/Hybrid starting at 10 ms. It analyzes LPC/NLSF and voiced
pitch/LTP prediction, quantizes excitation and codes the standard entropy
models. Hybrid combines its SILK lowband and CELT highband in the same range
coder. Constant rate pads the complete packet; variable rate compacts the
payload within the configured budget. Fractional byte budgets carry across
packets.

`reconfigure` applies validated changes immediately. `prepare_reconfigure`
queues a change after the next packet, permitting end transition redundancy
before an LP-to-CELT switch. A CELT-to-LP switch can carry beginning redundancy.
`has_transition_redundancy` reports what was actually emitted. Immediate
changes and small budgets can omit transition redundancy; decoding still
handles the mode change.

`with_fec(true)` enables previous-audio SILK redundancy in SILK/Hybrid packets.
The encoder keeps a coarse independently quantized shadow of eligible speech;
`has_fec` reports actual emission. Changes in frame geometry and timeline gaps
clear incompatible cached redundancy. CELT has no SILK LBRR and rejects that
configuration.

`with_dtx(true)` enables conservative quiet admission, a 200 ms hangover and
coded inactive refreshes approximately every 400 ms. It emits valid empty
frames with the configured duration between refreshes. `was_dtx` permits an
application to omit their transmission while advancing its sample clock. This
selector conservatively preserves sounds above its quiet floor; it does not
claim the reference codec's voice-activity classification.

`Decoder` reads every standard single-stream SILK/Hybrid/CELT TOC geometry into
one or two fixed output channels. `with_sample_rate` selects 8/12/16/24/48 kHz.
Coded channel and mode changes retain the appropriate history. Mono output
normally disables intensity phase inversion, matching common mobile decoder
behavior; `set_phase_inversion` explicitly controls that preference.

```rust
use snacka::opus::{Decoder, Encoder, EncoderConfig};

let config = EncoderConfig::default().with_fec(true).unwrap();
let mut encoder = Encoder::new(config).unwrap();
let mut decoder = Decoder::new(config.channels()).unwrap();
let mut packet = [0; 1276];
let mut pcm = [0.0; 960];
let length = encoder.encode(&[0.0; 960], &mut packet).unwrap();
assert_eq!(decoder.decode(&packet[..length], &mut pcm).unwrap(), 960);
```

Prepare codecs before an audio callback. Construction allocates the fixed
workspaces; processing, recovery, reset and configuration changes do not
allocate. The small-stack regression includes duplex codecs, all output rates
and mapped streams on a 1 MiB stack. `CeltEncoder`/`CeltDecoder` retain narrower
CELT-only APIs.

Output floats remain finite but may exceed ±1 through ringing or gain. Limit
or clip when converting to integer PCM. The encoder's own group delay is 120
samples at 48 kHz, exposed by `algorithmic_delay_samples`; raw users handle
start delay and end padding. This is separate from packet buffering and
transport jitter.

`Packet::parse` inspects borrowed ordinary framing without allocating.
Self-delimited parsing and `Repacketizer` support CBR/VBR, padding, arbitrary
valid frame groupings and the full 120 ms packet limit. Parallel encoders and
self-delimited packet composition can produce multistream transport without
requiring a C-style multistream encoder API. `ChannelMapping`,
`MultistreamDecoder` and `ProjectionDecoder` decode mapped channels and signed
Q15 Ambisonics demixing matrices, including channel aliases and silence.

A malformed compressed frame can advance some history. Reset after a corrupt
content error. Packet framing, configuration and output-capacity errors are
checked before history changes. The APIs are native Rust contracts; they do
not mirror the foreign codec's controls or instrumentation.

## Loss recovery and calls

`conceal(missing_samples, output)` predicts missing audio from existing LPC/LTP
or CELT pitch/spectral history. Requested duration is per output channel and
must use the standard 2.5 ms clock, up to 120 ms. Recovery advances the codec
clock and overlap history rather than replaying a cached frame.

For in-band recovery, retain the following packet and call
`recover(next_packet, missing_samples, lost_output)`. Then call `decode` on that
same packet for its current audio. `Recovery::Fec` reports recovered SILK LBRR;
`Recovery::Concealed` reports prediction where redundancy is absent. Shortened
losses can select the appropriate tail of coded redundancy. `final_range`
after recovery reports the consumed owned entropy prefix; a foreign decoder's
PLC RNG instrumentation need not be identical. Ordinary packet intervals are
checked independently against the reference.

The [RTP call guide](opus-calls.md) describes 48 kHz timestamps, SDP, bounded
jitter buffering and a std-only UDP peer example. Its optional reference test
exercises both directions against public libopus, including reordered packets,
duplicates, sequence/timestamp wraps, actual FEC and quiet-to-voice DTX. Phone
audio routing, Swift/Kotlin bindings and WebRTC session setup belong to the
application integration; physical mobiles have not been exercised here.

## Ogg Opus

`ogg` implements CRCs, continuation pages, sequence/granule validation, tags,
header gain, pre-skip and final trimming. `OpusIdentification` carries mapping
families 0/1/2/3/255. `OggDecoder` chooses mapped or projection decoding.
`OggReader` handles one logical stream; `OggChainReader` reports sequential
streams, and `OggDemuxReader` routes interleaved streams independently, including
opaque packets from other codecs. `OggWriter` writes mapped/projection headers
and continued audio packets.

Default audio limits cover the largest legal 255-stream packet. `OggLimits`
allows explicit application bounds, separate from metadata limits. Container
operations allocate and belong outside an audio callback.

```sh
cargo run --release --features opus --example opus_roundtrip -- \
  input.f32 output.opus decoded.f32 32000 1 20
cargo run --release --features opus --example opus_roundtrip -- \
  decode existing.opus output.f32
```

The file tool uses interleaved little-endian f32 PCM at 48 kHz, applies gain,
and handles start delay, end trimming and mapped channel output.

## Verification and performance target

The objective is full standard wire support and CPU time no greater than
libopus across encoding, decoding and loss recovery. The APIs need not map
one-to-one. Performance parity remains an active target; the current
implementation has measured slower cases and does not claim completion.

Normal tests use synthetic checked-in oracle observations and independent
mathematical checks. Development-only tools load a separately installed
libopus; neither foreign codec source nor recorded speech is vendored.

```sh
cargo test --locked --features opus
cargo test --locked --release --features opus
cargo clippy --locked --all-targets --features opus -- -D warnings
cargo bench --locked --bench opus --features opus
RUSTFLAGS="-L /path/to/lib" cargo test --locked --release \
  --features opus,reference-tests --test opus_reference --test opus_calls_reference
RUSTFLAGS="-L /path/to/lib" cargo bench --locked --bench opus \
  --features opus,reference-tests
```

Matched-packet decoder benchmarks feed the same packet sequence to both codecs,
alternate their order and report paired batch-throughput p50/p95/p99. Generic
encoder benchmarks check actual wire mode and byte budgets on identical PCM.
Their CPU results do not establish equivalent encoder or concealment quality.
The broader decoder matrix covers 1,080 mode/bandwidth/duration/channel/rate
and decode/PLC/FEC-return cases. Its functional validation passed; final timing
must bind to the measured source snapshot.

A serialized, frozen-source encoder run on Apple M5/macOS 27 against libopus
1.6.1 on 2026-10-14 covered 88 matched mode/bandwidth/duration/channel cases.
All 88 paired p50 CPU ratios were below 1.0. Median ratios by mode were 0.405
for SILK, 0.824 for Hybrid and 0.470 for CELT. Nine cases still exceeded the
reference at paired p95 or p99. These measurements use changing generated
voiced PCM at equal packet byte budgets; they do not establish equal encoder
quality or completion of the performance target.

The retained 24-case CELT matched-packet run on Apple M5/macOS 27 against
libopus 1.6.1 measured a median paired p50 ratio of 1.009, with cases from
0.892 to 1.337; 12 cases were at or below the reference. The earlier
same-packet baseline median was 1.619. This snapshot precedes the separate
native-rate SILK delay bypass; that change leaves CELT arithmetic untouched.
Exploratory runs can overlap host work; final parity acceptance requires a
serialized frozen-source repetition and every case to pass, including tails.

A subsequent SILK synthesis change uses f32 histories and four independent
prediction accumulators. In two alternating before/after runs with reversed
roles, all 270 tested rate/channel/decode/PLC/FEC-return cases improved at
paired p50; median CPU ratios were 0.922 and 0.923 against the preceding
Snacka implementation. These are incremental gains, not libopus parity.
The stored synthesis histories shrink by 1,288 bytes per coded channel.
Across all 40,150 official vector packets, final ranges remained exact and
the published quality scores were unchanged. Maximum additional sample error
against the f64 version was 3.82e-6 and maximum per-stream RMS error 1.57e-7.

Opus FFT/MDCT plans copy precomputed coefficients from Snacka's own dynamic
mathematics. There is no lazy global initialization or runtime trigonometry for
these standard sizes. `python3 tools/generate_transform_plans.py --check`
reproduces the constants on the generating toolchain/platform; independent
transform tests check the defining equations and all other sizes retain their
dynamic constructors. Ten alternating, warm-allocator `Decoder::new` plus drop
cases (both output channel counts and all five output rates) measured 2.4–2.8x
faster at p50 on the same M5. This measures initialization against the preceding
Snacka implementation, separately from packet processing and libopus parity.

A frozen-source follow-up at `c7e9ca6` measured 90 selected complete-decoder
cases against libopus 1.6.1, using 1,000 alternating batches and the same changing
packets. Each selected geometry covers both channel counts, all five output
rates, ordinary decode, PLC return and FEC return. Functional checks passed;
this is partial performance coverage, not the complete 1,080-case matrix:

| Selected wire geometry | Cases | Median p50 ratio | Largest p50 ratio |
| --- | ---: | ---: | ---: |
| SILK NB, 20 ms | 30 | 1.091 | 1.226 |
| Hybrid FB, 20 ms | 30 | 1.024 | 1.310 |
| CELT FB, 2.5 ms | 30 | 1.070 | 1.180 |

Ratios above one are slower than libopus. p95/p99 also contain slower cases;
the performance target remains unmet. These are continuous-stream batch averages
on generated voiced input, excluding construction, reset, validation and I/O.

### Decoder optimization on x86-64, 2026-10-14

The current change bounds SILK rewhitening to the history the five-tap predictor
actually reads, transposes full-rate interpolation coefficients without changing
sum order, and reuses planned FFT butterfly widths. CELT allocation evaluates
active bands directly and stops over-budget searches. CELT loss prediction scores
four coarse pitch lags together, keeps forecast-local LPC history contiguous,
and bypasses envelope calculations when the entire forecast precedes the fade.
Processing remains safe Rust and allocation-free.

Compare against `e68ef30` using the same current timing-oracle source and release
flags. Host: AMD EPYC 9V74 x86-64, Linux cloud VM, CPU affinity 0; Rust 1.99.0 and
separately built libopus 1.6.1 with GCC 14.2/CMake Release and runtime SIMD enabled.
Rust uses overflow checks, 16 codegen units, no LTO or native-CPU flags. Frequency,
power and other VM workloads are uncontrolled. The
[source and artifact manifest](measurements/2026-10-14-x86_64-opus-source.json)
binds both libraries, common oracle, reference, generated inputs and raw results.

The [selected paired table](measurements/2026-10-14-x86_64-opus-selected.csv)
contains nine complete-decoder cases, three alternating-order before/after
repetitions and 1,000 alternating Snacka/libopus batches per executable per case.
Every batch lasts at least 400 microseconds. Percentiles describe batch averages,
not individual callback deadlines. The change column is the median of each
repetition's current/baseline p50 ratios normalized against its interleaved
libopus measurements; independently aggregated ratio columns need not divide to
that change.

| Wire geometry, output and operation | Baseline p50/libopus | Current p50/libopus | Median p50 time change |
| --- | ---: | ---: | ---: |
| SILK NB 20 ms, mono, 16 kHz, decode | 1.338 | 1.307 | -1.22% |
| SILK NB 20 ms, mono, 48 kHz, decode | 0.894 | 0.822 | -8.15% |
| SILK WB 20 ms, mono, 8 kHz, decode | 1.615 | 1.534 | -4.40% |
| Hybrid FB 20 ms, mono, 48 kHz, decode | 1.177 | 1.160 | -1.26% |
| CELT FB 2.5 ms, mono, 48 kHz, decode | 1.338 | 1.280 | -4.48% |
| CELT FB 20 ms, stereo, 48 kHz, decode | 1.289 | 1.302 | +0.71% |
| CELT NB 2.5 ms, stereo, 8 kHz, PLC return | 1.950 | 1.699 | -12.83% |
| CELT NB 2.5 ms, stereo, 48 kHz, PLC return | 2.002 | 1.717 | -13.70% |
| CELT FB 20 ms, stereo, 48 kHz, PLC return | 1.380 | 1.365 | -1.11% |

Eight selected medians improve and one increases. Small changes remain sensitive
to host variation. All 1,080 generated mode/bandwidth/duration/channel/output-rate
and decode/PLC-return/FEC-return traces retain their baseline PCM fingerprints;
sample counts, finite PCM and ordinary entropy intervals also pass. Hashing runs
outside the timing loops. This verifies preserved output on these generated
streams; speech listening quality and official-vector scores were not remeasured.
All 282 debug tests and 297 release tests, including public libopus reference
checks, pass. Allocation and small-stack checks, Opus clippy and transform-plan
reproduction pass.

The [complete exploratory decoder matrix](measurements/2026-10-14-x86_64-opus-matrix.csv)
uses 100 batches per case on this frozen source and completes all 1,080 cases
without failures. Median p50/libopus ratios are 1.169 for SILK, 1.139 for Hybrid
and 1.374 for CELT. 1,020 cases exceed libopus at one or more reported percentiles.
This quick run identifies gaps; final acceptance requires 1,000 batches per case.
The separate [88-case encoder run](measurements/2026-10-14-x86_64-opus-encoder.csv)
uses 1,000 batches: median p50 ratios are 0.627 for SILK, 1.109 for Hybrid and
0.587 for CELT. Sixteen encoder cases exceed libopus at p50 and 71 at one or more
reported percentiles. Equal packet budgets do not establish equivalent quality.
**Full libopus parity remains unmet.** These measurements cover this x86-64 host;
ARM64 must be measured separately.

Reproduce the complete decoder run with the driver below. Use `--quick` for
exploration or `--validate-only` to generate the deterministic `.bit` inputs and
check functionality. For before/after comparisons, build each library at its
recorded snapshot, compile both standalone probes from the same current
`tools/reference/benchmark_opus_decode.rs` with `-O -C overflow-checks=yes`, then
run `matrix RATE CHANNELS SCHEDULE 1000 400000 FILE.bit` three times, reversing
probe order on the second repetition and retaining both raw outputs.

```sh
python3 tools/reference/benchmark_opus_matrix.py /path/to/libopus.so \
  --output-dir /tmp/snacka-opus-parity --require-parity
```

`--require-parity` returns failure if any case is missing, fails validation,
changes source during measurement or exceeds libopus at p50, p95 or p99. It
requires the full timed matrix and rejects quick, validation-only and filtered
runs. Remaining work prioritizes CELT decoding/PLC, below-native-rate SILK
filtering, Hybrid encoding and all encoder tails.

### Encoder optimization on x86-64, 2026-10-14

The follow-up caches the owned encoder FIR's exact f32-to-f64 coefficient
conversions at compile time and scales the Q12 pitch predictor once per analysis.
Four coarse pitch candidates share residual reads while retaining each
candidate's scalar sum order and the original tie-breaking order. Filter taps,
delay, packet budgets and stream workspace sizes are unchanged; read-only
coefficient storage grows by 2,124 bytes. Processing remains safe Rust and
allocation-free. These are encoder changes; decoder timings above were not
remeasured.

Compare against the pushed `bca7700` on the same x86-64 host and release flags
described above. All 88 encoder geometries run three times, reversing executable
order on the middle repetition, with 1,000 alternating Snacka/libopus batches
per case per executable. Each batch lasts at least 200 microseconds after
warmup. Input is generated changing voiced PCM; construction, validation and I/O
are excluded. Ratios describe batch-average throughput. Each case's time change
is the median of three libopus-normalized current/baseline p50 comparisons;
the table reports medians across that mode's cases.

| Encoder mode | Cases | Baseline p50/libopus | Current p50/libopus | Median p50 time change |
| --- | ---: | ---: | ---: | ---: |
| SILK | 24 | 0.629 | 0.594 | -6.03% |
| Hybrid | 16 | 1.108 | 1.045 | -5.69% |
| CELT | 48 | 0.584 | 0.586 | +0.44% |

All 40 SILK/Hybrid case medians improve, by 3.92–7.35%. CELT does not use the
changed paths: its case changes range from -1.37% to +2.68%, with 37 of 48
medians increasing. This control variation limits claims about small changes on
the uncontrolled VM. All 16 Hybrid cases remain slower than libopus at p50;
73 of 88 cases exceed it at one or more of the separately aggregated p50/p95/p99
ratios. **Full performance parity remains unmet.** Equal packet budgets alone
do not establish equal audio quality, and the
[quality goal](implementation.md#quality-goal) remains open.

The independent before/after trace probe covers all 88 geometries with voiced,
seeded noise, transient and silent input: 352 streams, 96 packets each. All
33,792 packets retain matching packet and decoded-PCM fingerprints. Hashing
runs outside timed loops. This preserves tested baseline output; it is not new
listening or perceptual-quality evidence. All 283 debug tests and 298 release
tests including public libopus reference checks pass, as do Opus clippy and
transform-plan reproduction. The new coarse-pitch test compares batched scores
bit-for-bit with independent scalar correlations.

The [88-case summary](measurements/2026-10-14-x86_64-opus-encoder-followup/summary.csv)
includes each repetition's minimum/maximum change and aggregated percentiles.
The [source manifest](measurements/2026-10-14-x86_64-opus-encoder-followup/source.json)
binds unchanged source, benchmark binaries, reference library, raw CSVs, trace
evidence and probe; it records build/run commands and verification commands.
The [recorded driver](measurements/2026-10-14-x86_64-opus-encoder-followup/recorded-run.py)
preserves the exact aggregation and host paths. Adapt its checkout, binary,
reference and output paths to repeat the comparison. Compile the same
[trace probe](measurements/2026-10-14-x86_64-opus-encoder-followup/trace-probe.rs)
against each library and run `MODE BANDWIDTH FRAME_MS CHANNELS 96 0 SIGNAL`
for each geometry and signal to reproduce the output checks.

### Speech quality improvement on x86-64, 2026-10-14

The owned SILK encoder could fit a detailed predictor into a packet while leaving
an excessively coarse excitation. It now compares a broader unvoiced predictor
when reconstruction-error power exceeds one quarter of input power, retaining
the fitting candidate with lower error. Gain search also retains the least
distorted fitting reconstruction among the candidates it already visits.

An encoder-only feedback pole of 0.65 colors quantization error toward lower
frequencies. It is enabled only at budgets no greater than 1.5 bits per native
sample, for dark input with adjacent-sample correlation above 0.9 of input energy,
and estimated unit-cell noise above one percent of input energy. Fine,
bright/noisy and silent cases retain unshaped cells. These are engineering
heuristics selected on development data, not guaranteed perceptual or SNR bounds.
Filter models, packet syntax, byte budgets and nominal delay are unchanged. Each
encoder channel adds one f64 of private shaping history; processing still
allocates nothing. SILK and Hybrid encoding change; CELT and decoding algorithms
are unchanged.

The baseline is pushed commit `bf3b8be`. The new
[source manifest](measurements/2026-10-14-opus-quality/source.json) binds sources,
tools, binaries, input hashes, results and verification. The corpus is
[CMU AN4](https://github.com/cmusphinx/an4/tree/b567d132f43304e70a2b2eca6077f22255311812),
copyright 1991–2005 Carnegie Mellon University, under its
[redistribution license](measurements/2026-10-14-opus-quality/LICENSE-AN4.txt).
Before scoring, select the first four sorted speakers in each official train/test
split and the first two sorted `cen` utterances for each speaker. Development and
evaluation speakers are disjoint. The
[input manifest](measurements/2026-10-14-opus-quality/an4-manifest.json) pins all
16 original WAV files and download URLs. Audio and transcripts are not
redistributed in this repository.

Each recording is originally 16 kHz, converted by FFmpeg to mono 48 kHz f32.
Evaluation covers clean input and the existing seeded Gaussian-noise method at
20/10 dB, with no clipping. Eight evaluation clips from four separate speakers
produce 144 SILK WB cases at 10/20 ms and 12/24/32 kb/s, plus 48 Hybrid FB cases
at 32 kb/s. The reference uses libopus 1.6.1, complexity 10, VOIP with a voice
hint, forced coded bandwidth, CBR and DTX disabled. Complete packet bytes and
one drain frame match. The owned encoder reports a nominal 120-sample delay;
the reference VOIP lookahead is 312. Each is separately trimmed to input length.
Thus before/after comparisons match delay, while absolute SILK/Hybrid comparisons
against libopus are exploratory and do not close matched-delay acceptance.
The unchanged CELT control uses restricted-lowdelay and matched 120-sample delay.

The [SILK evaluation table](measurements/2026-10-14-opus-quality/evaluation.csv)
retains every case, hashes and metrics. Clean means below weight each of the
eight clips and both durations equally. Higher STOI/ESTOI and lower speech-band
log-spectral distance are favorable; these are proxies, not listening scores or
percentages of understood words.

| Clean SILK WB kb/s | Baseline STOI | Current STOI | Libopus STOI | Baseline LSD dB | Current LSD dB | Libopus LSD dB |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 12 | 0.49555 | 0.80314 | 0.94449 | 24.56071 | 13.65343 | 6.19723 |
| 24 | 0.66942 | 0.94754 | 0.99206 | 18.65828 | 10.18702 | 4.58538 |
| 32 | 0.92695 | 0.97606 | 0.99626 | 9.87892 | 9.16396 | 3.76454 |

Across all SILK bitrates/durations, mean STOI/ESTOI changes are +0.21161/+0.22420
for clean speech, +0.16636/+0.18722 at noise20 and +0.08465/+0.06693 at noise10.
Mean LSD changes are -6.69783, -3.55309 and -1.59474 dB respectively. Individual
regressions remain: 10 STOI, 11 ESTOI, four LSD and five waveform-SNR cases out of
144. [Paired speaker-bootstrap intervals](measurements/2026-10-14-opus-quality/evaluation-before-after.json)
cover every condition; 15 of 18 conditions support all three no-worse-than-baseline
metric gates. Four speakers and percentile intervals without multiplicity
correction limit inference; uncertain conditions remain open.

The [Hybrid table](measurements/2026-10-14-opus-quality/hybrid.csv) improves clean
mean STOI/ESTOI by +0.28318/+0.30069, with -8.63508 dB LSD. Its noise20/noise10
means also improve; three STOI and five ESTOI cases regress, with none in LSD.
The [CELT control](measurements/2026-10-14-opus-quality/celt.csv) preserves all 144
packet/PCM fingerprints and all four metrics exactly. Two additional clean SILK
loss runs cover every bitrate/duration: an isolated lost packet every 23 frames
after frame 11, and three consecutive lost packets at those positions. All
received-packet entropy intervals and finite-output checks pass. Mean STOI
changes are +0.23660 for [isolated loss](measurements/2026-10-14-opus-quality/isolated.csv)
and +0.22843 for [bursts](measurements/2026-10-14-opus-quality/burst.csv); four and
two individual STOI cases regress. These do not establish FEC or general loss
quality parity.

The [cross-decoder diagnostic](measurements/2026-10-14-opus-quality/decoder-diagnostic.csv)
uses four evaluation utterances at all three bitrates and 20 ms. Swapping the
decoder on identical packets changes mean STOI by less than 0.0001 in every
profile. Reference-encoded packets retain reference-level speech scores with the
owned decoder; using libopus to decode owned packets does not close the gap.
The remaining speech gap in these cases is primarily encoding. Official-vector
and broader decoder-quality acceptance remain separate.

The [paired CPU summary](measurements/2026-10-14-opus-quality/cpu/summary.csv)
uses two evaluation utterances, seven 20 ms mono mode/bandwidth/bitrate profiles,
three alternating-order repetitions and 1,000 alternating Snacka/libopus batches
per executable. Use the same x86-64 host and flags as above, affinity 0 and a
256-frame input cycle. At SILK WB 12 kb/s, normalized p50 processing time falls
24.98% and 37.22% relative to the baseline; current/libopus p50 ratios are 0.509
and 0.597. Other case medians range from -0.44% to +11.61%, with a +0.44% median
across all 14 cases. Every selected p50 is below libopus, but nine cases exceed
it at p95 or p99. This is partial batch-throughput evidence on an uncontrolled
VM, not full CPU or callback-tail acceptance.

All 286 debug and 301 release tests including reference checks pass, along with
allocation/stack checks, clippy, transform-plan reproduction and five offline
quality-driver controls. A new synthetic low-rate formant regression bounds
decoded error power below 40% of input energy: the old model scores -2.63 dB SNR
and the current bright/unshaped case 4.98 dB. An independent wire-excitation
oracle checks shaped-cell choices; coloring guards cover silence, bright input,
impulses and fine quantization.

**Quality parity remains unmet.** All 54 declared held-out reference objective
conditions fail one or more confidence gates. A local blind package provides
16 randomly ordered A/B pairs at 12/24 kb/s, WB, mono and 20 ms, with its key
withheld; human ratings have not been supplied. More languages including Swedish,
music/transients, microphone noise, stereo, FEC and blind listening remain open.
Remaining encoder work should target excitation modeling, rate-aware spectral
prediction and perceptual allocation, using new held-out speakers for subsequent
tuning rounds. The [quality goal](implementation.md#quality-goal) stays active.

Build `tools/reference/opus_quality_probe.rs` against each release library with
the command recorded in the manifest. The
[corpus fetcher](measurements/2026-10-14-opus-quality/fetch-corpus.py) downloads
the pinned inputs and verifies SHA-256, including the required license and
manifest. Install offline NumPy/SciPy/pystoi
versions from each run's source manifest outside the Rust library, then run:

```sh
OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 python3 tools/reference/evaluate_opus_quality.py \
  /tmp/an4-corpus /tmp/snacka-quality-evaluation \
  --probe /tmp/current-probe --baseline-probe /tmp/baseline-probe \
  --baseline-commit bf3b8be38dd7c691da3d8bbf02b58bb3e9cc4b9f \
  --reference-library /path/to/libopus.so --split evaluation --mode silk
```

Use `--mode hybrid --bandwidth FB --bitrates 32000` for Hybrid, `--mode celt` for
the control, and `--conditions clean --loss isolated` or `--loss burst` for loss
runs. `--require-parity` is limited to the CELT evaluation split in this driver;
it fails if any objective condition is missing enough speakers or its confidence
gate fails. It does not establish listening, other modes or full CPU acceptance.
Recorded CPU/diagnostic/blind-kit scripts retain original host paths; adapt those
paths to reproduce. The benchmark now accepts a targeted encoder profile:

```sh
LD_LIBRARY_PATH=/path/to/libopus/build taskset -c 0 BENCH_BINARY \
  --encoder-case silk WB 20 1 12000 --input speech.f32 --frames 256
```

### Broad-predictor pitch follow-up, 2026-10-14

The follow-up compares voiced and unvoiced versions of the broader predictor
at low budgets. A separate pitch search permits a 0.15 coarse-correlation score,
while ordinary analysis retains 0.42. The voiced candidate is used only if its
complete stage fits and has lower reconstruction error. The existing strict
tap-sum contraction bound remains active. Syntax, nominal delay and storage are
unchanged; this deliberately spends more encoder work to improve low-rate speech.

The baseline is `fdc18d0`. The
[follow-up source manifest](measurements/2026-10-14-opus-quality-pitch/source.json)
binds the measurements and verification. Previously observed 16 clips from eight
speakers become [development regressions](measurements/2026-10-14-opus-quality-pitch/development.csv),
covering 288 conditions. Select four new, previously unused official AN4 test
speakers in sorted order: mdms2, menk, miry and mjgm, with the first two `cen`
utterances each. The algorithm is frozen before inspecting their results and is
not tuned afterward. The [input manifest](measurements/2026-10-14-opus-quality-pitch/an4-manifest.json)
and [fetcher](measurements/2026-10-14-opus-quality-pitch/fetch-corpus.py) preserve
selection, hashes and licensing. The scoring, reference controls and delay
limitations above apply unchanged.

Across all eight new clips and both 10/20 ms durations, clean WB means are:

| kb/s | Baseline STOI | Current STOI | Libopus STOI | Baseline ESTOI | Current ESTOI | Libopus ESTOI |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 12 | 0.84816 | 0.88064 | 0.94963 | 0.63091 | 0.71445 | 0.87880 |
| 24 | 0.96026 | 0.96268 | 0.99284 | 0.89442 | 0.89868 | 0.97832 |
| 32 | 0.98131 | 0.98131 | 0.99641 | 0.95129 | 0.95129 | 0.98891 |

At clean 12 kb/s, LSD falls from 13.77657 to 12.55132 dB; the reference remains
6.20927 dB. The [144-case evaluation](measurements/2026-10-14-opus-quality-pitch/evaluation.csv)
has mean STOI changes +0.01163/+0.00841/+0.00452 for clean/noise20/noise10 when
pooling all bitrates and durations. Twelve STOI, fourteen ESTOI, two LSD and four
waveform-SNR cases regress individually. All 48 higher-budget 32 kb/s packet/PCM
fingerprints are identical to the baseline. Per-condition
[paired intervals](measurements/2026-10-14-opus-quality-pitch/evaluation-before-after.json)
and all regressions remain visible.

The separate [Hybrid run](measurements/2026-10-14-opus-quality-pitch/hybrid.csv)
contains 48 cases; mean STOI/ESTOI changes are +0.00193/+0.00236 and LSD -0.08488 dB.
Clean [isolated](measurements/2026-10-14-opus-quality-pitch/isolated.csv) and
[burst](measurements/2026-10-14-opus-quality-pitch/burst.csv) loss runs add 48 cases
each, with mean STOI changes +0.00999/+0.00264. All received-packet entropy
intervals, complete byte budgets, sample counts and finite-output checks pass.
These are development gains rather than reference or listening acceptance.

The [CPU follow-up](measurements/2026-10-14-opus-quality-pitch/cpu/summary.csv)
uses new utterances cen1-mdms2-b and cen1-mjgm-b with the same 14-case, three-repeat
method above. WB 12 kb/s p50 processing costs rise 12.40% and 18.28% over `fdc18d0`;
current/libopus ratios remain 0.858 and 0.765. Thirteen of fourteen p50 ratios are
below libopus; one Hybrid case is 1.042, and eleven cases exceed the reference at
one or more reported percentiles. These are different utterances from the first
CPU run, so their percentages cannot be combined with its gains.

All 287 debug and 302 release/reference tests, clippy and transform reproduction
pass. A new relaxed-pitch check covers silence, valid lag bounds and contractive
filters. **The quality and full CPU goals remain open:** all 36 new reference
objective conditions fail at least one confidence gate. The previously prepared
blind package evaluates `fdc18d0`; no human ratings have yet been supplied.
Subsequent tuning requires further independent evaluation material.

Official RFC 8251 vectors are downloaded independently:

```sh
python3 tools/reference/check_opus_vectors.py /path/opus_newvectors \
  --opus-compare /path/opus_compare --channels both \
  --output-dir /tmp/snacka-opus-vectors
```

The checker validates published reference hashes, each packet's final interval
and the unchanged published PCM metric. It selects phase inversion consistently
with the normal or no-phase reference set. All 24 normal-reference runs passed
on 2026-10-13: 40,150 packets, exact final ranges, no clipped/non-finite output,
quality scores 68.0–99.9. Every vector meets the required nonnegative score;
some remain below the recommended 90. These measure decoder agreement rather
than speech listening quality. The owned SILK resampler is an independent stable
rational filter using the standard timing model; its PCM is approximate.

Additional native checks cover 23,405 packet comparisons and 450 loss/FEC
intervals, including 1,280 strict CELT PCM/channel/rate comparisons,
packed/padded 120 ms packets, FEC, DTX, mode changes, surround and Ambisonics.
Owned SILK encoding has a 1,200-packet public native decoder proof across all
24 native geometries. FEC and resumed DTX have separate long-signal regression
gates. See [fixture provenance](../tests/fixtures/README.md),
[fuzzing](../fuzz/README.md) and [call evidence](opus-calls.md).

## Ownership and licensing

The Rust algorithms and generated filter models are independently implemented
under MIT. Mandatory format tables and allocation constants from
[RFC 6716](https://www.rfc-editor.org/rfc/rfc6716), including normative Appendix A
data, retain their BSD notice in [LICENSE-OPUS-DATA](../LICENSE-OPUS-DATA).
[RFC 8251](https://datatracker.ietf.org/doc/html/rfc8251) corrections and
[RFC 8486](https://datatracker.ietf.org/doc/html/rfc8486) Ambisonics mappings are
covered by their wire tests. The package declares `MIT AND BSD-3-Clause`.

Reference tools and compiler requirements are excluded from the published
crate. `design_opus_resampler.py` derives the decoder model from permitted
black-box observations and mathematical constraints; `design_opus_downsampler.py`
generates the encoder's causal FIR from windowed-sinc mathematics. Neither reads
external algorithm bodies or coefficient tables. Experimental draft extensions
such as DRED can retain their padding bytes for interoperability; neural DRED
reconstruction is not implemented or advertised as part of this RFC core.
