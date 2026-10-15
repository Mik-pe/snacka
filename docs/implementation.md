# Development and acceptance

Build a small, modern Rust speech codec with its own bitstream. Native Snacka
implementation source must be written independently: do not copy, translate,
wrap or vendor external codec algorithms. Mathematical descriptions,
published research and black-box results may inform design. External codecs
are test oracles and competitors, never a required runtime path or fallback
implementation.

The optional `opus` feature implements a second, standardized bitstream owned
by Snacka. Its wire behavior follows RFC 6716, including the normative
specification in Appendix A. Format probability tables and allocation constants
carry their original notice in [LICENSE-OPUS-DATA](../LICENSE-OPUS-DATA).
The Rust implementation reuses Snacka's entropy and transform kernels; it adds
no codec dependency, C/FFI, build script or unsafe Rust. Native format versioning
and Opus negotiation remain distinct. Completion requires independent
interoperability evidence for every advertised coding mode, and competitive
performance requires measurements of complete frames on diverse audio.
The Opus performance goal is CPU time at most equal to libopus for each matched
encoding, decoding and loss-recovery case, including paired throughput percentiles.
The test matrix compares actual wire modes and identical decoder packets;
aggregate medians alone do not close the goal. Current evidence and remaining
gaps live in [Opus support](opus.md). The Rust API may compose format operations
differently from the reference API.

