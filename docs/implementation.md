# Implementation and acceptance

The target is a complete Opus encoder and decoder implemented independently in
Rust. Existing implementation source must not be copied, translated or wrapped.
The specification's mathematics, numeric format parameters and independently
generated test results are the basis for implementation. Reference programs may
be executed as black boxes.

## Current boundary

Packet parsing describes framing only. Range coding describes entropy only. The
MDCT operates on caller-provided floating-point blocks. These components cannot
yet turn an Opus audio packet into PCM or PCM into an Opus audio packet. MDCT
scaling, coefficient order and numerical accuracy will also require checks
against complete interoperable codec output.

The packet parser handles ordinary single-stream framing, not the optional
self-delimiting multistream extension. Raw entropy reads enforce physical frame
bounds. Arithmetic decoding zero-extends the prefix as required by the format;
the future audio decoder must also enforce its bit budget and parameter bounds.

## Delivery sequence

1. Validate the shared entropy and transform components independently, including
   malformed inputs, allocation behavior and reproducible kernel measurements.
2. Implement CELT band energy, pulse-vector indexing, bit allocation, spectral
   reconstruction, postfilter and persistent overlap state. Validate packets
   from an external encoder before designing our encoder's analysis decisions.
3. Implement SILK prediction, excitation, resampling and persistent state, then
   hybrid operation and mode transitions. Cover packet loss concealment and FEC.
4. Implement encoder analysis and rate control. Check our packets with an
   independent decoder at each supported frame duration, rate and channel count.
5. Run standard vectors and cross-implementation tests, sustained fuzzing and
   corpus-based quality and performance comparisons. Only then publish full
   codec compatibility claims.

## Engineering requirements

- Allocate stream storage before audio processing. Keep immutable plans reusable
  across streams and mutable history owned by each stream.
- Validate trust boundaries and preserve documented failure behavior. Bounded
  arithmetic, packet sizes and work per frame matter as much as valid audio.
- Keep one canonical implementation. Optimize measured bottlenecks; retain
  independent mathematical tests when changing layout, precision or arithmetic.
- Match bitrate, channels, frame duration, algorithmic delay and quality when
  comparing complete codecs. Publish losses as well as wins.
- Treat claims of higher quality, speed or robustness as results to establish,
  not properties implied by implementation language or model capability.
