#!/bin/zsh
set -eu
adapter_root=/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015/controls/callback-latency
cpu_root=/tmp/snacka-opus-silk-sparse-cpu-20261015
compiler=/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin/rustc
suffix=${1:?Pass a fresh verification output suffix}
for role in baseline candidate
do
  test ! -e "$adapter_root/verify-$suffix-$role"
  "$compiler" --edition=2024 -C opt-level=3 -C codegen-units=16 -C overflow-checks=on -C embed-bitcode=no -C strip=debuginfo "$adapter_root/adapter.rs" --extern "snacka=$cpu_root/$role-clean-target/release/libsnacka.rlib" -L "dependency=$cpu_root/$role-clean-target/release/deps" -o "$adapter_root/callback-$suffix-$role"
done
"$adapter_root/callback-$suffix-baseline" --prepare-normal
"$adapter_root/callback-$suffix-baseline" --verify --role baseline --output "$adapter_root/verify-$suffix-baseline"
"$adapter_root/callback-$suffix-candidate" --verify --role candidate --output "$adapter_root/verify-$suffix-candidate"
