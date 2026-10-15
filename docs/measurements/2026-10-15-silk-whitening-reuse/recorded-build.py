# SPDX-License-Identifier: MIT
"""Rebuild the frozen pair. All generated artifacts stay outside the checkout."""
from pathlib import Path
import hashlib
import io
import json
import os
import platform
import subprocess
import tarfile

COMBINED = "fb5dc6cb295f6fcb561b142d48a8e49f9a9312d0"
BEFORE_SYNTHESIS = "e831eca6ec987eb1110ba9a15d39b9e2a21eddbd"
ROOT = Path(os.environ["SNACKA_BENCH_CHECKOUT"]).resolve()
BASE = Path(os.environ["SNACKA_BENCH_ARTIFACTS"]).resolve()
TC = Path(os.environ["SNACKA_RUST_TOOLCHAIN_BIN"]).resolve()
REFERENCE = Path(os.environ["SNACKA_BENCH_REFERENCE"]).resolve()
if BASE == ROOT or ROOT in BASE.parents:
    raise SystemExit("Artifact directory must be outside the checkout")
BASE.mkdir(parents=True, exist_ok=True)
env = dict(os.environ)
for key in list(env):
    if key.startswith("CARGO_PROFILE_RELEASE_") or key.endswith("_RUSTFLAGS") or key in ("RUSTFLAGS", "RUSTC_BOOTSTRAP"):
        env.pop(key)
env.update({"RUSTC": str(TC / "rustc"), "RUSTDOC": str(TC / "rustdoc"),
            "RUSTC_WRAPPER": "", "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_INCREMENTAL": "0", "CARGO_BUILD_INCREMENTAL": "false",
            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16", "CARGO_PROFILE_RELEASE_LTO": "false",
            "RUSTFLAGS": "", "CARGO_ENCODED_RUSTFLAGS": ""})
keys = ("RUSTC", "RUSTDOC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTC_BOOTSTRAP",
        "CARGO_INCREMENTAL", "CARGO_BUILD_INCREMENTAL", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
        "CARGO_PROFILE_RELEASE_LTO", "CARGO_PROFILE_RELEASE_DEBUG", "CARGO_PROFILE_RELEASE_OPT_LEVEL",
        "CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS", "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS",
        "CARGO_PROFILE_RELEASE_PANIC", "CARGO_PROFILE_RELEASE_STRIP", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS", "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS", "CARGO_BUILD_TARGET",
        "MACOSX_DEPLOYMENT_TARGET", "SDKROOT", "CC", "CXX", "CFLAGS", "CXXFLAGS", "LDFLAGS")
manifest = {"combined_commit": COMBINED, "before_synthesis_commit": BEFORE_SYNTHESIS,
            "platform": platform.platform(), "architecture": platform.machine(),
            "production_environment": {key: env.get(key) for key in keys}, "variants": {},
            "reference": {"path": str(REFERENCE), "sha256": hashlib.sha256(REFERENCE.read_bytes()).hexdigest()},
            "rustc_version": subprocess.check_output([str(TC / "rustc"), "-Vv"], env=env, text=True)}
archive = subprocess.check_output(["git", "archive", "--format=tar", COMBINED], cwd=ROOT)
original = subprocess.check_output(["git", "show", f"{BEFORE_SYNTHESIS}:src/opus/silk/synthesis.rs"], cwd=ROOT)
for role in ("before", "after"):
    source = BASE / role
    source.mkdir(exist_ok=False)
    with tarfile.open(fileobj=io.BytesIO(archive)) as files:
        files.extractall(source, filter="data")
    if role == "before":
        (source / "src/opus/silk/synthesis.rs").write_bytes(original)
    target = BASE / f"{role}-target"
    binary = BASE / f"{role}-decode"
    build = [str(TC / "cargo"), "build", "--locked", "--release", "--features", "opus", "--lib", "--target-dir", str(target)]
    subprocess.run(build, cwd=source, env=env, check=True)
    probe = source / "tools/reference/benchmark_opus_decode.rs"
    bridge = [str(TC / "rustc"), "--edition=2024", "-O", "-C", "codegen-units=16", "-C", "lto=off",
              "-C", "debuginfo=0", "-C", "overflow-checks=yes", str(probe), "--extern",
              f"snacka={target}/release/libsnacka.rlib", "-L", f"dependency={target}/release/deps",
              "-L", f"native={REFERENCE.parent}", "-o", str(binary)]
    subprocess.run(bridge, cwd=source, env=env, check=True)
    paths = [source / "Cargo.toml", source / "Cargo.lock", *sorted((source / "src").rglob("*"))]
    manifest["variants"][role] = {"library_command": build, "bridge_command": bridge,
        "sources": {str(path.relative_to(source)): hashlib.sha256(path.read_bytes()).hexdigest()
                    for path in paths if path.is_file()},
        "bridge_sha256": hashlib.sha256(probe.read_bytes()).hexdigest(),
        "library_sha256": hashlib.sha256((target / "release/libsnacka.rlib").read_bytes()).hexdigest(),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
left, right = (manifest["variants"][role]["sources"] for role in ("before", "after"))
manifest["source_differences"] = [key for key in sorted(left.keys() | right.keys()) if left.get(key) != right.get(key)]
assert manifest["source_differences"] == ["src/opus/silk/synthesis.rs"]
(BASE / "build.json").write_text(json.dumps(manifest, indent=2) + "\n")
