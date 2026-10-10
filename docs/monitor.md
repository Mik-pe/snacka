# Live listening lab

The `monitor` example captures your microphone, converts it to mono 48 kHz PCM,
encodes real Snacka packets, decodes them, and plays the result to your selected
output. Its Ratatui dashboard lets you explore speech quality while talking.

## Start listening

Connect headphones, choose your microphone and headphones as the system's default
devices, and start with low headphone volume:

```sh
cargo run --locked --release --features live-audio --example monitor
```

The default settings are 24 kb/s, 20 ms frames, 40 ms playback prebuffer and 50%
playback gain. Speak normally, then use Space to compare raw and encoded audio.
Try 12, 24 and 32 kb/s on the same phrases. The activity trace shows ten seconds
of microphone peak levels; it is a level history, not a frequency spectrum.
Audio stays in memory and is never written to disk or sent over the network.

| Key | Action |
| --- | --- |
| Space | Toggle raw reference / Snacka encoder and decoder |
| Left / Right, or - / + | Change bitrate by 1 kb/s, within 12–32 kb/s |
| Up / Down | Change playback gain by 5%, within 0–200% |
| F | Switch 10 / 20 ms frames |
| B | Cycle 20 / 40 / 80 / 120 / 200 ms playback prebuffer |
| M | Mute playback; microphone capture continues |
| ? | Open / close the listening guide |
| Esc | Close help, or quit if help is closed |
| Q / Ctrl-C | Quit |

The layout adapts to terminal size; smaller terminals use a compact view. Quit,
startup errors, runtime errors and Rust panics restore the terminal.

## Reading the numbers

**48 kHz** describes the audio entering the codec: 48,000 mono samples each
second. **kb/s** describes the compressed data budget, in thousands of bits per
second. A lower bitrate saves data but gives the codec less room to describe your
voice. They are different units; 48 kHz does not mean 48 kb/s.

| Audio | Data per second | Data per minute |
| --- | --- | --- |
| Uncompressed 48 kHz mono, internal 32-bit float | 192 kB | 11.52 MB |
| Snacka at 12 kb/s | 1.5 kB | 90 kB |
| Snacka at 24 kb/s | 3 kB | 180 kB |
| Snacka at 32 kb/s | 4 kB | 240 kB |

These are decimal units: 1 kB is 1,000 bytes and a byte contains eight bits.
The uncompressed row describes the monitor's internal audio representation;
a selected device can use a different sample rate and sample format.

The dashboard also shows **kB/s avg**: actual generated packet bytes divided by
the duration of processed audio since startup. It includes Snacka's packet
headers and excludes network and encryption overhead. After a bitrate change it
averages both old and new packets, so it can differ from the current kb/s target.
Capture stalls do not dilute this average. The monitor keeps encoding in raw
mode too, so the number remains comparable during A/B switches.

12 kb/s is an experimental low-data setting. Listen for electronic tones, rough
s-sounds and changes to your voice, then compare the same phrase at 24 and
32 kb/s. Clear words alone do not establish natural speech quality. If raw audio
is also distorted, inspect the input device, microphone clipping and stream
health counters. Both A/B modes share capture processing and device sample-rate
conversion. Different codecs can produce different quality at the same bitrate;
see the [measured reference comparisons](reference-quality.md) for this codec's
current limitations.

## Devices and startup options

List device indices, then select an input and output explicitly:

```sh
cargo run --locked --release --features live-audio --example monitor -- --list-devices
cargo run --locked --release --features live-audio --example monitor -- --input 1 --output 2
```

Input and output indices are separate lists; indices may change when devices are
connected or disconnected. Indices 1 and 2 above are examples. On Linux, the
PipeWire/default ALSA device follows the system's routing; a hardware device may
bypass it. The monitor uses each device's default PCM format to avoid changing
its hardware clock. If that format is unavailable, it selects a supported PCM
rate closest to 48 kHz. Capture and playback are resampled independently using a
band-limited sinc filter when needed. This supports lower-rate Bluetooth headset
microphones and 44.1 kHz devices. The device list and live routing display include the selected
sample rates; Snacka always receives and produces 48 kHz mono PCM.

