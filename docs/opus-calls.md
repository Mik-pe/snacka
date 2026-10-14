# Opus packets in calls

`snacka::opus` produces and consumes ordinary Opus packets. A mobile peer that
already handles those packets can decode Snacka's output through its existing
Opus codec. The application must agree on packet transport, sample timing,
channel output and call setup. The crate exposes Rust APIs; it does not supply
Swift/Kotlin bindings, audio-device routing or a WebRTC stack.

An RTP payload contains one complete Opus packet, including its TOC byte. An
Ogg page, `OpusHead`, the native Snacka version-7 header and an application frame
counter do not belong in that payload. RTP uses a 48 kHz timestamp clock for
every Opus audio bandwidth and output sample rate; one 20 ms packet advances it
by 960. Payload types are negotiated dynamically. See
[RFC 7587 §4](https://datatracker.ietf.org/doc/html/rfc7587#section-4).

The existing `call`/`call_relay` examples use the experimental native codec.
Their `SNC1` envelope carries XChaCha20-Poly1305 authenticated room/session/role
messages. Audio contains an eight-byte frame index followed by a native packet;
the jitter buffer checks native profiles and a modulo-256 codec sequence. The
relay limits datagrams to 512 bytes. This protocol needs a matching application
peer. Enabling `opus` on the crate does not change these examples into RTP calls.

The new `opus_call` example exchanges generated voiced PCM over plain UDP/RTP.
Its sender uses the ordinary encoder's voice mode at 48 kHz with wideband,
fixed 20 ms packets. Its receiver
accepts standard SILK, Hybrid or CELT packets for the same negotiated duration.
It has a bounded 60 ms prebuffer, sequence/timestamp wrap, duplicate rejection,
one-packet FEC lookahead and PLC. Audio output is interleaved little-endian f32.
It uses a fixed test SSRC, DTX disabled by default and one known peer socket; production sessions
need their own SSRC/session lifecycle, clock adaptation and congestion control.

```sh
cargo build --release --features opus --example opus_call

# Start the receiver first; output remains outside Git.
target/release/examples/opus_call recv \
  --bind 127.0.0.1:5002 --peer 127.0.0.1:5000 \
  --frames 100 --channels 1 --output /tmp/received-opus.f32

target/release/examples/opus_call send \
  --bind 127.0.0.1:5000 --peer 127.0.0.1:5002 \
  --frames 100 --channels 1 --bitrate 32000 --drop-every 10
```

Use `--fec true` on the sender to carry redundancy for the previous packet.
Use `--dtx true --quiet-first 50` to exercise one second of quiet input followed
by generated voice. The example sends the valid minimal DTX packets at each
20 ms tick. Its status reports actual redundancy/DTX packet counts and the
owned encoder's algorithmic delay in 48 kHz samples.

A matching RTP session describes the dynamic payload type with
`a=rtpmap:111 opus/48000/2`, even when individual packets encode mono. Both
ends must use their negotiated payload type; 111 is only this example's default.
Use `a=ptime:20` for its fixed packet interval. Advertising
`a=fmtp:111 useinbandfec=1` tells the sender that the receiver can use FEC; it
does not guarantee redundancy in every packet. The sender's voice mode, bitrate,
loss estimate and signal determine which packets contain it. See
[RFC 7587 §7](https://datatracker.ietf.org/doc/html/rfc7587#section-7).

When packet N is missing, retain N+1 until the playout deadline for N. Call
`decoder.recover(next_packet, missing_samples, lost_output)`, then later
`decoder.decode(next_packet, current_output)` on that same decoder. `Recovery::Fec`
means redundancy was used; `Recovery::Concealed` means PLC supplied the interval.
Calling ordinary decode first consumes the current packet before the recovery
step. Redundancy is disabled by default on the example sender. Both the owned
sender with `--fec true` and a standard Opus voice peer can exercise this path. See
[RFC 7587 §3.3](https://datatracker.ietf.org/doc/html/rfc7587#section-3.3).

Codec interoperability covers encoded payloads and decoded PCM. RTP covers
sequencing and sample timestamps; SDP covers session parameters. An application
jitter buffer chooses when to reorder, recover or conceal. SRTP, ICE, DTLS,
RTCP and WebRTC call setup belong to the transport/application integration. A
mobile application must also exercise capture/playback formats, route changes,
headsets, backgrounding and audio interruption recovery on actual devices.

For a separately installed libopus peer, the repository-only reference tester
exercises both directions through actual loopback UDP sockets:

```sh
python3 tools/reference/check_opus_rtp.py /path/to/libopus \
  --example target/release/examples/opus_call \
  --output-dir /tmp/snacka-opus-rtp-peer
```

Repeat with `--channels 2` for the stereo journey.

The tester checks RTP fields, packet decode, final entropy ranges, reordered and
duplicate datagrams, real missing packets recovered from SILK FEC in both
directions, and quiet-to-voice DTX recovery. It writes raw PCM and a summary,
including an aligned generated-signal SNR regression check. This is a generated-signal peer simulation;
microphones, speakers, physical mobiles and a live WebRTC call are separate
acceptance checks. The tester alone loads the external codec. The example and
the `opus` library feature add no codec or WebRTC dependency.
