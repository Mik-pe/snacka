# Development and acceptance

Build a small, modern Rust speech codec with its own bitstream. Implementation
source must be written independently: do not copy, translate, wrap or vendor
external codec algorithms. Mathematical descriptions, published research and
black-box results may inform design. External codecs are test oracles and
competitors, never a required runtime path or fallback implementation.

## Current implementation

A working DSP baseline encodes 48 kHz mono audio into native packets at
12–32 kb/s and decodes it again. Frame duration is negotiated before stream
construction: 20 ms by default, 10 ms optionally. Each encoder and decoder owns
its transform workspace, spectral history and short overlap. Byte budgets can
change per frame without a new decoding configuration.

Sparse coefficient selection and one shared gain are deliberately simple
baselines. Per-frame selection between squared four-bit and linear six-bit
amplitudes balances retained coefficients against amplitude precision. Gain
selection minimizes coefficient reconstruction error. The main weakness remains
speech fidelity, especially unvoiced speech
and dense spectra. Concealment repeats decaying spectral history and is not a
speech predictor. There is no trained neural component, FEC, stereo, adaptive
frame switching or production quality claim.

## Next experiments

1. Improve spectral coding: band gains, perceptual allocation, prediction and
   vector quantization. Compare matched-rate outputs on diverse speech,
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
