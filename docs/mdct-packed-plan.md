# Packed MDCT plan

The transform stores window and rotation coefficients in contiguous rows.
Arithmetic order, packet profiles, decoder behavior and defaults remain unchanged.
All 29,448 tested API records and 1,472 complete packet/PCM trace configurations
matched the baseline; 96 additional rows reuse existing high-rate observations.

One fixed Apple M5 trial measured 5.53–7.96% lower median paired decoder process
CPU count differences across harmonic/default modes and 10/20 ms frames.
Harmonic encoding was approximately unchanged. All 256 paired results, 61 CPU
regressions and 69 wall regressions remain available; shared-host noise was
substantial. This is a repeated speech workload including process setup, not a
phone, battery, callback-deadline or tail-latency guarantee.

Construction temporarily adds at most 30,720 numerical-payload bytes; persistent
payload remains 16N bytes and processing adds no allocations. Conservative normal
Rust stack-chain bounds are 58,576 bytes for encode and 26,672 for decode,
excluding callers, platform and panic/unwinding code.

See the [complete measurement report](measurements/2026-10-11-mdct-packed-plan/README.md),
[CPU summaries](measurements/2026-10-11-mdct-packed-plan/cpu/summary8.csv),
[measured build inputs](measurements/2026-10-11-mdct-packed-plan/source/build39-provenance.json),
[current publication source](measurements/2026-10-11-mdct-packed-plan/source/publication-source-inventory.json)
and [artifact manifest](measurements/2026-10-11-mdct-packed-plan/artifacts.sha256).
