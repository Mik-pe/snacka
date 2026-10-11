![Snacka audio codec banner](assets/snacka-banner.webp)

# snacka

An independently implemented, MIT-licensed Rust codec for interactive speech,
with its own versioned bitstream. Snacka is the codec under development;
external codecs are measurement competitors and test oracles only.

**Experimental DSP prototype.** It encodes and decodes audio today, but its
speech quality and loss concealment need substantial improvement before use in
production calls. No superiority over established codecs is claimed.

- Mono normalized f32 PCM at 48 kHz.
- 20 ms frames by default; optional 10 ms frames.
- 12–32 kb/s including four bytes of framing and checksum per packet.
- Bitrate changes retain stream history; fractional byte budgets remain exact.
- Per-packet coded bandwidth of 8/12/24 kHz; PCM stays at 48 kHz.
- 120 samples (2.5 ms) of transform delay beyond frame buffering.
- Caller-owned buffers; no allocation during encoding, decoding, concealment,
  bitrate updates or reset. Each track owns its independent history.
- Default library builds have no dependencies, build script, C, FFI, CMake or unsafe Rust.

The encoder uses a short-overlap MDCT, sparse spectral selection, adaptive
four- or six-bit quantization, adaptive coded bandwidth
and arithmetic coding. A second spectral model independently codes four band
gains and normalized signed-pulse vectors. Each packet chooses the model and
bandwidth with greatest full-spectrum error reduction. The decoder validates framing,
CRC, stream sequence and spectral bounds, with explicit concealment for missing
packets. CRC detects corruption; authentication belongs to the transport.
Pulse gain precision adapts to the packet budget, retaining more shape bits at
low bitrate. Signed pulse classes recover additional shape bits without increasing
packet size. The experimental format is version 7. Encoder and decoder must use the same
version and support every negotiated profile; older native packets are rejected.
The opt-in harmonic speech model can emit profiles 14/15, which earlier version-7
decoders reject. `Config::with_harmonic_speech(true)` requires peer support for
these profiles and increases encoder CPU at 12–16 kb/s. It is disabled by default.

```rust
use snacka::{Config, Decoder, Encoder, FRAME_SAMPLES, MAX_PACKET_BYTES};

let mut encoder = Encoder::new(Config::default());
let mut decoder = Decoder::new();
let input = [0.0; FRAME_SAMPLES];
let mut packet = [0; MAX_PACKET_BYTES];
let mut output = [0.0; FRAME_SAMPLES];
let length = encoder.encode(&input, &mut packet).unwrap();
let frame = decoder.decode(&packet[..length], &mut output).unwrap();
assert_eq!(frame.valid_samples, FRAME_SAMPLES);
```

Discard the first `ALGORITHMIC_DELAY` output samples once per stream. At its
end, encode and decode `Encoder::finish` and retain the drain's
`FrameInfo::valid_samples` prefix. The offline example handles this alignment
and partial input frames:

```sh
cargo run --release --example roundtrip -- input.f32 output.f32 24000 20
```

To hear your microphone through Snacka in real time, put on headphones and run
the optional Ratatui listening lab:

```sh
cargo run --release --features live-audio --example monitor
```

Press **Space** to compare codec and raw audio, **Left/Right** to change bitrate,
**Up/Down** for volume, **F** for 10/20 ms frames, **H** for experimental harmonic
speech, **B** for buffering, and **Q** to
quit. The dashboard shows microphone activity, device routing and stream health.
It starts at 24 kb/s and 50% playback gain; no audio is saved. See
[the listening guide](docs/monitor.md) for devices, Linux build prerequisites and
latency details. Audio/TUI dependencies are enabled only by `live-audio`.

For a two-person internet call with a live VoIP dashboard, AEC3 echo cancellation,
noise suppression, automatic gain control and encrypted UDP, build the optional
calling example:

```sh
cargo build --locked --release --features internet-call --example call
target/release/examples/call --generate-key
# Set the printed key as SNACKA_CALL_KEY on both computers, then on client 1:
target/release/examples/call --peer '[PEER_IPV6]:5000' --role 1
# Client 2 uses client 1's address and --role 2.
```

Direct mode needs reachable IPv6 addresses and inbound UDP port 5000 on both
clients. It uses no server. A small optional relay supports networks where direct
UDP is impractical. The dashboard shows both voice meters, link health and packet
loss; M mutes, arrows adjust volume/bitrate, and A/N/G toggle processing. See
[the call guide](docs/call.md) for complete two-client commands, firewall setup,
relay mode and a device-free IPv6 smoke test.

Rust 1.99 or newer is required. Plans allocate during construction, before the
audio callback. Snacka operates on PCM and packet bytes; capture, playback,
echo cancellation, resampling, encryption, jitter buffering and relaying remain
application responsibilities, implemented by the optional call example. A relay
can forward bytes and send separate copies
to an analysis pipeline without delaying participant audio.

```sh
cargo test --locked
cargo test --locked --release
cargo clippy --locked --all-targets -- -D warnings
cargo bench --locked --bench kernels
```

For an optional CPU comparison against a separately installed libopus:

```sh
cargo bench --locked --bench kernels --features reference-tests
```

This benchmark alone contains reference FFI. Supply a nonstandard search path
with `RUSTFLAGS="-L /path/to/lib"`. Neither the feature nor normal builds add
reference code or linkage to the library.

The optional [speech-quality reference](docs/reference-quality.md) compares
historical version 5 with restricted-lowdelay CBR libopus 1.6.1 through an offline
Python tool. It records quality gaps alongside wins at equal complete codec-packet
byte budgets, 10/20 ms frames and 120-sample additional delay. Payload budgets,
ordering/integrity services and end-to-end latency are not matched.

See [the bitstream](docs/bitstream.md), [development and acceptance](docs/implementation.md),
[measurements](docs/measurements.md), [signed pulse classes](docs/pulse-classes.md), [harmonic speech experiment](docs/harmonic-speech.md), [joint harmonic fit](docs/harmonic-joint-fit.md), [Swedish recording diagnostics](docs/swedish-diagnostic.md), [harmonic analysis performance](docs/harmonic-analysis.md), [pitch scoring performance](docs/harmonic-pitch.md), [packed MDCT performance](docs/mdct-packed-plan.md), [paired speech CPU probes](docs/cpu.md), [quality evaluation](docs/quality.md), [packet-loss evaluation](docs/loss.md), [fixture provenance](tests/fixtures/README.md)
and [fuzzing](fuzz/README.md). The implementation license is [MIT](LICENSE).
External reference source and trained weights are not vendored.
