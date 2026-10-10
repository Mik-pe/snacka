# Independent test fixtures

## Opus entropy

`entropy-libopus-1.6.1.bin` contains 128 independently encoded entropy frames,
each with 128 events. The events cover cumulative symbols with boundaries
`[0, 27, 200, 256]`, bits with probability `2^-logp`, reverse raw bits and
uniform integers, including a `u32::MAX` alphabet. Each record includes the
interval, whole-bit usage and eighth-bit usage after its event. The generator
checks all records with libopus's decoder before saving them. No external
implementation source or recorded audio is included in the fixture.

The binary schema is little-endian:

- Eight-byte magic `SNKENT01`, followed by `u32` frame count.
- Per frame: `u16` packet length, `u16` event count, then packet bytes.
- Per event: `u8` kind and five `u32` fields: parameter, value, interval,
  whole-bit usage, eighth-bit usage. Kinds are symbol=0, bit=1, raw=2, uniform=3.

Regenerate using a separately installed **libopus 1.6.1 static archive**:

```sh
python3 tools/reference/generate_entropy_fixtures.py /path/to/libopus.a
```

The original test glue in `tools/reference/entropy_bridge.c` exposes oracle
calls and state fields; it contains no codec algorithms. It is compiled into a
temporary library only by this optional Python tool. Private oracle ABI layout
is version-specific, so the tool refuses other versions. Normal Cargo builds
and tests never compile or link this glue.

Check interoperability in the other direction, including our encoder's
termination and raw suffix compaction:

```sh
cargo run --quiet --example entropy_vectors > /tmp/snacka-native-entropy.bin
python3 tools/reference/generate_entropy_fixtures.py /path/to/libopus.a \
  --verify-file /tmp/snacka-native-entropy.bin
```

Add `-- --padded` to the example command to validate full-budget termination
used by native audio packets.

The example encodes the same event sequences using snacka. The tool independently
decodes its output using libopus and checks every value and state record.
