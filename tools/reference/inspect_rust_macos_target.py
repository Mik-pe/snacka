#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Snapshot an installed arm64 macOS Rust target without compiling any source.

The unstable target-spec query uses RUSTC_BOOTSTRAP only in that child process.
No Cargo configuration, compiler installation, source or build flags are changed.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess

TARGET = "aarch64-apple-darwin"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def query(rustc, arguments, *, specification=False):
    environment = os.environ.copy()
    if specification:
        environment["RUSTC_BOOTSTRAP"] = "1"
    command = [str(rustc), *arguments]
    completed = subprocess.run(command, check=True, capture_output=True, env=environment)
    return {"command": command, "stdout_sha256": sha256(completed.stdout),
            "stdout": completed.stdout.decode(), "stderr": completed.stderr.decode(),
            "bootstrap_for_query_only": specification}


def inspect(rustc):
    rustc = rustc.resolve(strict=True)
    root = Path(__file__).resolve().parents[2]
    version = query(rustc, ["--version", "--verbose"])
    if f"host: {TARGET}\n" not in version["stdout"]:
        raise ValueError("native CPU inspection requires an aarch64-apple-darwin host compiler")
    revision = re.search(r"^commit-hash: ([0-9a-f]{40})$", version["stdout"], re.MULTILINE).group(1)
    specification = query(rustc, ["-Z", "unstable-options", "--print", "target-spec-json",
                                  "--target", TARGET], specification=True)
    specification["value"] = json.loads(specification.pop("stdout"))
    cpus = query(rustc, ["--print", "target-cpus", "--target", TARGET])
    native = re.search(r"native\s+- Select the CPU of the current host \(currently ([^)]+)\)", cpus.pop("stdout")).group(1)
    configs = {}
    for name, extra in (("default", []), ("apple-m1", ["-C", "target-cpu=apple-m1"]),
                        ("native", ["-C", "target-cpu=native"]),
                        ("generic", ["-C", "target-cpu=generic"])):
        config = query(rustc, ["--print", "cfg", "--target", TARGET, *extra])
        config["features"] = sorted(re.findall(r'^target_feature="([^"]+)"$', config.pop("stdout"), re.MULTILINE))
        configs[name] = config
    source_revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    source_tree = subprocess.check_output(["git", "ls-tree", "-r", "-z", "HEAD", "--", "src", "Cargo.toml", "Cargo.lock"], cwd=root)
    source_dirty = subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=all", "--", "src", "Cargo.toml", "Cargo.lock"], cwd=root, text=True)
    baseline = set(configs["default"]["features"])
    return {
        "scope": "read-only compiler target queries; no build, execution, timing or codec parity evidence",
        "source_revision": source_revision,
        "source_tree_definition": "git ls-tree -r -z HEAD -- src Cargo.toml Cargo.lock",
        "source_tree_sha256": sha256(source_tree),
        "source_tree_dirty": bool(source_dirty),
        "collector_sha256": sha256(Path(__file__).read_bytes()),
        "platform": platform.platform(), "rustc_sha256": sha256(rustc.read_bytes()),
        "rustc_version": version,
        "target_specification": specification,
        "cpu_inventory": cpus, "native_cpu": native,
        "deployment_target": query(rustc, ["--print", "deployment-target", "--target", TARGET]),
        "target_cfg": configs,
        "default_equals_explicit_m1_cfg": configs["default"]["stdout_sha256"] == configs["apple-m1"]["stdout_sha256"],
        "native_extra_cfg_features": sorted(set(configs["native"]["features"]) - baseline),
        "native_missing_cfg_features": sorted(baseline - set(configs["native"]["features"])),
        "rust_target_source": f"https://github.com/rust-lang/rust/blob/{revision}/compiler/rustc_target/src/spec/targets/aarch64_apple_darwin.rs",
        "environment": {name: os.environ.get(name) for name in ("MACOSX_DEPLOYMENT_TARGET", "SDKROOT", "DEVELOPER_DIR")},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rustc", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    serialized = json.dumps(inspect(args.rustc), indent=2) + "\n"
    if args.output:
        args.output.write_text(serialized)
    else:
        print(serialized, end="")


if __name__ == "__main__":
    main()
