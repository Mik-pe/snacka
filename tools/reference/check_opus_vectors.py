# SPDX-License-Identifier: MIT
"""Run official Opus decoder conformance without a foreign codec in Snacka.

The Rust probe links only Snacka and decodes all `.bit` records with final-range
checks. The supplied external opus_compare executable is the published PCM
metric, not a decoder. RFC 6716 section 6 requires its quality score >= 0 and
identical final ranges; section 6.1 recommends quality > 90 at 48 kHz.
https://www.rfc-editor.org/rfc/rfc6716#section-6

Download the RFC 8251 updated vectors separately from:
https://opus-codec.org/docs/opus_testvectors-rfc8251.tar.gz

Example:
  python3 tools/reference/check_opus_vectors.py /path/opus_newvectors \
    --opus-compare /path/opus_compare --output-dir /tmp/snacka-opus-vectors
"""

import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import sys


# Published format-test data from RFC 8251 section 11:
# https://datatracker.ietf.org/doc/html/rfc8251#section-11
# Each tuple is indexed by the complete testvector01..12 suite. The `m.dec`
# variant disables CELT's optional 180-degree phase shift; both sets are stereo.
BIT_SHA1 = (
    "e49b2862ceec7324790ed8019eb9744596d5be01",
    "b809795ae1bcd606049d76de4ad24236257135e0",
    "e0c4ecaeab44d35a2f5b6575cd996848e5ee2acc",
    "a0f870cbe14ebb71fa9066ef3ee96e59c9a75187",
    "9b3d92b48b965dfe9edf7b8a85edd4309f8cf7c8",
    "28e66769ab17e17f72875283c14b19690cbc4e57",
    "bacf467be3215fc7ec288f29e2477de1192947a6",
    "ddbe08b688bbf934071f3893cd0030ce48dba12f",
    "3932d9d61944dab1201645b8eeaad595d5705ecb",
    "521eb2a1e0cc9c31b8b740673307c2d3b10c1900",
    "6bc8f3146fcb96450c901b16c3d464ccdf4d5d96",
    "338c3f1b4b97226bc60bc41038becbc6de06b28f",
)
DECODED_SHA1 = {
    "normal": (
        "f5ef93884da6a814d311027918e9afc6f2e5c2c8",
        "48ac1ff1995250a756e1e17bd32acefa8cd2b820",
        "d15567e919db2d0e818727092c0af8dd9df23c95",
        "1249dd28f5bd1e39a66fd6d99449dca7a8316342",
        "b85675d81deef84a112c466cdff3b7aaa1d2fc76",
        "55f0b191e90bfa6f98b50d01a64b44255cb4813e",
        "61e8b357ab090b1801eeb578a28a6ae935e25b7b",
        "a58539ee5321453b2ddf4c0f2500e856b3966862",
        "bb96aad2cde188555862b7bbb3af6133851ef8f4",
        "1b6cdf0413ac9965b16184b1bea129b5c0b2a37a",
        "b1fff72b74666e3027801b29dbc48b31f80dee0d",
        "98e09bbafed329e341c3b4052e9c4ba5fc83f9b1",
    ),
    "no-phase": (
        "1e7d984ea3fbb16ba998aea761f4893fbdb30157",
        "48ac1ff1995250a756e1e17bd32acefa8cd2b820",
        "d15567e919db2d0e818727092c0af8dd9df23c95",
        "1249dd28f5bd1e39a66fd6d99449dca7a8316342",
        "d70b0bad431e7d463bc3da49bd2d49f1c6d0a530",
        "6ac1648c3174c95fada565161a6c78bdbe59c77d",
        "fc5e2f709693738324fb4c8bdc0dad6dda04e713",
        "aad2ba397bf1b6a18e8e09b50e4b19627d479f00",
        "6feb7a7b9d7cdc1383baf8d5739e2a514bd0ba08",
        "1b6cdf0413ac9965b16184b1bea129b5c0b2a37a",
        "fd3d3a7b0dfbdab98d37ed9aa04b659b9fefbd18",
        "98e09bbafed329e341c3b4052e9c4ba5fc83f9b1",
    ),
}


def verify_vectors(directory, reference_set):
    """Reject partial, additional, or changed official vectors before building."""
    stems = [f"testvector{index:02}" for index in range(1, 13)]
    suffix = "m.dec" if reference_set == "no-phase" else ".dec"
    bit_names = {stem + ".bit" for stem in stems}
    decoded_names = {stem + suffix for stem in stems}
    actual_bits = {path.name for path in directory.glob("testvector*.bit")}
    actual_decoded = {
        path.name for path in directory.glob("testvector*.dec")
        if path.name.endswith("m.dec") == (reference_set == "no-phase")
    }
    for kind, expected, actual in (("bitstream", bit_names, actual_bits),
                                   ("decoded reference", decoded_names, actual_decoded)):
        if actual != expected:
            missing, extra = sorted(expected - actual), sorted(actual - expected)
            raise ValueError(f"official {kind} suite must be exactly testvector01..12; missing={missing}, extra={extra}")
    manifest = {}
    for stem, bit_hash, pcm_hash in zip(stems, BIT_SHA1, DECODED_SHA1[reference_set], strict=True):
        for name, expected in ((stem + ".bit", bit_hash), (stem + suffix, pcm_hash)):
            path = directory / name
            if not path.is_file():
                raise ValueError(f"official vector is not a file: {name}")
            with path.open("rb") as source:
                actual = hashlib.file_digest(source, "sha1").hexdigest()
            if actual != expected:
                raise ValueError(f"RFC 8251 SHA-1 mismatch for {name}: expected={expected}, actual={actual}")
            manifest[name] = actual
    vectors = [directory / (stem + ".bit") for stem in stems]
    references = {vector: vector.with_name(vector.stem + suffix) for vector in vectors}
    return vectors, references, manifest


