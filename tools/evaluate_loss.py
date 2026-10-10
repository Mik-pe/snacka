#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Compare causal loss concealment with an aligned, uninterrupted decode."""
import argparse
import csv
import hashlib
import math
from pathlib import Path
import random
import subprocess
import tempfile

from evaluate_speech import floats, noisy


def snr(reference, output, indices):
    if not indices:
        return None
    energy = sum(float(reference[i]) ** 2 for i in indices)
    error = sum((float(output[i]) - float(reference[i])) ** 2 for i in indices)
    if energy == 0 or error == 0:
        return None
    return 10 * math.log10(energy / error)


def loss_frames(clip, duration, frames, pattern):
    if pattern.startswith("periodic"):
        burst = int(pattern.removeprefix("periodic"))
        return {i for i in range(frames) if i % (10 * burst) >= 9 * burst}
    generator = random.Random(f"snacka-loss-6716:{clip}:{duration}:{pattern}")
    lost = set()
    bad = False
    for i in range(math.ceil(80 / duration), frames):
        draw = generator.random()
        if pattern == "independent5":
            bad = draw < 0.05
        else:
            leave = duration / (40 if pattern == "burst40" else 80)
            bad = draw >= leave if bad else draw < leave / 9
        if bad:
            lost.add(i)
    return lost


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--roundtrip", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--patterns", choices=("periodic", "seeded"), default="periodic")
    parser.add_argument("--quality", action="store_true", help="add optional STOI, ESTOI and spectral-distance scores")
    parser.add_argument("--bitrates", type=int, nargs="+", default=[24000])
    args = parser.parse_args()
    if any(not 12000 <= value <= 32000 for value in args.bitrates):
        parser.error("bitrates must be within 12000..32000")
    if args.quality:
        import numpy as np
        from evaluate_quality import intelligibility, spectrum, spectral_distance
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
        loss_file = directory / "loss.txt"

        def run(executable, bitrate, duration, losses=False):
            command = [str(executable), str(source), str(output), str(bitrate), str(duration)]
            if losses:
                command.extend(["--loss-frames", str(loss_file)])
            result = subprocess.run(command, check=True, capture_output=True, text=True)
            values = floats(output.read_bytes())
            if len(values) != len(original) or not all(math.isfinite(x) and abs(x) <= 1 for x in values):
                raise ValueError("invalid decoded PCM")
            fields = dict(field.split("=", 1) for field in result.stdout.strip().split(","))
            return values, fields

        for flac in sources:
            subprocess.run(
                [args.ffmpeg, "-v", "error", "-y", "-i", str(flac), "-ar", "48000",
                 "-ac", "1", "-f", "f32le", str(source)], check=True,
            )
            clean = source.read_bytes()
            clean_values = floats(clean)
            if args.quality:
                clean_array = np.asarray(clean_values, dtype=np.float64)
            digest = hashlib.sha256(flac.read_bytes()).hexdigest()
            for condition in ("clean", "noise20", "noise10"):
                data, clipped = (clean, 0) if condition == "clean" else noisy(
                    clean_values, flac.stem, int(condition[5:])
                )
                original = floats(data)
                source.write_bytes(data)
                if args.quality:
                    original_spectrum = spectrum(np.asarray(original, dtype=np.float64))
                for duration in (10, 20):
                    samples = 48 * duration
                    frames = (len(original) + samples - 1) // samples
                    patterns = ("periodic1", "periodic3", "periodic6") if args.patterns == "periodic" else (
                        "independent5", "burst40", "burst80"
                    )
                    for bitrate in args.bitrates:
                        reference, expected_fields = run(baseline, bitrate, duration)
                        reference_pcm = output.read_bytes()
                        if run(current, bitrate, duration) != (reference, expected_fields):
                            raise ValueError("uninterrupted output or packet metadata differs from baseline")
                        if output.read_bytes() != reference_pcm:
                            raise ValueError("uninterrupted PCM bytes differ from baseline")
                        reference_hash = hashlib.sha256(reference_pcm).hexdigest()
                        uninterrupted = {}
                        key = f"{flac.stem}:{condition}:{duration}:{bitrate}"
                        if args.quality:
                            reference_array = np.asarray(reference, dtype=np.float64)
                            uninterrupted.update(intelligibility(clean_array, reference_array, key))
                            uninterrupted["lsd_db"] = spectral_distance(original_spectrum, reference_array)
                        for pattern in patterns:
                            dropped = loss_frames(flac.stem, duration, frames, pattern)
                            mask_text = "".join(f"{i}\n" for i in sorted(dropped))
                            loss_file.write_text(mask_text)
                            # Output alignment removes 120 priming samples; the drain is always received.
                            lost = [i for i in range(len(original)) if (i + 120) // samples in dropped]
                            onset = [i for i in lost if (i + 120) // samples - 1 not in dropped
                                     and (i + 120) % samples < 120]
                            recovery = [i for i in range(len(original))
                                        if (i + 120) // samples not in dropped
                                        and (i + 120) // samples - 1 in dropped
                                        and (i + 120) % samples < 120]
                            received = []
                            for frame in range(frames + 1):
                                if frame in dropped:
                                    continue
                                start = max(frame * samples - 120, 0)
                                if frame - 1 in dropped:
                                    start += 120
                                end = min((frame + 1) * samples - 120, len(original))
                                if start < end:
                                    received.append((start, end))
                            burst = int(pattern.removeprefix("periodic")) if pattern.startswith("periodic") else 0
                            row = dict(
                                clip=flac.stem, flac_sha256=digest, condition=condition,
                                pcm_sha256=hashlib.sha256(data).hexdigest(), reference_pcm_sha256=reference_hash,
                                loss_frames_sha256=hashlib.sha256(mask_text.encode()).hexdigest(),
                                clipped_samples=clipped, samples=len(original), bitrate=bitrate,
                                frame_ms=duration, pattern=pattern, period=10 * burst, burst=burst,
                                total_frames=frames, lost_frames=len(dropped),
                                lost_samples=len(lost), onset_samples=len(onset), recovery_samples=len(recovery),
                                verified_received_samples=sum(end - start for start, end in received),
                            )
                            row.update({f"uninterrupted_{key}": value for key, value in uninterrupted.items()})
                            for label, executable in (("baseline", baseline), ("current", current)):
                                values, fields = run(executable, bitrate, duration, True)
                                decoded_pcm = output.read_bytes()
                                if any(decoded_pcm[4 * start:4 * end] != reference_pcm[4 * start:4 * end]
                                       for start, end in received):
                                    raise ValueError("loss error propagated beyond the short recovery overlap")
                                if int(fields["lost_frames"]) != len(dropped):
                                    raise ValueError("roundtrip did not apply the requested loss frames")
                                for field in ("packet_bytes", "drain_bytes"):
                                    if fields[field] != expected_fields[field]:
                                        raise ValueError("packet byte budget changed")
                                    row[f"{label}_{field}"] = int(fields[field])
                                for metric, indices in (("lost", lost), ("onset", onset), ("recovery", recovery),
                                                        ("whole", range(len(original)))):
                                    row[f"{label}_{metric}_snr_db"] = snr(reference, values, indices)
                                if args.quality:
                                    array = np.asarray(values, dtype=np.float64)
                                    for metric, value in intelligibility(clean_array, array, key).items():
                                        row[f"{label}_{metric}"] = value
                                    row[f"{label}_lsd_db"] = spectral_distance(original_spectrum, array)
                            for metric in ("lost", "onset", "recovery", "whole"):
                                before, after = (row[f"{label}_{metric}_snr_db"] for label in ("baseline", "current"))
                                row[f"delta_{metric}_db"] = None if before is None or after is None else after - before
                            if args.quality:
                                for metric in ("stoi", "estoi", "lsd_db"):
                                    row[f"delta_{metric}"] = row[f"current_{metric}"] - row[f"baseline_{metric}"]
                                    row[f"loss_delta_{metric}"] = row[f"current_{metric}"] - uninterrupted[metric]
                            for field, value in row.items():
                                if isinstance(value, float):
                                    if not math.isfinite(value):
                                        raise ValueError(f"non-finite metric: {field}")
                                    row[field] = f"{value:.6f}"
                            rows.append(row)
            print(f"evaluated {flac.stem}", flush=True)
    with args.output.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


if __name__ == "__main__":
    main()
