#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Offline, aligned waveform comparison; requires FFmpeg and roundtrip binaries."""
import argparse
import array
import csv
import hashlib
import math
from pathlib import Path
import random
import subprocess
import sys
import tempfile


def floats(data):
    values = array.array("f")
    values.frombytes(data)
    if sys.byteorder != "little":
        values.byteswap()
    return values


def pcm(values):
    data = array.array("f", values)
    if sys.byteorder != "little":
        data.byteswap()
    return data.tobytes()


def noisy(values, clip, snr):
    generator = random.Random(f"snacka-speech-6716:{clip}:{snr}")
    noise = [generator.gauss(0.0, 1.0) for _ in values]
    energy = sum(float(x) ** 2 for x in values)
    noise_energy = sum(x * x for x in noise)
    scale = math.sqrt(energy / (10 ** (snr / 10) * noise_energy))
    mixed = [float(x) + scale * n for x, n in zip(values, noise)]
    clipped = sum(abs(x) > 1.0 for x in mixed)
    return pcm(max(-1.0, min(1.0, x)) for x in mixed), clipped


def score(executable, source, output, original, bitrate, duration):
    command = [sys.executable, str(executable)] if executable.suffix == ".py" else [str(executable)]
    result = subprocess.run(
        command + [str(source), str(output), str(bitrate), str(duration)],
        check=True, capture_output=True, text=True,
    )
    data = output.read_bytes()
    decoded = floats(data)
    if len(decoded) != len(original) or not all(math.isfinite(x) for x in decoded):
        raise ValueError("roundtrip returned invalid PCM or a different length")
    energy = sum(float(x) ** 2 for x in original)
    error = sum((float(x) - float(y)) ** 2 for x, y in zip(original, decoded))
    fields = dict(field.split("=", 1) for field in result.stdout.strip().split(","))
    fields["decoded_pcm_sha256"] = hashlib.sha256(data).hexdigest()
    return 10 * math.log10(energy / max(error, 1e-30)), fields


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="directory of original mono FLACs")
    parser.add_argument("output", type=Path, help="CSV results")
    parser.add_argument("--roundtrip", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    args = parser.parse_args()
    executable = args.roundtrip.resolve(strict=True)
    baseline = args.baseline.resolve(strict=True) if args.baseline else None
    sources = sorted(args.input.glob("*.flac"))
    if not sources:
        parser.error("input directory contains no FLAC files")
    rows = []
    with tempfile.TemporaryDirectory(prefix="snacka-speech-") as temporary:
        directory = Path(temporary)
        source = directory / "input.f32"
        decoded = directory / "decoded.f32"
        for flac in sources:
            subprocess.run(
                [args.ffmpeg, "-v", "error", "-y", "-i", str(flac), "-ar", "48000",
                 "-ac", "1", "-f", "f32le", str(source)], check=True,
            )
            clean = source.read_bytes()
            values = floats(clean)
            if not values or not all(math.isfinite(x) and abs(x) <= 1.0 for x in values):
                raise ValueError(f"invalid normalized PCM: {flac.name}")
            digest = hashlib.sha256(flac.read_bytes()).hexdigest()
            for condition in ("clean", "noise20", "noise10"):
                data, clipped = (clean, 0) if condition == "clean" else noisy(
                    values, flac.stem, int(condition[5:])
                )
                original = floats(data)
                source.write_bytes(data)
                pcm_digest = hashlib.sha256(data).hexdigest()
                for duration in (10, 20):
                    for bitrate in (12000, 24000, 32000):
                        current, fields = score(
                            executable, source, decoded, original, bitrate, duration
                        )
                        previous = score(
                            baseline, source, decoded, original, bitrate, duration
                        )[0] if baseline else None
                        rows.append(dict(
                            clip=flac.stem, flac_sha256=digest, condition=condition,
                            pcm_sha256=pcm_digest, clipped_samples=clipped,
                            samples=len(original), frame_ms=duration, bitrate=bitrate,
                            packet_bytes=fields["packet_bytes"], drain_bytes=fields["drain_bytes"],
                            aligned_snr_db=f"{current:.6f}",
                            baseline_snr_db=f"{previous:.6f}" if previous is not None else "",
                            delta_db=f"{current - previous:.6f}" if previous is not None else "",
                        ))
            print(f"evaluated {flac.stem}", flush=True)
    with args.output.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


if __name__ == "__main__":
    main()