def build_probe(root, destination):
    target = destination / "target"
    command = ["cargo", "build", "--release", "--features", "opus", "--lib",
               "--target-dir", str(target), "--message-format=json"]
    built = subprocess.run(command, cwd=root, text=True, capture_output=True)
    if built.returncode:
        raise RuntimeError(built.stderr + built.stdout)
    library = None
    for line in built.stdout.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("reason") == "compiler-artifact" and event["target"]["name"] == "snacka":
            library = next((Path(path) for path in event["filenames"] if path.endswith(".rlib")), library)
    if library is None:
        raise RuntimeError("Cargo did not report Snacka's compiled Rust library")
    probe = destination / "opus_vectors"
    subprocess.run(["rustc", "--edition=2024", "-O", str(root / "tools/reference/opus_vectors.rs"),
                    "--extern", f"snacka={library}", "-L", f"dependency={library.parent}",
                    "-o", str(probe)], check=True, cwd=root)
    return probe


def compare(executable, reference, decoded, channels):
    command = [str(executable)]
    if channels == 2:
        command.append("-s")
    command += ["-r", "48000", str(reference), str(decoded)]
    compared = subprocess.run(command, text=True, capture_output=True)
    report = compared.stdout + compared.stderr
    match = re.search(r"Opus quality metric:\s*([+-]?[\d.]+)", report)
    quality = float(match.group(1)) if match else None
    approximate = False
    if quality is None:
        match = re.search(r"Internal weighted error is\s*([\d.eE+-]+)", report)
        if match:
            # Failing published comparator prints weighted error, not Q. Its
            # documented formula recovers Q from the rounded reported error.
            quality = 100 * (1 - 0.5 * math.log1p(float(match.group(1))) / math.log(1.13))
            approximate = True
    return {"passed": compared.returncode == 0, "quality": quality,
            "quality_from_rounded_error": approximate, "report": report.strip()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vectors", type=Path)
    parser.add_argument("--opus-compare", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--channels", choices=("both", "1", "2"), default="both")
    parser.add_argument("--reference-set", choices=("normal", "no-phase"), default="normal",
                        help="RFC 8251 permits either complete output set; 'm.dec' disables 180-degree CELT phase, it is not mono")
    parser.add_argument("--probe", type=Path, help="Use an already built owned Rust probe")
    args = parser.parse_args()
    try:
        vectors, references, manifest = verify_vectors(args.vectors.resolve(), args.reference_set)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    destination = args.output_dir.resolve()
    root = Path(__file__).resolve().parents[2]
    if destination == args.vectors.resolve() or root in destination.parents or root == destination:
        parser.error("output directory must be outside the repository and vector source directory")
    destination.mkdir(parents=True, exist_ok=True)
    probe = args.probe.resolve() if args.probe else build_probe(root, destination)
    channels = (1, 2) if args.channels == "both" else (int(args.channels),)
    results = []
    for count in channels:
        for vector in vectors:
            decoded = destination / f"{vector.stem}-{count}ch.dec"
            run = subprocess.run([str(probe), str(vector), str(decoded), str(count)], text=True, capture_output=True)
            try:
                result = json.loads(run.stdout)
            except json.JSONDecodeError:
                result = {"input": str(vector), "channels": count, "complete": False,
                          "probe_error": (run.stderr + run.stdout).strip()}
            result["decode_passed"] = run.returncode == 0
            result["reference_set"] = args.reference_set
            with vector.open("rb") as source:
                result["vector_sha256"] = hashlib.file_digest(source, "sha256").hexdigest()
            if result.get("complete"):
                result["comparison"] = compare(args.opus_compare.resolve(), references[vector], decoded, count)
            else:
                result["comparison"] = {"passed": False, "quality": None, "report": "not run: incomplete decoding"}
            results.append(result)
            print(json.dumps(result, sort_keys=True), flush=True)
    passed = all(result["decode_passed"] and result["comparison"]["passed"] for result in results)
    qualities = [result["comparison"]["quality"] for result in results if result["comparison"]["quality"] is not None]
    summary = {"passed": passed, "reference_set": args.reference_set, "verified_rfc8251_sha1": manifest, "runs": len(results), "fully_decoded": sum(result.get("complete", False) for result in results),
               "minimum_quality": min(qualities) if qualities else None, "recommended_48khz_quality": 90,
               "pcm_passing_quality": 0, "results": results}
    (destination / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({key: value for key, value in summary.items() if key != "results"}, sort_keys=True))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
