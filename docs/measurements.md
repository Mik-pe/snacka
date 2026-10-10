# Measurements

## Method

Run `cargo bench --locked --bench kernels --features reference-tests` in a release
build with an independently installed libopus. Omit the feature to measure only
the native kernels. Inputs, plans, workspaces and output buffers are prepared
outside the measured operation. Outputs are observed through `black_box`.

Each kernel receives 1,000 warmups. A batch size is calibrated once to a duration
of at least 200 microseconds, then 1,000 batches are timed and sorted. The report
divides each batch duration by its operation count. The p95 and p99 columns
describe **batch-average throughput**, not individual audio callback tail latency.
Batching avoids the timer resolution floor seen when timing tiny G.711 and packet
operations individually. Timer, loop and API call overhead remain included.

The packet comparison uses identical one-frame, 80-byte payloads and observes
frame lengths, configuration and channels in both implementations. The C call's
FFI overhead is included. The native API returns a validated owned descriptor;
the C API writes caller-provided arrays. This is one API-level framing workload,
not an audio encoding or decoding comparison.

## Initial local run

The checked-in [raw result table](measurements/2026-10-10-apple-m5.csv) was captured
on Apple M5, macOS 27.0 build 26A428, aarch64, Rust 1.99.0
(`b940084d7`, 2026-09-28), and libopus 1.6.1. Cargo's release profile uses overflow
checks; no `target-cpu=native`, LTO or explicit SIMD was enabled. This was a shared
desktop run without CPU affinity or fixed power settings. Repeated runs vary,
especially the high percentiles.

The single-frame parser is slower than libopus in this initial workload. No
speed advantage over a complete competing codec has been established. The FFT
avoids recursive calls for single-sample leaves and skips identity twiddle
multiplications, while retaining independent accuracy tests. These are internal
kernel optimizations, not evidence of full-codec superiority.

## Correctness and robustness evidence

- All 256 G.711 codewords for each law match independent decoded fixtures.
  Every PCM16 input is checked for monotonic reconstruction and bounded error.
- Packet differential tests cover all 256 one-byte headers, all 65,536 two-byte
  headers, and 100,000 pseudorandom packets of lengths up to 4,096 bytes. They
  compare acceptance, frame boundaries, channel count and sample duration.
- Independent libopus entropy vectors cover 128 frames and 16,384 events.
  Each value, interval and whole/fractional bit count matches. An optional live
  oracle also decoded all 128 packets emitted by our encoder, checking
  termination and suffix compaction in the other direction.
- FFT output agrees with an independent double-precision DFT at over 115 dB
  for tested lengths up to 480. MDCT agrees with its defining cosine sum at over
  110 dB for 120/240/480/960 coefficients. Overlapping inverse blocks reconstruct
  the tested signal with absolute error below `3e-6`. These figures measure
  numerical agreement, **not perceptual codec audio quality**.
- The allocator test observed zero allocations over 1,000 repetitions of the
  processing sequence after plans and workspaces had been constructed.
- Initial coverage-guided libFuzzer runs with AddressSanitizer, seed 6716 and
  a 30-second requested limit completed 30,710,988 packet inputs and 3,862,907
  entropy inputs without a reported failure. Maximum input lengths were 65,536
  and 4,096 bytes respectively. Corpus, coverage and memory figures belong to
  the fuzz engine and must not be treated as library performance measurements.
- Local debug/release tests and Clippy passed. Library checks passed for
  `aarch64-apple-ios`, `aarch64-apple-ios-sim` and `aarch64-linux-android`.
  Cross-compilation is not installed-device acceptance.

## Full-codec comparison still required

The Opus audio encoder/decoder, SILK/CELT state transitions, PLC, FEC and
multistream behavior remain incomplete. There is no full-codec bitrate,
algorithmic-delay, quality, memory or tail-latency comparison yet.

Those comparisons need matched settings, explicit competitor versions, public
corpora with reproducible licenses, noisy speech and music, packet loss traces,
quality metrics and listening evidence. Measure cold initialization and maximum
stream storage separately from allocation-free steady-state processing. Include
per-callback latency under load, and report regressions alongside improvements.
