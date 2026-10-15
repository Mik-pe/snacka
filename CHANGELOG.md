# Changelog

## 0.2.0

- Accept all five standard Opus input sample rates in ordinary and mapped
  encoders, with owned causal interpolation and an exact 48 kHz bypass.
- Add a stateful mapped multistream encoder with allocation-free processing and
  aggregate packet budget preservation, plus ordinary 80/100/120 ms packets.
- Make `has_fec` report previous-packet recovery from the first frame instead of
  redundancy internal to a multi-frame packet.
- Add the owned SILK/Hybrid encoder and a common configurable Opus encoder,
  including voiced prediction, real in-band FEC, DTX and transition redundancy.
- Add all five standard decoder output rates, history-based packet-loss
  concealment and recovery from the following packet's SILK redundancy.
- Add allocation-free repacketization and multistream composition, mapped
  decoding and Ambisonics projection. Extend Ogg handling to mapping families
  0/1/2/3/255, continued packets, chained streams and multiplexing.
- Add an RTP/UDP interoperability example and public libopus reference checks.
- Reduce transform, band allocation, resampling, encoder quantization and
  concealment CPU costs while
  preserving reference conformance. Full performance parity with libopus is
  still an active goal; slower measured cases remain.
- Keep the optional `opus` feature free of dependencies, unsafe code, external
  codec implementations and processing allocations. Prepare codecs before
  entering an audio callback.
