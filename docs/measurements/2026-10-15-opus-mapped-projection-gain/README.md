# Uniform mapped and projection decoder gain

Source `d664f519aa0d2e2885443460c83c353d9ff635b4`, based on public main
`4f68e4808e8a251368342b16d4edb72f23ee8e38`. Local macOS ARM64 validation uses
Rust 1.99 and the separately installed libopus 1.6.1 public API.

`MultistreamDecoder::set_gain_q8(i16)` forwards the setting to every private
ordinary decoder, including unmapped streams. `gain_q8()` reads the first
validated nonempty stream; no API exposes mutable child decoders, so their gain
remains uniform. `ProjectionDecoder` delegates both controls to its multistream
owner. Gain therefore applies once after each stream's resampling, before channel
mapping and demixing. Reset retains it. Floating PCM is not clipped, and Ogg
header gain remains a separate caller-owned playback choice.

The installed public headers permit ordinary decoder gain CTLs on multistream
and projection decoders, define the full signed16 Q8 range, default zero,
`10^(gain / 5120)` scaling and reset retention. They do not specify the aggregate
getter after deliberately heterogeneous native child controls or exact float
rounding around demix. The owned API maintains uniformity; the tests query every
native child's gain and compare nonzero scaling with tolerances.

## Validation

The final release/reference suite passes **386 tests across 18 suites**, with no
failures or ignored tests. Strict release all-target Clippy, fmt and diff checks
pass. The public gain subset passes all four tests, including the two new tests;
the extended processing allocation gate passes with **zero allocations**.
Root independently verified integrated head
`d1bea6901966a55d30bd72f549bb9b73735de6a0` with the same 386/18/0/0, strict release
all-target Clippy and fmt. The gain API/test files match this source commit.

The shared gain test runs 140 owner/family/rate/gain setups and 280 fresh/reset
trajectories. It covers all five output rates, `i16` endpoints, positive/negative
settings, coupled/mono streams, aliases, silent slots, an omitted stream, native
aggregate/child getters, suffix/refusal behavior, unclipped headroom and 140
bit-identical zero-gain continuations within each implementation. Of the 280
recovery trajectories, 140 require actual FEC and nonzero PLC-prefix/recovered-tail
PCM; 140 require CELT's absent-FEC fallback.

An independent f64 Q15 oracle checks gained coded-channel PCM against square and
rectangular demixing: 225 geometry/rate/gain/packet cases, including 75 square
native projection cases. Its error bound scales with absolute contributions to
handle cancellation. Native rectangular projection limits remain unchanged;
C=3/6 use owned coded PCM, and the public native projection API covers C=K=4.
These are functional API checks, with no owned/native waveform bit-identity or
exact native float-operation-order claim.

`processing-source-audit.json` verifies that every pre-existing constructor and
processing method body, both decoder field layouts, and the entire ordinary
`Decoder` source file are byte-identical to the base. Only controls and their
documentation were added. This establishes no new default-path source branch or
processing pass; it is not a CPU measurement. Nonzero gain cost is unmeasured.
No new quality corpus, scoring, tuning or performance-parity claim is involved.

## Reproduction and evidence

On macOS with native libopus available:

```sh
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_LTO=false \
CARGO_INCREMENTAL=0 RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
CARGO_ENCODED_RUSTFLAGS=-L/opt/homebrew/lib \
cargo test --offline --locked --release --features opus,reference-tests
```

The retained command/environment receipts pin default target CPU, ordinary
release optimization, CGU16, no LTO/incremental/wrappers and enabled overflow
checks. `checks.json` records exact counts, commands, public-header hashes and
limitations; `validation-pins.json` pins tools, source and native binary linkage.
The installed oracle library SHA256 is
`98dcd3cc62c6cf913b789051536ab246c56cc163c74bd8b4968ac467ec74b68b`.

Complete logs, the original source-only plan and first Clippy failure remain in
`/tmp/snacka-opus-mapped-decoder-gain-evidence-20261015`, with hashes in the
receipt. That initial failure concerned only SAFETY-comment placement in the
test wrapper and was corrected without production changes. No timings were run.
