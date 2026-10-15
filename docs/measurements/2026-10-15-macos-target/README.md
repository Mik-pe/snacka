# Rust macOS CPU baseline

The installed Rust 1.99.0 compiler (`b940084d7`, LLVM 23.1.1) already defaults
`aarch64-apple-darwin` to **apple-m1**, as its
[exact target source](https://github.com/rust-lang/rust/blob/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4/compiler/rustc_target/src/spec/targets/aarch64_apple_darwin.rs#L21)
and the read-only `installed.json` snapshot confirm. Explicit
`-C target-cpu=apple-m1` produces the identical target-feature configuration.
There is no generic-to-M1 optimization to obtain by adding that flag. The
[supported platform](https://doc.rust-lang.org/rustc/platform-support/apple-darwin.html)
is M1 or later Apple Silicon, with macOS 11.0 as the default deployment target.

On this M5 host, `native` resolves to `apple-m5`; its additional Rust-exposed
features are `bf16`, `bti`, `i8mm` and `mte`. `cfg(target_feature)` is not a
complete LLVM scheduling/tuning inventory. The
[LLVM processor definitions](https://github.com/rust-lang/llvm-project/blob/1b9c0d5ff9bbe7634aead059efe6b11a7eeba145/llvm/lib/Target/AArch64/AArch64Processors.td)
from this Rust source's pinned LLVM submodule also distinguish Apple tuning
policies; M1 is an alias for A14. Neither feature presence nor tuning differences
establish a complete-codec speed gain. Keep the default M1-compatible build;
no global `native`, feature, Cargo profile or production configuration change
is part of this investigation.

`target-cpu` selects instructions and tuning; it does not change this target's
Mach-O format or target triple. It does not by itself switch to the arm64e ABI,
change public Rust function types or raise the declared Rust 1.99 minimum.
It can change binary CPU compatibility. An M5-native arm64 slice cannot be
advertised as M1 compatible without an instruction-set proof. A Mac Intel
build remains a separate `x86_64-apple-darwin` slice; Apple CPU flags must never
be applied globally across both triples. No native build or timing was run.

CPU flags belong to the consuming application's build decision. Cargo searches
configuration from the invocation directory and its ancestors; checking a
`.cargo/config.toml` into a library cannot impose that policy on its consumers.
[Target-scoped Cargo rustflags](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerustflags)
and explicit `--target aarch64-apple-darwin` can confine a consumer's experiment
to the arm64 target. `CARGO_ENCODED_RUSTFLAGS` and `RUSTFLAGS` take precedence,
so record the effective verbose compiler invocations. Precompiled standard
libraries are not rebuilt merely by setting the application's CPU flag.

Reproduce only the metadata queries, without building source:

```sh
python3 tools/reference/inspect_rust_macos_target.py \
  /Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin/rustc \
  --output docs/measurements/2026-10-15-macos-target/installed.json
```

The snapshot records compiler and query-output SHA-256 values, commands, target
specification, feature sets, source revision and production-tree hash. The
target-specification print operation needs an unstable query, so the collector
sets `RUSTC_BOOTSTRAP=1` only for that child process; it never compiles a source
file or changes the toolchain. Ordinary target/CPU/feature queries are stable.

If a future coordinated experiment is justified, freeze one clean source
revision, compiler, reference library and input corpus. Build separate target
directories for default, explicit M1 as a control, and native as an M5-only
experiment, changing only the CPU argument. Keep production optimization,
codegen-unit/LTO/overflow settings, bridge flags and output paths consistent;
record binary/code hashes as well as source hashes. First compare the complete
encoder packet hashes, decoder PCM bits, entropy ranges, FEC/PLC/DTX and mode/
rate/channel transitions over the declared matrix. Any changed outputs require
independent quality evaluation before timing is accepted. Then measure complete
encode/decode calls in repeated forward/reverse roles in an otherwise quiet
macOS window, retaining tails and regressions. Default versus explicit M1 is a
redundancy control; binary equality can eliminate its timing run. A native gain
would remain M5-specific evidence until tested against a supported hardware
scope. This note contains no benchmark or codec-quality result.
