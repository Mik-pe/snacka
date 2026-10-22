#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build/test the isolated energy/pitch probe and retain complete measurements.

No model fitting, audio downloads, cloud allocation or production changes.
"""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import platform
import statistics
import subprocess
from datetime import datetime, timezone


ROOT = Path(__file__).resolve().parents[1]


def run(arguments):
    return subprocess.run(arguments, cwd=ROOT, check=True, text=True, capture_output=True).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--runs", type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.runs <= 10:
        parser.error("runs must be in1..10")
    if args.output.exists():
        parser.error("fresh output directory required")
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    source = "tools/probe_plc_energy_pitch.rs"
    compiler = ["rustc", "--edition=2024", "-C", "opt-level=3"]
    tests = args.output / "probe-tests"
    binary = args.output / "probe"
    run(compiler + ["--test", source, "-o", str(tests)])
    test_output = run([str(tests)])
    (args.output / "tests.log").write_text(test_output)
    run(compiler + [source, "-o", str(binary)])
    diagnostics = run([str(binary), "--diagnose"])
    (args.output / "pitch-diagnostics.csv").write_text(diagnostics)
    rows = []
    for repetition in range(args.runs):
        timings = run([str(binary)])
        (args.output / f"timings-{repetition}.csv").write_text(timings)
        rows.extend(dict(row, run=repetition) for row in csv.DictReader(io.StringIO(timings)))
    groups = {}
    for row in rows:
        key = (row["run"], row["component"], int(row["samples"]))
        groups.setdefault(key, []).append(float(row["nanoseconds_per_call"]) / 1000)
    summary = []
    for (repetition, component, samples), values in groups.items():
        if len(values) != 21:
            raise ValueError("incomplete repetitions")
        summary.append(dict(run=repetition, component=component, samples=samples, repetitions=len(values),
                            median_us=statistics.median(values), min_us=min(values), max_us=max(values)))
    report = dict(
        schema=1, kind="synthetic-feasibility-not-speech-quality",
        created_at=datetime.now(timezone.utc).isoformat(),
        base_commit=run(["git", "rev-parse", "HEAD"]).strip(),
        platform=platform.platform(), rustc=run(["rustc", "--version"]).strip(),
        compile_options=compiler[1:],
        source_sha256={name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                       for name in (source, "tools/measure_plc_energy_pitch.py", "src/concealment.rs", "src/neural_plc.rs",
                                    "src/neural_plc_weights.rs", "tests/testdata/neural-plc-phase-parity.bin")},
        binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
        tests=test_output, summary=summary, raw_timings=rows,
        pitch_diagnostics=list(csv.DictReader(io.StringIO(diagnostics))),
        limits=["Synthetic spectra and waveform histories; no speech quality conclusion.",
                "Component wall timings; not an integrated decoder or deadline guarantee.",
                "One older 10ms pitch window has 13 lag candidates; matched windows have26, plus fractional refinement.",
                "Native begin includes observing/copying1920 samples in this standalone probe.",
                "No CUDA/Colab/Drive/audio access; no newly fitted model."])
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
