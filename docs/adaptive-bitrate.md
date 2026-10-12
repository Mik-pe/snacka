# Adaptive bitrate

Snacka already accepts every integer codec-packet target from 12,000 to 32,000
bits/s. `Encoder::set_bitrate` preserves sequence, overlap and fractional-byte
history; packets describe their own coded bandwidth/model, so the decoder needs
no separate bitrate negotiation. Adaptation uses this existing path and does not
change the version-7 codec format or enable harmonic profiles.

The dependency-free `snacka::adaptive` module supplies an allocation-free
controller. The optional `call` example supplies encrypted receiver feedback,
local-clock RTT probes and integration with the encoder. Each sending direction
has its own controller. Automatic adaptation is enabled by default:

```sh
target/release/examples/call --peer PEER:5000 --role 1 \
  --bitrate 24000 --max-bitrate 32000
# For a fixed-rate listening comparison or an older calling client:
target/release/examples/call --peer PEER:5000 --role 1 \
  --bitrate 24000 --fixed-bitrate
```

`--bitrate` is the starting rate; `--max-bitrate` is the adaptive ceiling.
The floor is 12 kb/s. **R** switches between automatic and fixed rate.
**Left/Right** adjust the ceiling in automatic mode, or the actual bitrate in
fixed mode. Lowering the ceiling takes effect immediately; raising it retains
the recovery hold. A new peer/connection clears measurements and probe history.
The dashboard and plain status output show the codec target, estimated wire
rate, last measured RTT, recent remote playout loss and the decision reason.

## Measurements and decisions

Clients send a probe every 250 ms while a peer is connected. The peer replies
with cumulative scheduled-frame/concealment counts from its jitter buffer,
including packets that missed their playout deadline. A packet that arrives
out of order but before its deadline is not counted as lost. Local playback
queue gaps are not substituted for the remote receiver's observations.

Reports are authenticated by the existing XChaCha20-Poly1305 transport and replay
window. They identify the originating sender session. RTT requires a matching,
unconsumed locally issued probe less than two seconds old, without subtracting
remote timestamps. Reordered reports, decreasing counters, impossible interval
counts and unmatched/expired probes are ignored. Cumulative counters survive
lost reports; the first report establishes a baseline. Replies are bounded to
at most five per second even under an authenticated probe flood. Relay mode
forwards these control datagrams unchanged between the leased participants.
Probe identifiers keep increasing across peer reconnects, so a delayed report
cannot match a new probe even if the local transport session is still alive.

The calling transport retains its `SNC1` authenticated envelope. Probe kind 5
has a nine-byte body (version 1 and a big-endian u64 token). Feedback kind 6 has
a 49-byte body: version 1, the 16-byte recipient session, then four big-endian
u64 values (serial, echoed probe token, cumulative scheduled frames, cumulative
concealed frames). Current replies use the probe token as the serial. Unknown
body versions and incorrect lengths are rejected. This is a calling-example
extension, separate from the codec packet format.

The controller aggregates observations over at least one second and requires
at least 20 scheduled frames before making a loss-based decision:

| Observation | Decision |
| --- | --- |
| Loss at least 5% | Reduce estimated total wire rate by 15%, at most once/s |
| Loss at least 20% | Reduce estimated total wire rate by 30%, at most once/s |
| Smoothed RTT at least 40 ms above baseline for two windows | Reduce wire rate by 15% |
| Loss at most 2%, RTT excess below 20 ms for three seconds | Add 1 kb/s of codec rate per second, up to the ceiling |
| No fresh nonempty receiver interval for two seconds | Reduce wire rate by 15% per second, down to the floor |
| Small, empty or ambiguous observation window | Hold; no upward probe |

RTT uses a 3/4 previous + 1/4 new smoother and a minimum-delay baseline with
30-second observation epochs. Old propagation-delay minima age out in at most
two epochs, allowing a changed route to establish a new baseline. A worker
stall triggers one backoff on resumption, rather than replaying missed timer
events. Invalid reports and backwards time do not mutate the controller.

## Account for the full sending cost

