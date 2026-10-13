#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Regenerate independent CELT band decoding evidence from RFC 6716 Appendix A.

The downloaded standard source is an external oracle, never part of Snacka's
build or runtime dependencies. Invoke with the extracted Appendix A directory.
The fixture records 256 cases with final range, fractional tell, noise seed,
collapse masks and normalized f32 coefficients. Oracle entropy errors are also
recorded: arbitrary compressed inputs can intentionally be invalid.
"""

import argparse
from pathlib import Path
import subprocess
import tempfile


def main() -> None:
    repository = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("normative", type=Path, help="extracted RFC 6716 Appendix A source")
    parser.add_argument("--compiler", default="clang")
    parser.add_argument("--output", type=Path, default=repository / "src/opus/testdata/bands-rfc6716.bin")
    parser.add_argument("--collapse-output", type=Path, help="anti-collapse fixture destination")
    args = parser.parse_args()
    normative = args.normative.resolve()
    sources = (
        "bands", "cwrs", "entcode", "entdec", "entenc", "kiss_fft", "laplace",
        "mathops", "mdct", "modes", "pitch", "celt_lpc", "quant_bands", "rate", "vq",
    )
    with tempfile.TemporaryDirectory(prefix="snacka-opus-bands-") as scratch:
        executable = Path(scratch) / "oracle"
        subprocess.run(
            [args.compiler, "-O2", "-DOPUS_BUILD", "-DUSE_ALLOCA",
             f"-I{normative / 'include'}", f"-I{normative / 'celt'}",
             str(repository / "tools/reference/generate_opus_bands_reference.c"),
             *(str(normative / "celt" / f"{source}.c") for source in sources),
             "-o", str(executable)],
            check=True,
        )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("wb") as output:
            subprocess.run([str(executable)], stdout=output, check=True)
        collapse_output = args.collapse_output or args.output.with_name("bands-anticollapse-rfc6716.bin")
        collapse_output.parent.mkdir(parents=True, exist_ok=True)
        with collapse_output.open("wb") as output:
            subprocess.run([str(executable), "anti-collapse"], stdout=output, check=True)
    print(f"Wrote {args.output.stat().st_size} bytes to {args.output}")
    print(f"Wrote {collapse_output.stat().st_size} bytes to {collapse_output}")


if __name__ == "__main__":
    main()
