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

## Native version 3 framing

The inline short/long packet literals in `tests/codec.rs` are complete silence
packets emitted by the native encoder at commit
`f66d2c8825e8ccc828f2be1bb6933d12b116adcd`, using 12,000 bits/s and 10/20 ms
respectively. Their original checksum bytes are retained. They test rejection
of the previous experimental packet layout without consuming stream history.
They contain no external implementation or recorded audio.

## Standard Opus kernels and SILK

The `opus` feature uses additional synthetic observations from external codec
calls. These are test data, not external implementation source or recorded
audio. Production code never reads these files. Binary readers in the matching
Rust tests specify the record layout and check that the whole file is consumed.

| Fixture | Coverage | Generator |
| --- | --- | --- |
| `src/opus/testdata/range-libopus-1.6.1.bin` | 32 frames, 3,072 mixed ICDF/Laplace/range/raw operations, exact bytes and coder state | `generate_opus_range_fixtures.py` |
| `src/opus/testdata/pvq-libopus-1.6.1.bin` | Pulse-vector rank/unrank, dimensions and boundary codebooks, exact integer/range state | `generate_opus_pvq_fixtures.py` |
| `src/opus/testdata/bands-rfc6716.bin` | 256 band reconstructions, spectral coefficients, masks and coder state | `generate_opus_bands_fixtures.py` |
| `src/opus/testdata/bands-anticollapse-rfc6716.bin` | Eight transient anti-collapse cases | Same bands generator |
| `src/opus/testdata/bands-hybrid-libopus-1.6.1.bin` | 24 start-17 hybrid cases, including unequal-width folding, mono/stereo and transients | `generate_opus_hybrid_bands_fixtures.py` |
| `src/opus/testdata/resampler-response.bin` | 12 complex response frequencies at each of 8/12/16 kHz | `design_opus_resampler.py` |
| `src/opus/testdata/celt-to-hybrid.opus` | 80-byte raw transition packet from synthetic tones, testing reset after beginning redundancy | `generate_celt_transition.py` |
| `opus-silk-pulses.bin` | 610 shell/LSB/sign/excitation cases, exact pulses and coder state | `generate_opus_silk_fixtures.py` |
| `opus-silk-decode.bin` | 456 coded frames with 8/12/16 kHz rates, mono/stereo, 10/20/40/60 ms, coded redundancy and channel changes | `generate_opus_silk_decode_fixtures.py` |

All generators live under `tools/reference/`. The libopus observations use
version 1.6.1 and its separately installed static archive. Example commands:

```sh
python3 tools/reference/generate_opus_range_fixtures.py /path/libopus.a
python3 tools/reference/generate_opus_pvq_fixtures.py /path/libopus.a --verify-native
python3 tools/reference/generate_opus_silk_fixtures.py /path/libopus.a
python3 tools/reference/generate_opus_silk_decode_fixtures.py /path/libopus.a
python3 tools/reference/generate_opus_hybrid_bands_fixtures.py /path/libopus.a
```

The bands generator uses the separately extracted RFC 6716 Appendix A oracle:

```sh
python3 tools/reference/generate_opus_bands_fixtures.py /path/opus-rfc6716
```

The archive is the RFC's normative specification with SHA-1
`86a927223e73d2476646a1b933fcd3fffb6ecc8c`. No copy is checked into Snacka.
The generator compiles only external source plus our original call glue into
a temporary executable. Format constants used by Rust preserve BSD notices
in `LICENSE-OPUS-DATA`; original algorithm code remains MIT.

The resampler generator requires NumPy and matching external libopus headers
for development. It fits original model coefficients from observed impulses
at thirteen amplitudes, then constrains unit DC gain and the standard delay.
Neither production code nor ordinary tests require NumPy, foreign compilation
or an installed codec. Its compact fixture stores frequency observations only.
Use `--output-dir` outside the repository to keep design artifacts separate;
`--write` explicitly replaces the owned inline model and its fixture.

SILK tests also retain small gain/pitch/stereo/NLSF oracle vectors and output
digests inline. Energy and allocation differential tools compare states from
the external normative oracle; no reference source is compiled by Cargo.

The transition generator uses synthetic 180/360/720 Hz tones, eight 20 ms
music-biased frames at 128 kb/s and a voice-biased frame at 32 kb/s with external
libopus 1.6.1. The retained packet is raw Opus, not an Ogg container. The public
decoder regression checks that two different old CELT histories cannot alter
the new Hybrid highband after redundancy; restoring the former reset condition
makes this regression fail. No official vector audio is retained.
