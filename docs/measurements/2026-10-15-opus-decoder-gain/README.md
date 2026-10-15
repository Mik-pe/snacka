# Ordinary Opus decoder gain, 2026-10-15

`Decoder::set_gain_q8(i16)` and `gain_q8()` close the ordinary decoder output-gain
control gap. The signed value uses 256 units per dB, defaults to zero, and
survives reset. Its cached factor is `10^(gain / (20 * 256))`, as specified by
the [public Opus decoder gain API](https://opus-codec.org/docs/opus_api-1.6/group__opus__decoderctls.html).
Output gain applies exactly once after final resampling to ordinary decoding,
PLC and FEC, including an earlier concealed prefix. It does not change codec or
filter history, entropy intervals, or the caller's unwritten output suffix.

Zero gain skips the gain loop. Nonzero gain preserves floating headroom above
±1. A non-finite product returns `DecodeError::Corrupt` without clamping; as
with other content errors, history can advance and an output prefix can be
written, so reset before continuing. Mapped/projection decoder controls and
caller-owned Ogg header gain are unchanged. Integer PCM soft clipping and
encoder input-precision/extended-range behavior remain separate parity gaps.

## Source binding and functional checks

The implementation and tests are source commit
`a939e81a71679919bd7e2e5ee4d28b831611b2ac`, based on
`d2e420edd388bfd7e8925401da012ed238fe0bf8`. The normal merge
`6c4f916f9f2357dae8b388810a5bb50855ca2e9d` then incorporates main
`d5d3971b7b75137df1d1150fb5df9c4ad7062d5e`. No production change follows that
merge in this evidence commit. Relative to D5, only `src/opus/decoder.rs`
differs in the production source tree.

On Apple M5/macOS 27 with Rust 1.99 and public libopus 1.6.1, the source-`a939e81`
release suite passes **371 unit/integration tests and six doctests**. This
includes all 15 public native-call tests and all five allocation tests. Strict
all-target release Clippy, formatting and diff checks pass. The commands,
source hash, log hashes and exact counts are in [functional/checks.json](functional/checks.json).
These checks precede the D5 merge; the cost-pair proof below separately binds
the merged decoder.

The separate integrated local checkpoint on exact merge `6c4f916` passes
**383 release/reference checks across 18 suites, zero failures and zero
ignored**, plus strict all-target release Clippy and formatting. The
[integrated receipt](functional/macos-gain-integrated-tests.json), complete logs
and [their hashes](functional/integrated-check-files.json) are retained. This is
local validation. D5's remote CI was separately green; remote CI for the new
gain head had not run when this evidence was prepared.

The generated gain tests maintain four synchronized histories: owned/native,
each with zero/gained output. They cover all three modes, both coded and output
channel counts, all five output rates, gain endpoints, ±12 dB, ±1 Q8 unit,
120 ms repacketized output, PLC, missing-FEC fallback, reset retention, refused
capacities/intervals and untouched suffixes. Actual native SILK LBRR fills the
tail of a 40 ms recovery after a nonzero 20 ms PLC prefix; both segments contain
nonzero PCM. Returning gain to zero requires bit-identical continuing PCM
within each implementation pair. The allocator test includes gain changes,
decoding, reset, concealment and recovery without processing allocations. A
direct output-boundary unit reaches the finite-overflow rejection branch.

Nonzero gain is compared to each implementation's own zero-gain PCM multiplied
by an independent `f64` formula, with 8 ppm relative float tolerance. This checks
the public formula without requiring identical libm approximations or equating
the implementations' existing SILK resamplers. It does not claim bit-exact
nonzero native PCM or native integer soft-clipping behavior. Nonzero-gain CPU
cost was **not measured**.

[functional/source-and-results.json](functional/source-and-results.json) binds
a frozen D2-before/`a939e81`-after comparison. Eight generated public native
streams cover SILK/Hybrid/CELT, coded mono/stereo and SILK FEC. Across five
output rates and both output channel counts, all **80 rows** match exactly in
PCM-bit hash, entropy/sample-count hash and FEC status, with suffix sentinels
checked. The driver includes ordinary packets, 2.5/20 ms PLC, 40 ms recovery,
120 ms repacketization and reset. Both result files, eight packet inputs,
untimed driver, source hashes and logs are retained under `functional/`.

## Default-zero macOS cost

The [frozen cost record](cpu/README.md) compares D5 with merge `6c4f916` using
default-zero ordinary decoders and the unchanged common decoder benchmark
bridge. Seven existing generated matrix workloads run once as three ABBA/BAAB
quartets each: 84 role processes and all 168,000 native/owned raw batches.
All PCM/count/range/FEC checks and the final integrity audit pass.

Normalized p50 median changes range from -0.465% to +0.220%, including small
positive SILK costs. Individual and median tail regressions are retained,
including Hybrid FEC-return p99 +26.506% in one quartet and +1.173% median,
and WB/20 ms mono 48 kHz p99 +17.204% in one quartet. WB/20 ms mono 8 kHz has
direct p99 median +6.178% versus native-normalized -1.637%. These observations
characterize the default-zero API cost; they establish neither a general
speedup, absence of tail regressions, full libopus speed parity nor nonzero-gain
performance. No quality corpus, retuning, variant, discarded run or rerun was
used.

The original plans/manifests retain their historical paths and preparation
status. Completed runtime metadata is in `cpu/acceptance/manifest.json`.
[evidence-files.json](evidence-files.json) hashes every retained file. Production
libraries and binaries are identified by source/build/tool hashes rather than
checked in. The selected six existing `.bit` inputs are copied byte-for-byte to
`cpu/inputs/`; their original hashes and paths remain in `cpu/source.json`.

To reproduce, recreate the pinned source worktree and input paths documented by
the retained preparation scripts, copy those scripts to a fresh artifact
directory, and inspect their commands before executing. `functional/run.py`
performs only generated untimed verification. `cpu/prepare.py` builds and runs
seven zero-batch matrix validations without warmup/calibration/timers. The
separate CPU runner requires an explicitly coordinated quiet window; its
argument is an execution gate, not evidence of a new authorization.
