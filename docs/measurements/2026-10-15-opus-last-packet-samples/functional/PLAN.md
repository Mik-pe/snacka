# Last successful ordinary decoder interval — source-only draft

Status: **NOT BUILT, FORMATTED OR EXECUTED**. Root's full-M5 window is active.
No native calls, codec generation, tests, scoring, timing or hash inventory now.

Worktree: `/tmp/snacka-opus-decoder-last-packet-samples-20261015`
Branch: `codex/opus-decoder-last-packet-samples-20261015`
Frozen public baseline: `dccf36b564afb819e5f206f24ec8a2aa34121ef6`.
Cargo files and existing regression-witness bytes are untouched. Exact source,
Cargo, tool, native and fixture manifests will be bound after the quiet release.

## Owned draft contract

`Decoder::last_packet_samples() -> usize` retains samples/channel in the fixed
configured output rate. Zero at construction/reset. Successful ordinary decode
commits the whole packet total; successful conceal commits its requested duration;
successful recovery commits the entire requested prefix-plus-tail interval.
Gain/phase changes do not alter this status. Every Err retains the prior status.
Content errors may still advance audio history/write a prefix and require reset.

This is one ordinary-Decoder metadata owner, with no mapped/projection getter or
duplicate aggregate field. Public/internal parsed entrypoints commit only after
success. Private `conceal_unrecorded`/`recover_unrecorded` contain the former
processing bodies; all recovery prefix/fallback conceal calls are unrecorded.
No audio arithmetic, entropy, resampling, PLC loop or corpus is changed.

Fresh/reset zero and Err retention are explicitly provisional OWNED guarantees.
`opus_defines.h:802–805` specifies last successful decoded/concealed duration at
the output rate, but does not settle all constructor/error side effects. Native
gain-overflow behavior has no direct equivalent to the owned Corrupt rejection.
The postwindow public oracle may require an explicit contract decision before
final source/docs freeze.

## First postwindow gate: public native edge observations

Compile the adjacent `native_duration_oracle.rs` only after CPU release, using
direct Rust1.99 and installed public libopus1.6.1 with `-L native=/opt/homebrew/lib`.
Run it once against the already committed decoder-gain `functional` witness
directory (silk/hybrid/celt/silk-fec, both coded channel counts). It creates no
new packets/corpus, uses no Snacka implementation and runs no timers.

The proposed observation set is 80 native instances (8 existing streams × 5
rates × 2 output channels), 12 codec calls plus fresh/reset observations each:
expected 1,120 event rows plus one version row. It records raw return codes and
before/after duration without asserting unknown error-side behavior:

- fresh, coded20ms and ordinary following packet;
- PLC7.5ms/40ms;
- FEC request40ms (known SILK-FEC witness prefix+tail, absent-FEC controls);
- 48 implicit2.5ms frames totaling120ms, separate from FEC requested duration;
- malformed framing, too-small advertised normal capacity, invalid PLC duration;
- reset, fresh implicit Hybrid10ms and following coded20ms.

Negative native calls retain a physically allocated full120ms output buffer and
reduce only the advertised capacity. There is no unsafe undersized FFI buffer.
Retain all records/errors and bind source, witness, library and binary hashes.

## Functional and ownership gates after observing the oracle

- Existing `GainDecoders` source now checks owned/native CTL4039 after every
  ordinary decode, PLC and recovery. This reuses actual FEC prefix/tail, coded
  multi-frame120ms, gain/reset/history, all5rates and both output channels.
- Existing360-case short-frame regression now checks full implicit/mixed packet
  duration separately from the explicit PLC-plus-coded reference's final call.
- New `last_packet_samples_tracks_whole_packets_plc_and_refusals` reuses the same
  frozen Hybrid/CELT packet fixture, adds7.5ms PLC, whole100ms packet, overflow
  arguments, reset/control changes and refusal status/output preservation.
- New owned unit injects a nonfinite cached gain factor to force late Corrupt
  through all three public paths; this checks metadata atomicity and is expressly
  not native overflow evidence. The existing unit separately covers actual
  finite-gain product overflow at the common output boundary.
- Existing allocation gate reads status under its zero-allocation counter.
- Format, targeted contracts, full release opus/reference-tests, strict release
  all-target Clippy and fmt/diff checks. Retain every initial failure.

Baseline/candidate codegen and processing fingerprints plus bitexact witness
replay are required because a field and private helper entrypoints are added.
Reuse existing complete codec bridges/streams and clean Rust1.99 CGU16/defaultCPU,
overflow-on production settings, with wrappers/incremental off. Record actual
Cargo/local-LTO semantics from the public-dccf profile; do not silently change
release flags. A small selected macOS CPU comparison needs a separately frozen
protocol and root's coordinated timing authorization. No cost or parity claim
exists from this source-only draft.