Codec targets include Snacka's four framing/checksum bytes. They exclude the
calling transport's 46 authentication/header bytes, eight-byte audio index,
UDP/IP headers, feedback and keepalives. At 50 audio packets/s, transport
overhead is substantial. The call estimates per-direction sending cost as:

| Cost | IPv4 | IPv6 |
| --- | ---: | ---: |
| UDP/IP + crypto + audio index per audio packet | 82 bytes | 102 bytes |
| Audio transport overhead at 20 ms | 32,800 b/s | 40,800 b/s |
| Four probes and four reports/s plus one HELLO/s | 7,184 b/s | 8,624 b/s |
| Total at a 24 kb/s codec target | 63,984 b/s | 73,424 b/s |
| Total at the 12 kb/s floor | 51,984 b/s | 61,424 b/s |

These are steady-state estimates, excluding link-layer headers, tunnels,
handshake bursts and inbound traffic. A relay also has its own forwarding cost.
Applying a 15% reduction only to codec bytes would barely change total traffic;
the controller reduces the total estimate and then subtracts fixed overhead.
`packet_overhead_bps` supports either 10 or 20 ms framing for other integrations.

## Use the controller independently

The application owns a monotonic epoch, fresh authenticated interval feedback,
transport overhead and pacing. Call `tick` during outages and apply the current
target before encoding the next frame:

```rust
use snacka::{Encoder, Config};
use snacka::adaptive::{BitrateController, Feedback};
use std::time::Duration;

let mut encoder = Encoder::new(Config::new(24_000).unwrap());
let mut rate = BitrateController::new(
    24_000, 12_000, 32_000, 39_984, Duration::ZERO,
).unwrap();
rate.update(Duration::from_secs(1), Feedback {
    frames: 50, lost: 3, rtt: Some(Duration::from_millis(30)),
});
encoder.set_bitrate(rate.bitrate()).unwrap();
```

Interval counts must exclude duplicate reports and packets still awaiting their
reordering deadline. When a stream is intentionally idle, the application should
stop driving its sending controller or initialize a fresh controller when media
resumes. Empty reports cannot sustain an upward probe or hide a media outage.

## Verification and limits

`tests/adaptive.rs` covers capacity step-down/recovery with a deterministic fluid
queue, loss bursts, sustained delay, a delay spike, feedback outages, worker
stalls, bounds and invalid observations. It also feeds changing targets through
real encoders/decoders at both frame durations, checking exact accumulated byte
budgets, modulo-256 sequence and finite bounded output. Allocation tests cover
controller construction, feedback, ceiling changes and timeouts.

Call/relay tests cover probe/report framing, authentication, session binding,
replay rejection, restart and unchanged relay forwarding. `tools/smoke_call.py`
starts real device-free client processes over direct IPv4, direct IPv6 and an
IPv4 relay. It additionally drops 25% of one direction's audio, then restores
the link, and separately blocks returning feedback. Both scenarios require
backoff followed by recovery while the other sending direction stays healthy.
These checks run in CI using the existing smoke-test step.

This is an experimental bounded controller, not a validated replacement for
GCC, SCReAM or a complete production congestion-control stack. RTT includes the
reverse path and peer processing, and playout loss can include sender/device
stalls. It cannot identify every cause of degradation. At the codec floor,
fixed headers alone may exceed available capacity: adaptation cannot solve
that by lowering codec bitrate further. A production transport still needs
pacing, a circuit breaker/media suspension, clock-drift management and tests
with competing traffic and real bottleneck queues. Fixed mode deliberately
disables adaptive backoff. Older peers/relays without feedback cause automatic
mode to fall toward the floor; fixed mode supports measurement compatibility.

The design follows the feedback and per-direction principles in
[RFC 8085](https://www.rfc-editor.org/rfc/rfc8085.html), the delay/quality
requirements in [RFC 8836](https://www.rfc-editor.org/rfc/rfc8836.html), and the
evaluation guidance in [RFC 8868](https://www.rfc-editor.org/rfc/rfc8868.html).
The tests establish the behavior above, not TCP fairness or production fitness.
