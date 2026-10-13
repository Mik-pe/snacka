![Snacka audio codec banner](assets/snacka-banner.webp)

# snacka

Independently implemented Rust audio codecs: Snacka's experimental speech
bitstream and optional standard Opus support. Enable `opus` for the owned
SILK/Hybrid/CELT decoder, CELT encoder and Ogg Opus file handling. Both library
paths have zero dependencies, C, FFI, CMake or unsafe Rust. External codecs are
measurement competitors and test oracles only.

**The native Snacka format is an experimental DSP prototype.** It encodes and decodes audio today, but its
speech quality and loss concealment need substantial improvement before use in
production calls. No superiority over established codecs is claimed.

The following describes the native format. For the standard format's API,
supported modes, interoperability checks and CPU comparisons, see
[Opus support](docs/opus.md). To convert 48 kHz interleaved little-endian f32 PCM:

```sh
cargo run --release --features opus --example opus_roundtrip -- \
  input.f32 output.opus decoded.f32 32000 1 20
```

- Mono normalized f32 PCM at 48 kHz.
- 20 ms frames by default; optional 10 ms frames.
- 12–32 kb/s including four bytes of framing and checksum per packet.
- Bitrate changes retain stream history; fractional byte budgets remain exact.
- Allocation-free adaptive bitrate controller; the call example uses encrypted
  remote playout feedback and RTT, with transport overhead accounted for.
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
# Open the connection panel; choose IPv4/IPv6, address, role and shared key:
target/release/examples/call
# Or connect from the CLI (same SNACKA_CALL_KEY on both clients):
target/release/examples/call --ipv4 --peer PEER_IPV4:5000 --role 1
```

Direct mode supports IPv4 and IPv6 without a server. IPv4 over the internet needs
port forwarding on both sides; IPv6 needs reachable addresses and inbound UDP
allowed. A small optional relay supports networks where direct UDP is impractical.
Press C to choose or change the connection in the TUI, or hang up and reconnect.
The dashboard shows both voice meters, link health and packet loss; M mutes,
arrows adjust volume/bitrate ceiling, R toggles automatic bitrate, and A/N/G toggle processing. See
[the call guide](docs/call.md) for complete two-client commands, firewall setup,
relay mode and device-free IPv4/IPv6 smoke tests.
Calls start at 24 kb/s and adapt within 12–32 kb/s; `--max-bitrate` sets the
ceiling and `--fixed-bitrate` disables adaptation. See
[adaptive bitrate](docs/adaptive-bitrate.md) for the controller API, wire-rate
budgets, feedback protocol, tests and limits.

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
and [fuzzing](fuzz/README.md). Original implementation code is [MIT](LICENSE);
normative Opus format data retains its [BSD notices](LICENSE-OPUS-DATA).
External codec implementations and trained weights are not vendored.