A headset microphone may have limited bandwidth, and enabling Bluetooth capture
can change the headset's playback mode. Resampling cannot restore missing high
frequencies. For a wider-band comparison, use the computer's microphone with the
headset as output, selecting the indices printed by `--list-devices`.

```sh
cargo run --locked --release --features live-audio --example monitor -- \
  --bitrate 16000 --frame-ms 10 --buffer-ms 80 --gain 0.3
```

`--raw` starts in reference mode. `--seconds 30` ends a session automatically.
`--no-tui` disables terminal controls, useful for a timed smoke test or redirected
output. `--help` lists all options without opening audio devices.

### Build prerequisites

Use Rust 1.99 or newer. The optional feature enables CPAL, Crossterm, Ratatui,
the lock-free `rtrb` queues and Rubato resampling. Default builds of the codec have
no dependencies.
Linux needs ALSA development headers and pkg-config, for example:

```sh
# Debian / Ubuntu
sudo apt-get install libasound2-dev pkg-config

# Arch Linux
sudo pacman -S alsa-lib pkgconf
```

On macOS, allow microphone access for the terminal/application when prompted.
On Windows, enable microphone access in the operating system's privacy settings.
Desktop CI compiles and tests the feature on Linux, macOS and Windows; these
checks do not replace listening tests on each platform's hardware.

## Understanding the comparison

The encoder and decoder stay active in reference mode, retaining stream history
through A/B switches. The raw reference is delayed by the same 120 samples
(2.5 ms) as Snacka's transform. Both paths share frame buffering, output gain and
the playback queue. Changing bitrate retains codec history. Changing frame
duration restarts both encoder and decoder because it changes the stream profile;
this can produce a brief discontinuity. Queued audio plays before changes become
audible.

The displayed queue duration is application buffering, **not measured end-to-end
latency**. Frame buffering, transform delay, capture/playback device buffers and
headphone transport contribute additional delay. Non-48 kHz devices also add
resampling filter delay and up to 10 ms of resampling block buffering on each
converted path. Both A/B modes share these conversions. Bluetooth can add
substantial delay. Larger prebuffers can reduce gaps, at the cost of slower monitoring.
Changing the prebuffer briefly re-primes playback, discarding stale queued samples
when reducing the target. Deliberate discards are included in the drop counter.

Steady-state audio callbacks convert samples, resample when needed using
preallocated buffers, and access bounded lock-free queues; encoding, decoding,
terminal rendering and key handling run on the main thread. Input channels are
averaged to mono; decoded mono is duplicated to each
output channel. Non-finite input becomes silence, input is constrained to the
codec's normalized range, and playback is limited to [-1, 1]. Playback gain above
100% can clip the signal. Capture drops, playback gaps, output drops and clipped
microphone samples are shown cumulatively, in samples. Queue drops and playback
gaps count 48 kHz codec samples; clipped input counts device-rate input frames.

If the stream stutters, increase buffering with B, use a release build and check
the device routing. If the microphone level stays at zero, verify microphone
permissions, mute settings and the selected capture device. Fatal stream errors
stop the example; recoverable device underruns/routing notices appear in the
status line. The example does not reconnect a disconnected device automatically.

## Noise and echo processing

Snacka compresses audio; it does not implement noise suppression, acoustic echo
cancellation or automatic gain control. A calling application can apply those
effects to captured PCM before encoding. Echo cancellation additionally uses the
audio sent to the speakers as a reference, as illustrated by
[WebRTC's audio processing API](https://webrtc.googlesource.com/src/+/refs/heads/main/api/audio/audio_processing.h).

The monitor adds none of these effects. "Raw" means bypassing Snacka; firmware,
driver or operating-system processing may still be present in the captured
signal. The example does not request or guarantee a hardware raw-capture mode.
Use headphones: feeding your microphone back through speakers can create
acoustic feedback, and this example has no echo canceller.

## Developer checks

```sh
cargo clippy --locked --all-targets --features live-audio -- -D warnings
cargo test --locked --features live-audio --example monitor
```

Tests exercise codec/reference alignment, bitrate updates across A/B switches,
frame-profile restarts, device format selection, resampling duration and pitch,
downsampling alias suppression, playback re-priming, allocation-free conversion
and resets, and wide/compact dashboard rendering without opening microphone or
speaker devices.
