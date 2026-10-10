# Fuzzing

The standalone fuzz package exercises native packet framing and audio decoding, malformed entropy frames,
argument bounds, small output buffers and mixed-operation entropy round trips.
The packet target generates valid frames at both durations and mutates their
payloads while repairing CRC to reach spectral validation. It verifies rejected
packets leave output untouched and accepted audio stays finite and normalized.
Both targets cap work per input. The main library remains dependency-free.
The packet target primes four received frames before corruption, reaching the
full pitch-analysis history at both durations. Twelve consecutive losses check
bounded PCM and exact silence after the fade and overlap drain.
Profile mutations cover every profile, including invalid 6/7 and harmonic
14/15, with repaired checksums. The encoder also exercises the opt-in model.
Corruption can reach paid pitch fields, harmonic padding and entropy interpreted
under a different parameter dimension.

Install `cargo-fuzz`, a nightly Rust toolchain and the C++ compiler required by
libFuzzer, following the [Rust Fuzz setup guide](https://rust-fuzz.github.io/book/cargo-fuzz/setup.html).
This compiler is for the external fuzz engine, not a library build dependency.

```sh
cargo +nightly fuzz run packet -- -max_total_time=60 -max_len=1024
cargo +nightly fuzz run entropy -- -max_total_time=60 -max_len=4096
```

Longer runs should retain their corpora. Reproduce and minimize failures before
fixing them, and add the minimal case to ordinary regression tests. A clean short
run is evidence about that run, not proof that all malformed inputs are safe.
