# snacka

An independently implemented, MIT-licensed Rust audio codec project.

The goal is an Opus-compatible encoder and decoder for interactive speech, with
measured improvements in speed, latency, memory use and robustness. Implementation
code is written here from the standard's mathematics. External codec libraries
serve only as independent test oracles and benchmark competitors.

**A complete Opus audio encoder and decoder are not available yet.** Current
components are:

| Component | Current evidence |
| --- | --- |
| G.711 A-law and mu-law | Every decoded byte checked against independent FFmpeg fixtures; exhaustive PCM bounds and canonical round trips |
| Opus single-stream packet framing | All four framing codes, padding and duration limits; optional libopus differential tests |
| Opus range coding and raw bits | Symbol, bit and uniform-integer coding; 16,384 reference operations checked for values, interval and bit usage |
| FFT and low-overlap MDCT | Mixed radix 2/3/5, half-length FFT MDCT; independent cosine/DFT comparisons and overlapping reconstruction |

The library has no dependencies, build script, C, FFI, CMake or unsafe Rust.
Plans allocate during construction; processing uses caller-owned buffers. A
counting allocator test checks zero steady-state allocations across packet,
entropy, G.711, FFT and MDCT operations. Input dimensions and numeric bounds are
validated before transform output is changed.

Rust 1.99 or newer is required.

```sh
cargo test --locked
cargo test --locked --release
cargo clippy --locked --all-targets -- -D warnings
cargo bench --locked --bench kernels
```

Optional interoperability tests link to an independently installed libopus:

```sh
cargo test --locked --features reference-tests
cargo bench --locked --bench kernels --features reference-tests
```

On platforms with a nonstandard library search path, supply it through
`RUSTFLAGS="-L /path/to/lib"`. This feature adds linkage only to test and benchmark
programs; it never adds FFI to the library.

See [the implementation plan](docs/implementation.md),
[measurement methodology](docs/measurements.md), [fixture provenance](tests/fixtures/README.md)
and [fuzzing instructions](fuzz/README.md).

The format specification is [RFC 6716](https://www.rfc-editor.org/rfc/rfc6716.html),
with decoder corrections in [RFC 8251](https://www.rfc-editor.org/rfc/rfc8251.html).
The implementation license is [MIT](LICENSE). External reference programs retain
their own licenses; their source is not vendored here. Opus also has separate
[patent licensing terms](https://opus-codec.org/license/).
