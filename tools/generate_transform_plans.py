#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Regenerate Opus transform constants with the dependency-free Rust constructors.

No external codec source or data is read. Run with the repository Rust toolchain;
--check verifies reproducibility on the generating platform. Trigonometric libm
rounding can vary across platforms, so transform tests also check the defining
mathematics independently. Output holds f32 bits, making runtime plans portable.
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    output = root / "src/transform/precomputed.rs"
    source = (root / "src/transform.rs").read_text()
    helper = Path(__file__).with_suffix(".rs").read_text()
    with tempfile.TemporaryDirectory(prefix="snacka-transform-plans-") as temporary:
        directory = Path(temporary)
        generator = directory / "generate.rs"
        generator.write_text(source + "\n" + helper)
        executable = directory / "generate"
        generated = directory / "precomputed.rs"
        subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2024",
                        "-A", "dead_code", str(generator), "-o", str(executable)], check=True)
        subprocess.run([str(executable), str(generated)], check=True)
        subprocess.run([os.environ.get("RUSTFMT", "rustfmt"), "--edition", "2024",
                        str(generated)], check=True)
        data = generated.read_bytes()
    if args.check:
        if output.read_bytes() != data:
            raise SystemExit("Transform constants differ; regenerate with the project Rust toolchain")
        print("Transform constants reproduce exactly")
    else:
        output.parent.mkdir(exist_ok=True)
        output.write_bytes(data)
        print(f"Generated {output.relative_to(root)} ({len(data)} bytes)")


if __name__ == "__main__":
    main()
