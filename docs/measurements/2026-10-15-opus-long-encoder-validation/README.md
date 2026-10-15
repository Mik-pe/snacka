# Untimed long-packet encoder validation

This tooling-only follow-up starts from main
`924039cb531b6034a8b352474d3601db42f6b223`. Production codec files, Cargo
configuration and all previous measurements remain unchanged. The source,
benchmark binary, installed reference and commands are identified in
[`native-validation.json`](native-validation.json).

The earlier [SILK encoder timing result](../2026-10-15-silk-encoder-gain/README.md)
retains its original exclusion of durations above 60 ms and the selected 60 ms
case. Its reference wrapper asserted one of six sample counts, at most 2880,
**before calling any native API**. The peer decoder had the same guard. That
was a tester restriction, not a native libopus rejection. No previous timing
is replaced, reinterpreted as a 120 ms result or rerun here.

Both wrappers now share one explicit standard-duration check: 120, 240, 480,
960, 1920, 2880, 3840, 4800 or 5760 samples/channel at 48 kHz. Unsupported
intermediate durations remain rejected. Input, packet and decoded PCM buffers
already derive their capacities from the requested duration.

`--validate-only` requires encoder selection and `reference-tests`. It takes a
separate path before the normal benchmark setup and does not call timers,
warmups, calibration or measurement batches. Normal benchmark validation and
measurement behavior remain unchanged apart from accepting the three longer
durations. The untimed path checks:

- Each encoder's exact CBR packet size, wire mode, bandwidth, coded channels and
  aggregate sample duration.
- Each encoder's packets in separate owned and native decoder histories.
- All four decoded sample counts and entropy ranges against the corresponding
  encoder, plus finite PCM.

The pinned macOS arm64 libopus 1.6.1 build passed all 18 profiles: 80/100/120 ms,
mono/stereo, and SILK WB at 24 kb/s/channel, Hybrid FB at 32 kb/s/channel and
CELT FB at 64 kb/s/channel. Eight changing voiced intervals per profile produce
288 encoded packets and 576 successful decode/count/range checks. The manifest
retains every attempted profile's command, return code, stdout and stderr;
there were zero failures. This exercises generated voiced input at 48 kHz only
and establishes neither perceptual quality nor performance parity.

No additional native duration control was required. The public
[encoder CTL contract](https://opus-codec.org/docs/opus_api-1.6/group__opus__encoderctls.html)
specifies `OPUS_FRAMESIZE_ARG` as the default and lists explicit 80/100/120 ms
choices. Longer packets contain multiple codec frames, within the
[RFC 6716 packet-duration limit](https://www.rfc-editor.org/rfc/rfc6716.html#section-3.2.5).

For an untimed targeted check, run:

```sh
RUSTFLAGS="-L /opt/homebrew/lib" cargo bench --locked --bench opus \
  --features opus,reference-tests -- \
  --validate-only --encoder-case hybrid FB 120 2 64000 --frames 8
```

The `cargo bench` command invokes the binary, but `--validate-only` exits through
the untimed branch. For a full future acceptance matrix, retain unsuccessful
mode selections and other refusals as failures; do not silently replace cases
or infer timing results from this validation. The nine-duration guard has a
separate test that rejects all unsupported sample counts through 6000 and
`usize::MAX`.
