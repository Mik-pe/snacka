# Owned Opus support

Enable `opus` for standard Opus packet and Ogg Opus file support. The feature
adds no dependencies. Production code is safe Rust owned by Snacka, sharing
the native FFT/MDCT and entropy engines. There is no external codec
implementation, build script, C, FFI or CMake in the library build.

## API and supported formats

`snacka::opus::Decoder` reads ordinary single-stream SILK, Hybrid and CELT
packets into normalized interleaved f32 PCM at 48 kHz. Construct it with one
or two output channels. Float output is finite but can exceed ±1 through
ringing or gain; clip or limit it when converting to integer PCM. It duplicates
coded mono to stereo or averages coded
stereo to mono, preserving layer history through coded channel and mode changes.
Packet framing supports CBR/VBR, padding, multiple frames and the standard
120 ms packet limit. Allocate up to 5,760 output samples per channel.

`CeltEncoder` writes standard CELT packets at 48 kHz, with one or two channels,
2.5/5/10/20 ms frames, and narrowband/wideband/superwideband/fullband bandwidth.
`CeltConfig` validates rates from 6–512 kb/s against the selected duration's
3–1,276-byte packet bounds. The bitrate includes the TOC byte; fractional byte
budgets carry between packets. `set_bitrate` preserves history. The encoder
currently selects CELT only; it does not contain a SILK encoder, adaptive
application selection, DTX or an in-band FEC encoder.

```rust
use snacka::opus::{CeltConfig, CeltEncoder, Decoder};

let config = CeltConfig::default(); // 32 kb/s, mono, 20 ms, fullband
let mut encoder = CeltEncoder::new(config);
let mut decoder = Decoder::new(config.channels()).unwrap();
let mut packet = [0; 1276];
let mut pcm = [0.0; 960];
let length = encoder.encode(&[0.0; 960], &mut packet).unwrap();
assert_eq!(decoder.decode(&packet[..length], &mut pcm).unwrap(), 960);
```

Prepare codecs before the audio callback. Encoding, decoding, reset and bitrate
changes use fixed working buffers and do not allocate. `CeltDecoder` offers
the narrower CELT-only API with identical coded/output channel counts.
`Packet::parse` borrows the bytes and validates framing without decoding audio.
A malformed compressed frame can partially advance decoder history: call
`reset` after `DecodeError::Corrupt`. Framing and output capacity errors are
checked before history changes.

The encoder's transform delay is 120 samples per channel. Raw packet users
must handle start delay and end padding. The file example handles both:

```sh
cargo run --release --features opus --example opus_roundtrip -- \
  input.f32 output.opus decoded.f32 32000 1 20
cargo run --release --features opus --example opus_roundtrip -- \
  decode existing.opus output.f32
```

Input/output PCM is interleaved little-endian f32 at 48 kHz. The reader/writer
in `opus::ogg` supports one logical stream with mapping family 0, mono/stereo,
CRC checks, continued packets, tags, gain, pre-skip and final granule trimming.
It rejects multiplexing, chaining and surround mappings. Container operations
allocate and belong outside the callback.

The decoder consumes SILK's coded redundancy when present but exposes no FEC
recovery API. Empty/one-byte compressed frames use a simple independent fade;
there is no reference-quality packet-loss concealment claim. Empty complete
packets are framing errors. The SILK resampler uses an owned stable rational
filter with the delays specified in RFC 6716 Table 54. Its PCM is approximate,
as permitted by the standard; CELT reference comparisons are much tighter.

## Verification and performance

Normal feature tests use synthetic checked-in oracle observations; they do
not require a foreign library. Optional development tools use separately
installed libopus or the RFC's external reference executable. They contain
call glue rather than a second production codec implementation.

```sh
cargo test --locked --features opus
cargo test --locked --release --features opus
cargo clippy --locked --all-targets --features opus -- -D warnings
cargo bench --locked --bench opus --features opus
RUSTFLAGS="-L /path/to/lib" cargo test --locked --release \
  --features opus,reference-tests --test opus_reference
RUSTFLAGS="-L /path/to/lib" cargo bench --locked --bench opus \
  --features opus,reference-tests
```

The benchmark compares complete equal-size packets with external libopus
restricted-lowdelay CBR at complexity 10. Its percentiles summarize calibrated
batch-average throughput, not individual callback latency. The default covers
24 matched channel/rate/duration/input cases; `--full` covers 72. Supply real
48 kHz f32 PCM with `--input FILE --input-channels N`. CPU results do not
establish equivalent encoder speech quality.

The official RFC 8251 decoder vectors are intentionally downloaded separately:

```sh
python3 tools/reference/check_opus_vectors.py /path/opus_newvectors \
  --opus-compare /path/opus_compare --channels both \
  --output-dir /tmp/snacka-opus-vectors
```

The runner checks every packet's final arithmetic interval and the published
PCM metric for all twelve vectors, in both output channel counts. It verifies
the published vector and reference hashes before decoding. It never
loosens the official comparator. RFC 6716 requires a nonnegative PCM quality
score and recommends exceeding 90 at 48 kHz; the score measures decoder
agreement and is not a speech listening score.

