#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Optional offline intelligibility proxies and spectral distortion; no listening score."""
import argparse
import csv
import hashlib
import math
from pathlib import Path
import subprocess
import tempfile
import warnings

import numpy as np
from pystoi import stoi

from evaluate_speech import floats, noisy, score


def spectrum(values):
    frames = np.lib.stride_tricks.sliding_window_view(values, 1024)[::480]
    amplitudes = np.abs(np.fft.rfft(frames * np.hanning(1024), axis=1))
    rms = np.sqrt(np.mean(frames * frames, axis=1))
    return amplitudes, rms


def spectral_distance(reference, values):
    original, rms = reference
    reconstructed, _ = spectrum(values)
    frequency = np.fft.rfftfreq(1024, 1 / 48000)
    bands = (frequency >= 150) & (frequency <= 8000)
    active = rms > max(float(rms.max()) * 0.01, 1e-12)
    floor = np.maximum(original.max(axis=1, keepdims=True) * 0.001, 1e-12)
    ratio = np.maximum(reconstructed, floor) / np.maximum(original, floor)
    distance = np.sqrt(np.mean((20 * np.log10(ratio[:, bands])) ** 2, axis=1))
    return float(distance[active].mean())


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
    with tempfile.TemporaryDirectory(prefix="snacka-quality-") as temporary:
        directory = Path(temporary)
        source = directory / "input.f32"
        output = directory / "decoded.f32"
        for flac in sources:
            subprocess.run(
                [args.ffmpeg, "-v", "error", "-y", "-i", str(flac), "-ar", "48000",
                 "-ac", "1", "-f", "f32le", str(source)], check=True,
            )
            clean = source.read_bytes()
            clean_values = floats(clean)
            clean_array = np.asarray(clean_values, dtype=np.float64)
            digest = hashlib.sha256(flac.read_bytes()).hexdigest()
            if len(clean_values) < 48000:
                raise ValueError("quality evaluation requires at least one second of speech")
            for condition in ("clean", "noise20", "noise10"):
                data, clipped = (clean, 0) if condition == "clean" else noisy(
                    clean_values, flac.stem, int(condition[5:])
                )
                original = floats(data)
                source.write_bytes(data)
                reference = spectrum(np.asarray(original, dtype=np.float64))
                for duration in (10, 20):
                    for bitrate in (12000, 24000, 32000):
                        row = dict(
                            clip=flac.stem, flac_sha256=digest, condition=condition,
                            pcm_sha256=hashlib.sha256(data).hexdigest(),
                            clipped_samples=clipped, samples=len(original),
                            frame_ms=duration, bitrate=bitrate,
                        )
                        for label, executable in (("baseline", baseline), ("current", current)):
                            waveform_snr, fields = score(
                                executable, source, output, original, bitrate, duration
                            )
                            values = np.asarray(floats(output.read_bytes()), dtype=np.float64)
                            row[f"{label}_snr_db"] = waveform_snr
                            with warnings.catch_warnings():
                                warnings.simplefilter("error", UserWarning)
                                row[f"{label}_stoi"] = float(stoi(clean_array, values, 48000))
                                row[f"{label}_estoi"] = float(stoi(clean_array, values, 48000, extended=True))
                            row[f"{label}_lsd_db"] = spectral_distance(reference, values)
                            row[f"{label}_packet_bytes"] = int(fields["packet_bytes"])
                            row[f"{label}_drain_bytes"] = int(fields["drain_bytes"])
                            if label == "current":
                                for band in (8000, 12000, 24000):
                                    row[f"band_{band}_frames"] = int(fields[f"band_{band}_frames"])
                        for metric in ("snr_db", "stoi", "estoi", "lsd_db"):
                            row[f"delta_{metric}"] = row[f"current_{metric}"] - row[f"baseline_{metric}"]
                        if (row["current_packet_bytes"] != row["baseline_packet_bytes"]
                                or row["current_drain_bytes"] != row["baseline_drain_bytes"]):
                            raise ValueError("paired packet byte budgets differ")
                        for key, value in row.items():
                            if isinstance(value, float):
                                if not math.isfinite(value):
                                    raise ValueError(f"non-finite quality metric: {key}")
                                row[key] = f"{value:.6f}"
                        rows.append(row)
            print(f"evaluated {flac.stem}", flush=True)
    with args.output.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


if __name__ == "__main__":
    main()
