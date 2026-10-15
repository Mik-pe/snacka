# Implicit short-frame history compatibility

Source: `43c90f762aa2eed8a203621d01132cd7a73a95ae`, based on public
`2754f689fae64c2b26dab004039b0d693f7ea4a0`.

A fresh ordinary decoder seeded its previous mode, bandwidth and coded-channel
history from an Opus frame containing zero or one compressed byte. Its immediate
PCM matched explicit packet-loss concealment, but a later inter-coded Hybrid or
CELT frame could differ. The public libopus 1.6.1 API produced identical PCM for
implicit short frames and explicit PLC, including continuation.

The fix deletes only that five-line initialization. Each short frame still calls
`conceal_frame` with its existing duration, final range zero and existing output
path. A real coded frame or recovered FEC continues to establish codec history.
Primed decoders retain their existing owner. A mixed packet still processes each
short and coded frame in order. Gain, resampling, framing and normal coded-frame
metadata are unchanged.

## Checks

- The public/native regression covers 360 combinations: Hybrid and inter-coded
  CELT continuations; all five output rates; one/two output channels;
  fresh/primed/reset histories; zero-byte and one-byte `00`/`ff` frames; separate
  packets and a code-2 VBR short frame followed by a coded frame in the same
  packet. It verifies sample counts, final ranges, finite PCM, gain zero and an
  untouched destination suffix. PCM identity is required within each codec's
  implicit/explicit pair; this is not an owned/native waveform identity claim.
- The same final test fails against the public-main-equivalent baseline library
  with `decoder histories differ at gain 0` (exit 101), then passes with the fix.
- The release/reference suite passed 384 checks across 18 suites, with no
  failures or ignored tests. All 16 public/native reference tests, strict release
  all-target Clippy, fmt and diff checks pass on the final source. Root then
  independently verified the integrated commit
  `a6a9c23835ec2f0d10fae7f1422890096cb309ca`: 384/18/0/0, strict release all-target
  Clippy and fmt. Its Cargo/src/test source is identical to this source commit.
- An independent deletion-only replay passed 35,840 all-TOC and 12,000 targeted
  records across all five rates. It repaired 1,680 and 2,728 continuation
  controls respectively; all 17,920 primed matrix records remained unchanged.
- The existing 142 streams at five output rates gave 710 exact before/after
  decode-schedule JSON identities: PCM hashes, native diagnostics, counts and
  final ranges. Both variants processed 16,915 coded and 125 implicit-PLC packet
  operations. All 1,420 subprocess outputs were retained, with no failed or
  filtered cases. This proof excludes the other loss/FEC schedules.

No timing, new encoding, quality scoring or tuning was performed. The 960
regenerated diagnostic V1 rows remained unchanged; their quality acceptance stays
**INCONCLUSIVE**. This closes the observed history bug and makes no full-parity
or performance claim.

## Reproduction and pins

The six witness packets are retained as original `.bit` records in
`tests/fixtures/opus-short-frame-continuation.bin`; the adjacent JSON pins the
existing Hybrid/CELT streams and packet indexes. No new corpus is needed for the
committed regression. On macOS with the public native library available:

```sh
CARGO_ENCODED_RUSTFLAGS=-L/opt/homebrew/lib cargo test --locked --release \
  --features opus,reference-tests --test opus_calls_reference \
  implicit_short_frames_preserve_explicit_plc_and_coded_continuation
```

The unchanged `tools/reference/benchmark_opus_decode.rs` has bridge SHA256
`c6adb009e8e889adf358385d1eeb9c69f557289c60d068acb5bae8b2159f7aeb`.
For each retained stream and output rate, both frozen binaries used:

```sh
<binary> matrix <8000|12000|16000|24000|48000> <1|2> decode 0 200000 <stream.bit>
```

The zero batch argument bypasses all timers, warmup and calibration. Both builds
used Rust 1.99, ordinary release optimization, CGU 16, default target CPU, LTO off,
overflow checks on, and wrappers/incremental compilation off. The reused baseline
build source `6c4f916f9f2357dae8b388810a5bb50855ca2e9d` was verified identical to
public main 2754 across every Cargo/src file.

`evidence.json` pins the source, fixtures, native library, binaries, exact commands
and external receipt hashes. Original diagnosis and causal traces remain under
`/tmp/snacka-opus-short-frames-{diagnosis,causal}-20261015`; the coded proof,
all raw before/after stdout/stderr, gates and expected baseline failure remain in
`/tmp/snacka-opus-short-frame-state-evidence-20261015`. These host-local raw files
are retained externally rather than duplicated in Git.