The active delivery objective is full Opus parity with libopus 1.6.1 in
capabilities, processing cost and audio quality. Functional scope includes PCM
formats and standard sample rates, ordinary/mapped/Ambisonics encoding and
decoding, packet/container operations, loss recovery and encoder policies.
The [1.6 API](https://opus-codec.org/docs/opus_api-1.6.pdf) and
[extension descriptions](https://opus-codec.org/demo/opus-1.6/) also identify
24-bit PCM, DRED, speech enhancement and Opus HD capabilities. Unsupported or
unexercised extensions remain explicit gaps; their absence from one installed
reference build does not close them.

`tools/reference/inspect_opus_capabilities.py LIBOPUS --output OUT.json` records
the reference library hash, exported APIs, sample-rate constructors and accepted
controls. This inventories the selected build; symbol or control availability
alone does not prove working encoding, decoding or quality.

## Performance goal

Make Snacka as fast as libopus or faster without sacrificing correctness,
audio quality or bounded, allocation-free processing. The first acceptance
target is the owned standard Opus implementation, where wire modes and decoder
packets can be matched directly.

For every matched case, require Snacka/libopus processing-time ratios at most
1.0 at paired p50, p95 and p99. These are percentiles of batch throughput;
individual callback deadlines require separate measurements. A faster aggregate
median cannot compensate for a slower case. Missing, failed or unmeasured cases
keep the goal open.

- Encoding: identical changing PCM, actual wire mode, bandwidth, frame duration,
  channel count and complete packet byte budget; record reference complexity.
- Decoding and loss recovery: identical packets, output rates, channel counts
  and loss schedules, covering SILK, Hybrid and CELT. Run the full 2,130-case
  decoder matrix, including ordinary decode, PLC return, FEC return and
  aggregated packets through 120 ms.
- Evidence: serialize measurements on frozen sources, alternate codec order
  and retain raw results, source hashes, toolchain, build flags, libopus version
  and host details. Repeat final measurements to confirm parity. Track ARM64
  and x86-64 separately; one passing host closes only that host's result.
- Quality: retain interoperability, official-vector and mathematical checks,
  finite output and zero processing allocations. Preserve exact packet/PCM
  traces for changes intended to preserve output. Evaluate deliberate encoder
  or concealment output changes separately for quality before accepting speed
  gains; equal packet budgets alone do not establish equivalent quality.

Prioritize complete-decoder cases currently above 1.0, profiling SILK synthesis
and resampling, Hybrid processing and short-frame CELT before choosing an
optimization. Recheck encoder p95/p99 regressions, then rerun both complete
matrices against libopus. Current gaps and benchmark commands are in
[Opus support](opus.md#verification-and-performance-target); the full decoder
driver is `tools/reference/benchmark_opus_matrix.py`.

The native Snacka format also targets libopus-or-better processing cost, but
uses a different bitstream. Compare matched PCM, packet budgets, frame durations
and documented delay using the [CPU](cpu.md) and
[quality](reference-quality.md) methods. Native results do not close the
standard Opus target, and a faster native codec still needs quality validation.

## Quality goal

Make Snacka sound at least as good as libopus while meeting the performance
goal. Both targets must pass together; reduce measured cost without degrading
quality, packet meaning, numerical bounds or allocation-free processing.

Compare the same PCM with matched complete packet byte budgets, actual coded
bandwidth, channels, frame duration and documented algorithmic delay. Match FEC,
DTX and loss schedules where those services are supported. Report native-format
and standard Opus results separately, including their framing/service overhead.

For each declared speech condition, require STOI and ESTOI at least equal to the
reference and speech-band log-spectral distance no greater than the reference.
Publish per-clip regressions and paired confidence intervals alongside each
condition's results. Uncertain comparisons keep the goal open; improvements in
one bitrate, speaker group or noise condition cannot hide a failing condition.
Use perceptual audio metrics appropriate to music and transient material rather
than applying speech intelligibility scores to them.

Confirm objective results with randomized blind paired listening on independent,
reproducibly licensed held-out audio. Cover clean/noisy speech, different voices
and languages including Swedish, overlapping talkers, music/transients, isolated
losses, burst loss and recovery. Freeze evaluation conditions before tuning and
keep evaluation inputs separate from development inputs. Listening must support
no worse quality in every declared condition before claiming parity.

Owned Opus decoding must also retain exact ordinary entropy intervals, official
vector conformance and finite output; target the recommended score of 90 or
higher for every official vector, in addition to the mandatory nonnegative
score. These checks validate decoding and do not establish encoder or PLC quality.
Exact baseline packet/PCM traces remain the gate for output-preserving CPU
changes. Output-changing experiments need fresh quality evidence before adoption.
Current native quality gaps are recorded in the
[matched reference](reference-quality.md); standard Opus evidence and CPU gaps
are in [Opus support](opus.md). Neither quality nor full CPU parity is claimed
complete.

## Current implementation

A working DSP baseline encodes 48 kHz mono audio into native packets at
12–32 kb/s and decodes it again. Frame duration is negotiated before stream
construction: 20 ms by default, 10 ms optionally. Each encoder and decoder owns
its transform workspace, spectral history and short overlap. Byte budgets can
change per frame without a new decoding configuration. Four framing/checksum
bytes leave the rest of each packet available for spectral coding.

Per-packet selection compares two spectral models: sparse four-/six-bit scalar
amplitudes with one gain, and four independent band gains with normalized signed
pulse vectors. Five-to-eight-bit pulse gains derive from the frame size and
packet budget, retaining more shape bits at low bitrate. The second model spends
amplitude precision collectively on a
band shape. A bounded pulse exchange search improves normalized correlation
after the initial projection and greedy assignment. Pulse allocation is inferred
deterministically from gain codes and packet length, requiring no allocation side information. Both models use native
arithmetic coding and bounded stack workspaces.
The encoder compares 8/12/24 kHz coded bandwidths and both models over the full
transform, counting omitted high frequencies as error. Full-band coding remains
available for high-frequency content. Current objective speech comparisons and
CPU costs are published in [quality evaluation](quality.md).
The main weakness remains
speech fidelity, especially unvoiced speech
and dense spectra. Concealment extrapolates recent decoded PCM with period
estimation, fractional interpolation and a 40–80 ms fade. It cannot predict
new phonemes or reconstruct missing information. There is no trained neural component, FEC, stereo, adaptive
frame switching or production quality claim.

## Next experiments

1. Prioritize the 12 kb/s intelligibility gap identified by the
   [matched reference](reference-quality.md), especially at 10 ms. Improve
   spectral coding: perceptual allocation, prediction, transient handling
   and improved vector searches. Compare matched-rate outputs on diverse speech,
   background noise, overlap between talkers and transient signals. Delete
   superseded implementations after a measured replacement wins.
2. Improve packet-loss concealment and transitions using reproducible isolated
   and burst loss traces. Prevent clicks and limit error propagation without
   requiring reliable transport.
3. Profile complete processing on ARM64 and x86-64, including mobile hardware,
   initialization, stream storage, individual callback tails under load and
   battery use. Optimize measured bottlenecks while retaining independent
   mathematical accuracy tests.
4. Explore small causal neural postfilters only after the DSP baseline and
   evaluation pipeline are credible. Integrate models only with measured
   benefit, appropriate training data and compatible weight licensing. No
   mandatory GPU or network inference. Required decoder models need a specified
   profile/model identity; optional postfilters must not change packet meaning.

## Real-time integration contract

The codec stays independent of networking and audio devices. Capture and native
microphone processing supply normalized 48 kHz samples. One encoded frame is one
bounded packet. A relay forwards packet bytes unchanged; analysis decodes its
own copy with independent history. Slow transcription or inference must not
block relay, playback or microphone callbacks. Participant tracks remain
separate until an application intentionally mixes decoded audio.

The transport negotiates profile, authenticates packets, tracks stream identity
and arranges delivery deadlines. The decoder does not guess whether a sequence
gap means loss or reordering. Explicitly conceal a missed deadline, reject late
packets, and reset both sides when starting another stream.

## Acceptance

Correctness tests prove dimensions, byte budgets, sequence wrap, refusal without
history mutation, numeric bounds, overlap alignment and no processing
allocations. Fuzzing must reach the decoder with valid checksums as well as test
framing rejection. Cross-compilation verifies source portability, not installed
mobile behavior.

A competitive quality claim requires public, reproducibly licensed corpora,
matched bitrate, frame duration and delay, perceptual metrics, blind listening
and packet-loss scenarios. Publish regressions alongside wins. CPU throughput
on a simple tone does not establish quality, battery life or universal speed.
Do not call a prototype production-ready because its roundtrip tests pass.
