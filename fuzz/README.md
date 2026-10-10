# Fuzzing

The standalone fuzz package exercises packet framing, malformed entropy frames,
argument bounds, small output buffers and mixed-operation entropy round trips.
Both targets cap work per input. The main library remains dependency-free.

Install `cargo-fuzz`, a nightly Rust toolchain and the C++ compiler required by
libFuzzer, following the [Rust Fuzz setup guide](https://rust-fuzz.github.io/book/cargo-fuzz/setup.html).
This compiler is for the external fuzz engine, not a library build dependency.

```sh
cargo +nightly fuzz run packet -- -max_total_time=60 -max_len=65536
cargo +nightly fuzz run entropy -- -max_total_time=60 -max_len=4096
```

Longer runs should retain their corpora. Reproduce and minimize failures before
fixing them, and add the minimal case to ordinary regression tests. A clean short
run is evidence about that run, not proof that all malformed inputs are safe.
