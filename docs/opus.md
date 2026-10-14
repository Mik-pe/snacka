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
