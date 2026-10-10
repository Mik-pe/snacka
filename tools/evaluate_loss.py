#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Compare causal loss concealment with an aligned, uninterrupted decode."""
import argparse
import csv
import hashlib
import math
from pathlib import Path
import subprocess
import tempfile

from evaluate_speech import floats, noisy


def snr(reference, output, indices):
    energy = sum(float(reference[i]) ** 2 for i in indices)
    error = sum((float(output[i]) - float(reference[i])) ** 2 for i in indices)
    return 10 * math.log10(max(energy, 1e-30) / max(error, 1e-30))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--roundtrip", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    args = parser.parse_args()
    current = args.roundtrip.resolve(strict=True)
    baseline = args.baseline.resolve(strict=True)
    sources = sorted(args.input.glob("*.flac"))
    if not sources:
        parser.error("input directory contains no FLAC files")
    rows = []
    with tempfile.TemporaryDirectory(prefix="snacka-loss-") as temporary:
        directory = Path(temporary)
        source = directory / "input.f32"
        output = directory / "decoded.f32"

        def run(executable, duration, period=None, burst=None):
            command = [str(executable), str(source), str(output), "24000", str(duration)]
            if period is not None:
                command.extend([str(period), str(burst)])
            subprocess.run(command, check=True, capture_output=True)
            values = floats(output.read_bytes())
            if len(values) != len(original) or not all(math.isfinite(x) for x in values):
                raise ValueError("invalid decoded PCM")
            return values

        for flac in sources:
            subprocess.run(
                [args.ffmpeg, "-v", "error", "-y", "-i", str(flac), "-ar", "48000",
                 "-ac", "1", "-f", "f32le", str(source)], check=True,
            )
            clean = source.read_bytes()
            clean_values = floats(clean)
            digest = hashlib.sha256(flac.read_bytes()).hexdigest()
            for condition in ("clean", "noise20", "noise10"):
                data, clipped = (clean, 0) if condition == "clean" else noisy(
                    clean_values, flac.stem, int(condition[5:])
                )
                original = floats(data)
                source.write_bytes(data)
                for duration in (10, 20):
                    samples = 48 * duration
                    frames = (len(original) + samples - 1) // samples
                    reference = run(baseline, duration)
                    if run(current, duration) != reference:
                        raise ValueError("uninterrupted output differs from baseline")
                    for period, burst in ((10, 1), (30, 3), (60, 6)):
                        # Alignment discards 120 priming samples once, before masking.
                        lost = [i for i in range(len(original))
                                if (i + 120) // samples < frames
                                and ((i + 120) // samples) % period >= period - burst]
                        recovery = [i for i in range(len(original))
                                    if ((i + 120) // samples) % period == 0
                                    and (i + 120) % samples < 120
                                    and i >= period * samples - 120]
                        if not lost or not recovery:
                            raise ValueError(f"clip is too short for loss trace: {flac.name}")
                        row = dict(
                            clip=flac.stem, flac_sha256=digest, condition=condition,
                            pcm_sha256=hashlib.sha256(data).hexdigest(),
                            clipped_samples=clipped, samples=len(original),
                            bitrate=24000, frame_ms=duration, period=period, burst=burst,
                            lost_samples=len(lost), recovery_samples=len(recovery),
                        )
                        for label, executable in (("baseline", baseline), ("current", current)):
                            values = run(executable, duration, period, burst)
                            for metric, indices in (("lost", lost), ("recovery", recovery),
                                                    ("whole", range(len(original)))):
                                row[f"{label}_{metric}_snr_db"] = snr(reference, values, indices)
                        for metric in ("lost", "recovery", "whole"):
                            row[f"delta_{metric}_db"] = (
                                row[f"current_{metric}_snr_db"] - row[f"baseline_{metric}_snr_db"]
                            )
                        for key, value in row.items():
                            if isinstance(value, float):
                                row[key] = f"{value:.6f}"
                        rows.append(row)
            print(f"evaluated {flac.stem}", flush=True)
    with args.output.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


if __name__ == "__main__":
    main()
