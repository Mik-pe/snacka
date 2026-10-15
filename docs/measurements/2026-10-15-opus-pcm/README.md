# Public PCM API observations

`native.json` records public libopus 1.6.1 calls on the identified macOS arm64
build. The reference library SHA-256 and host are embedded. Only public headers
and callable APIs were used; no reference implementation source was read or
added to Snacka. Reproduce from the repository root:

```sh
python3 tools/reference/inspect_opus_pcm.py /opt/homebrew/lib/libopus.dylib \
  --output docs/measurements/2026-10-15-opus-pcm/native.json
```

The probe generates eight 20 ms mono CELT packets and independently decodes
them through the float, signed-16 and signed-24-in-32 APIs at 0, 12 and 60 dB of
public decoder gain. Python's integer arithmetic, exact power-of-two scaling
and nearest/even rounding provide the conversion oracle. All 23,040 signed-24
outputs, including 3,019 exact half ties, match rounding float PCM scaled by
8,388,608 and saturating to the **full i32 limits**. At 12 dB the outputs exceed
the nominal signed-24 range; at 60 dB they saturate at i32 limits. The exposed
sample format therefore does not imply clipping to 24 bits.

Signed-16 decoding differs from deterministic nearest/even hard clipping. The
public `opus_pcm_soft_clip` operation, retaining one state value across packets,
followed by half ties away from zero matches all signed-16 outputs on this
build. Half ties differ even below unity; rounding behavior from this build
is an observation, not a cross-platform API promise. Snacka's utility states a
portable nearest/even contract and does not claim native signed-16 decoder
equivalence or implement soft clipping.

With the public LSB-depth hint set to 16, native signed-16, shifted signed-24
and exactly scaled float inputs produce identical packets in four consecutive
checks. The header specifies an effective depth capped at 16 for `opus_encode`,
whereas the float API defaults to 24. Snacka does not yet expose that hint.
The native signed-24 encoder also accepts a generated waveform whose peak is
approximately 8,500,000, beyond the nominal signed-24 range. Snacka still rejects encoder
float input outside ±1. Format conversion preserves that extended input so the
encoder can reject it explicitly instead of silently clipping.

Owned checks cover every signed-16 input value, signed-24/i32 extrema, positive
and negative half ties with an independent integer oracle, infinities, atomic
length/NaN errors, destination suffixes and zero processing allocations. The
same buffers compose with ordinary, mapped and rectangular projection codecs
at all five standard rates, including concealment and recovery. No codec audio,
extended-dynamic-range encoding or performance parity follows from these checks.