On 2026-10-13, the complete normal reference set passed all 24 runs at 48 kHz:
40,150 packets, exact final intervals, no clipped/non-finite output. Decoder
agreement scores were 68.0–99.9, meeting the required threshold for every
vector; several remain below the recommended 90. The lower scores leave room
to improve SILK synthesis/resampling agreement.

| Vector | Mono output | Stereo output |
| --- | ---: | ---: |
| 01 | 98.4 | 99.9 |
| 02 | 91.1 | 89.9 |
| 03 | 88.6 | 88.7 |
| 04 | 85.8 | 86.0 |
| 05 | 77.3 | 77.6 |
| 06 | 68.0 | 68.3 |
| 07 | 98.1 | 99.8 |
| 08 | 95.5 | 94.6 |
| 09 | 97.4 | 93.7 |
| 10 | 96.7 | 87.9 |
| 11 | 98.4 | 99.8 |
| 12 | 88.8 | 88.8 |

On the same date, three paired CPU repetitions on Apple M5/macOS 27.0/aarch64
against libopus 1.6.1 produced the following CELT throughput measurements.
The 24-case matrix spans four durations, both channel counts and
voiced/noise/transient signals at balanced 24/64/96 kb/s budgets. Reference
encoder complexity is 10; Snacka uses fixed coding decisions. Both codecs'
complete raw packet lengths, including TOC, match in every case.

| Input and operation | Snacka / libopus CPU time |
| --- | ---: |
| Balanced 24 cases, encoding | 0.681× median; 0.447–0.960× across cases |
| Balanced 24 cases, decoding | 1.723× median; 1.239–2.038× across cases |
| Real voice, 20 ms mono, 32 kb/s, encoding | 23.68 / 49.73 µs; paired ratio 0.481× |
| Same real voice, decoding | 14.40 / 10.86 µs; paired ratio 1.274× |

Each decoder consumes its own encoder's equal-byte packets. The real voice
input contains 512 frames of LibriSpeech test-clean `61-70968-0011`, with
[corpus attribution and license](reference-quality.md#corpus-and-scoring). Its 48 kHz
f32 input SHA-256 is
`f65345be877b5790088e2d53ee5c004bad90a20d3fb17fcc5f291961daed5b8e`.
Codec order alternated between repetitions, with 128 warmups and 1,000
calibrated batches per measurement. Existing background work was preserved;
these were paired host measurements with contention, not exclusive CPU runs.
Encoding is competitive on these profiles; decoding still costs more CPU than
libopus. Equal packet budgets and lower CPU do not prove equal speech quality.

Complete 48 kHz stereo decoding of the same cached official packets was also
measured in three paired repetitions, after checking every duration and final
interval. Eight full-stream warmups preceded 31 timed passes per repetition;
public reset was timed, while initialization, file IO and interval inspection
were excluded.

| Stream | Snacka / libopus mean µs per packet | Median paired CPU ratio |
| --- | ---: | ---: |
| Vector 02, SILK | 12.40 / 7.72 | 1.626× |
| Vector 05, Hybrid | 23.53 / 13.03 | 1.806× |
| Vector 06, Hybrid | 24.51 / 14.64 | 1.674× |

These streams cover 5,098 packets with varied coded durations and channels.
The owned decoder remains roughly 1.6–1.8× the reference CPU cost on these
profiles; decoder performance parity is still a development target.
The optional [complete-decoder timing tool](../tools/reference/benchmark_opus_decode.rs)
documents the isolated build, public libopus linkage and repeatable stream
validation/timing commands.

Kernel oracles cover packet framing, mixed entropy operations, integer
allocation, PVQ vectors, energy, band reconstruction and SILK gain/pitch/NLSF
decoding. The separate public libopus test covers both encoding directions,
mono/stereo, frame durations, bandwidths and coding transitions. See
[fixture provenance](../tests/fixtures/README.md) and [fuzzing](../fuzz/README.md).
Mobile target compilation and allocation checks are build/runtime evidence;
physical-device audio and perceptual encoder acceptance remain separate work.

## Ownership and licensing

The Rust algorithms and resampler model are independently implemented under
MIT. Mandatory probability tables, allocation curves and other normative
format constants originate in [RFC 6716](https://www.rfc-editor.org/rfc/rfc6716),
including its reference data. Their BSD attribution is retained in
[`LICENSE-OPUS-DATA`](../LICENSE-OPUS-DATA) and marked in affected source files.
Corrections in [RFC 8251](https://datatracker.ietf.org/doc/html/rfc8251),
including Hybrid folding, are exercised through the updated official vectors.
The package declares `MIT AND BSD-3-Clause` to preserve both sets of notices.

External implementation source and recorded audio are not vendored. Reference
tools and their compiler/NumPy requirements are development-only and excluded
from the published crate. `design_opus_resampler.py` regenerates the owned
model from observed impulse responses and mathematical constraints; it reads
no reference coefficient tables or algorithm bodies.
