# Bit-exact native PLC search optimization

The native decoder keeps PR31's learned gains and phase parameters. The first-loss pitch search now evaluates four consecutive lag candidates using a contiguous four-sample window. Each lane retains its original floating-point addition order; coarse threshold updates and fine-search ties still run in ascending lag order. Remaining one-to-three candidates use the original scalar routine. Curvature interpolation is unchanged.

Inlining matters: the earlier general four-lag prototype regressed latency, and the initial consecutive-window version also failed to improve it. The accepted layout specializes the window/stride at its call sites. The failed prototypes remain in the private preflight evidence.

## Verification

- 200 silence, periodic and random signal cases compare every optimized lane's score bits against the independent scalar calculation, including the 320-sample coarse buffer and fine-search stride.
- Fresh scalar/optimized applications produce identical PCM hashes for 216 streams across 36 source/configuration cases with isolated and six-frame loss masks.
- All 374 release tests with the pure-Rust Opus feature pass, including the zero-allocation suite. All-target Clippy passes with warnings denied.
- Encoder/decoder state and all neural weights are unchanged. Standard Opus source, packet syntax, pulse tables and Phase 1 are unchanged.

## Paired M5 timing

The [raw timing report](neural-plc-search-timing.json) uses the same compiler/options and source pair: frozen scalar PR31 versus the consecutive-lane search. Three known source views cover English 48 kHz, filtered English 16 kHz and Swedish 16 kHz, each at 12/24/32 kbps, 10/20 ms and both harmonic settings.

Each input repeats to 30 seconds. Three alternating warmup pairs precede eleven measured alternating pairs; the first 80 frames of each trace are excluded. Measurements time the actual online decoder call, excluding encoding and offline metric capture.

| First-loss median across the 36 strata | Scalar PR31 | Consecutive lanes |
|---|---:|---:|
| Range | 17.625–23.500 µs | 14.833–20.833 µs |

Every measured stratum improved by 11.2–16.3%, and every new median was below 25 µs. These are median CPU measurements on the measured M5 setup, not worst-case scheduling guarantees or end-to-end call latency. The change concerns the first lost frame; continued losses already reuse the period search.

The six-fit neural study remains bound to its original scalar source and fixed weights. This separate search change preserves the decoded signal rather than changing the study's candidates or acceptance criteria. Historical experimental builds now explicitly restore the frozen source commit instead of inheriting a future main.
