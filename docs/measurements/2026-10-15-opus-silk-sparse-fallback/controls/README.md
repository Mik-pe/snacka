Functional proof for isolated sparse fallback commit
`8f723ba1f35c053e1cc840f38a4b20446a127dde`, compared with main `cca87dd`.
No production source edits, timing or perceptual scoring occurred during these probes.
`source-hashes.json` pins the two production files, every external probe/script
and both library archives. `reproduce.sh` contains the complete build/run and
postprocessing commands; it has been written for reproducibility and was not
rerun during the subsequent CPU hold.

- `comparison.json`: all 22 successful control streams, 354 packets/layers,
  exactly match baseline bytes, final ranges, FEC/transition flags and decoded
  PCM bits. Includes ordinary WB 12 kbit/s mono 20 ms plus native input rates,
  stereo, grouped durations, Hybrid, FEC changes, reset/reconfigure and raw SILK
  layers. Libopus 1.6.1 decoded all with expected samples and exact ranges.
- `fallback-comparison.json`: 56 predetermined full-scale synthetic profiles
  plus one retained-FEC carrier. NB/WB, 48 kHz/matching native input, mono/stereo,
  10/20/40/60/80/100/120 ms, 12 kbit/s per coded channel. Baseline refuses every
  stream; candidate completes all 462 packets. Every emitted packet has active
  VAD flags and native exact ranges/finite PCM. All tracked encode/decode/recover
  calls allocate zero, and the entire matrix runs in a 1 MiB thread.
- The FEC carrier is NB 10 ms mono, 20 kbit/s, FEC enabled: six 0.03-amplitude
  tone packets warm history/LBRR, packet 6 uses full-scale clipping, then seven
  tone packets follow. Baseline's six successful packets are exactly preserved.
  Candidate clipping packet retains actual LBRR flags; owned and native FEC
  recovery succeeds for omitted packet 5, then current/following ranges match.
- `composition-comparison.json`: four mapped/projection profiles at NB 10 ms,
  12/16 kbit/s per mono stream, full-scale independent channels. Baseline refuses
  frame zero; candidate emits 16 packets each. Full transport bytes, individual
  stream/XOR ranges and PCM bits match independently gathered or signed/permuted
  mixed PCM encoded by ordinary stream encoders. Aliases, silent mapping slots
  and midstream reset are covered. Processing/reset/composition allocates zero,
  and construction/processing runs in a 1 MiB thread.
  `composition-candidate-data/native-validation.json` confirms all 64 native
  multistream sample counts, finite PCM and exact ranges. Projection native
  decoding validates its coded transport; the owned PCM oracle checks demixing.
- `late-output.json`: a caller begins writing an entropy prefix into a tiny
  destination, then attempts a sparse layer. Terminal `Output` propagates and
  finish remains `Output`; there is no retry. After resetting the SILK encoder,
  the same sparse signal encodes and decodes with exact range. Zero processing
  allocations and a 1 MiB thread apply to this case too.

Comparison rules distinguish complete baseline-success streams from the exact
successful prefix before baseline refusal. No tails after a baseline error are
claimed equal. The two original refusal reproductions are owned by the parent
agent; this matrix does not rerun them by default. No useful-speech or perceptual
acceptance follows from functional recovery alone.
The parent separately reports 171 native-valid packets from the two original
refusal profiles, with maximum owned/native PCM differences approximately
NB 0.383 and WB 0.00342. Those results do not establish decoder or speech-quality
parity and are outside this probe's independent measurement scope.

Allocation hooks in `fallback.rs`, reused by `composition.rs` and
`late_output.rs`, count `alloc`, `alloc_zeroed` and `realloc` through Rust's
global allocator on the current thread. Only the measured public API closure
is enabled: encode, decode, recovery, reset and packet composition where used.
PCM preparation, vector/codec construction and file I/O happen outside those
closures. Constructor and processing execution occur inside a thread configured
with a 1 MiB stack. These optimized probes establish that configured stack
bound; they do not measure peak stack usage or claim a debug-build bound.
`normal.rs` checks identity and native interoperability without allocation hooks.

The baseline archive is `../libsnacka_baseline_cca.rlib`, with dependency search
directory `/tmp/snacka-opus-silk-sparse-baseline-target-20261015/release/deps`.
The candidate archive is `../libsnacka_sparse.rlib`, with dependency search
directory `/tmp/snacka-opus-silk-sparse-fallback-target-20261015/release/deps`.
All binaries were compiled with the installed Rust 1.99 compiler, edition 2024
and `-O`, for example:

```sh
TASK_ARTIFACTS=/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015
TASK_CONTROLS="$TASK_ARTIFACTS/controls"
TASK_RUSTC=/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin/rustc
"$TASK_RUSTC" --edition=2024 -O "$TASK_CONTROLS/fallback.rs" --extern "snacka=$TASK_ARTIFACTS/libsnacka_sparse.rlib" -L dependency=/tmp/snacka-opus-silk-sparse-fallback-target-20261015/release/deps -o "$TASK_CONTROLS/fallback-candidate"
"$TASK_CONTROLS/fallback-candidate" "$TASK_CONTROLS/fallback-candidate-data"
```

The same invocation with baseline archive/dependency paths creates
`fallback-baseline`; `normal.rs` creates the normal-control binaries and
`composition.rs` creates composition binaries. `late_output.rs` is candidate only.
Completed postprocessing commands:

```sh
python3 "$TASK_CONTROLS/compare_normal.py" "$TASK_CONTROLS/normal-baseline-data" "$TASK_CONTROLS/normal-candidate-data"
python3 "$TASK_CONTROLS/compare_fallback.py" "$TASK_CONTROLS/fallback-baseline-data" "$TASK_CONTROLS/fallback-candidate-data"
python3 "$TASK_CONTROLS/native_compositions.py" "$TASK_CONTROLS/composition-candidate-data"
"$TASK_CONTROLS/late-output-candidate"
```

Every command exited successfully. Source integration validation, the original
two refusals and the broader 924-configuration trace are reported separately
by the parent agent.
