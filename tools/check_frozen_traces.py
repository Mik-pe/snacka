#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Check CPU-probe packet/PCM traces against a frozen CSV, without timing."""
import argparse
import csv
import hashlib
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from evaluate_speech import floats, noisy

DELAY = 120
HASH_FIELDS = ("flac_sha256", "pcm_sha256", "trace_sha256", "decoded_pcm_sha256")
COUNT_FIELDS = ("frame_ms", "bitrate", "samples", "clipped_samples", "packet_bytes",
                "drain_bytes", "frames", "harmonic_frames")
REQUIRED_FIELDS = {"clip", "condition", *HASH_FIELDS, *COUNT_FIELDS}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def reference_rows(path):
    with path.open(newline="", encoding="utf-8") as source:
        reader = csv.DictReader(source)
        fields = reader.fieldnames or []
        if len(fields) != len(set(fields)) or not REQUIRED_FIELDS.issubset(fields):
            raise ValueError("reference has duplicate or missing columns")
        expected = {}
        for number, row in enumerate(reader, start=2):
            if None in row or any(value is None or value == "" for value in row.values()):
                raise ValueError(f"reference row {number} has missing or extra cells")
            for field in HASH_FIELDS:
                value = row[field]
                if len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
                    raise ValueError(f"reference row {number}: invalid {field}")
            for field in COUNT_FIELDS:
                value = row[field]
                if not value.isascii() or not value.isdecimal():
                    raise ValueError(f"reference row {number}: invalid {field}")
                row[field] = int(value)
            duration, bitrate = row["frame_ms"], row["bitrate"]
            if duration not in (10, 20) or not 12000 <= bitrate <= 32000:
                raise ValueError(f"reference row {number}: unsupported duration or bitrate")
            if row["condition"] not in ("clean", "noise20", "noise10") or not row["clip"]:
                raise ValueError(f"reference row {number}: unsupported condition or empty clip")
            samples = row["samples"]
            frames = (samples + 48 * duration - 1) // (48 * duration)
            divisor = 8000 // duration
            if (samples == 0 or row["frames"] != frames
                    or row["clipped_samples"] > samples or row["harmonic_frames"] > frames
                    or row["packet_bytes"] != frames * bitrate // divisor
                    or row["drain_bytes"] != (frames + 1) * bitrate // divisor - row["packet_bytes"]):
                raise ValueError(f"reference row {number}: inconsistent sample/frame/byte counts")
            key = (row["clip"], row["condition"], duration, bitrate)
            if key in expected:
                raise ValueError(f"duplicate reference key: {key}")
            expected[key] = row
    if not expected:
        raise ValueError("reference contains no rows")
    return expected


def trace_result(trace, duration, bitrate, input_samples):
    position = 0
    decoded = bytearray()
    packet_bytes = drain_bytes = frames = harmonic_frames = drains = 0
    quota = 0
    divisor = 8000 // duration
    while position < len(trace):
        if len(trace) - position < 2:
            raise ValueError("truncated trace packet length")
        length = int.from_bytes(trace[position:position + 2], "little")
        position += 2
        quota += bitrate
        allocated, quota = divmod(quota, divisor)
        if length != allocated or length > len(trace) - position:
            raise ValueError("trace packet length differs from byte budget or available data")
        packet = trace[position:position + length]
        position += length
        if length < 4 or packet[0] >> 5 != 7 or packet[0] & 15 in (6, 7):
            raise ValueError("invalid trace version or profile")
        if packet[0] & 1 != int(duration == 10) or packet[1] != frames % 256:
            raise ValueError("trace frame duration or sequence differs")
        final = bool(packet[0] & 16)
        count = DELAY if final else 48 * duration
        if len(trace) - position < count * 4:
            raise ValueError("truncated trace decoded PCM")
        output = trace[position:position + count * 4]
        position += count * 4
        if any(not math.isfinite(value) or abs(value) > 1 for value in floats(output)):
            raise ValueError("trace contains non-finite or out-of-range PCM")
        decoded.extend(output)
        if final:
            drain_bytes = length
            drains += 1
            if position != len(trace):
                raise ValueError("trace contains data after the drain")
        else:
            packet_bytes += length
            frames += 1
            harmonic_frames += int(packet[0] & 14 == 14)
    expected_frames = (input_samples + 48 * duration - 1) // (48 * duration)
    if drains != 1 or frames != expected_frames:
        raise ValueError("trace must contain every input frame and exactly one 120-sample drain")
    aligned = decoded[DELAY * 4:(DELAY + input_samples) * 4]
    if len(aligned) != input_samples * 4:
        raise ValueError("trace does not cover the input after removing 120 priming samples")
    return dict(trace_sha256=digest(trace), decoded_pcm_sha256=digest(aligned),
                packet_bytes=packet_bytes, drain_bytes=drain_bytes,
                frames=frames, harmonic_frames=harmonic_frames)


def write_csv(path, rows):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", newline="", encoding="utf-8",
                                         dir=path.parent, prefix=".snacka-traces-", delete=False) as output:
            temporary = Path(output.name)
            writer = csv.DictWriter(output, fieldnames=list(rows[0]), lineterminator="\n")
            writer.writeheader()
            writer.writerows(rows)
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path, help="directory of original mono FLAC files")
    parser.add_argument("reference", type=Path, help="frozen trace CSV")
    parser.add_argument("output", type=Path, help="CSV replaced only after every row matches")
    parser.add_argument("--current", type=Path, required=True, help="CPU-probe executable")
    parser.add_argument("--harmonic", action="store_true", help="pass --harmonic to the current probe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--diagnostics", type=Path,
                        help="retain failed input/trace in a new subdirectory here; default discards them")
    args = parser.parse_args()
    current = args.current.resolve(strict=True)
    expected = reference_rows(args.reference)
    clips = {path.stem: path for path in args.corpus.glob("*.flac")}
    missing = {key[0] for key in expected} - clips.keys()
    if missing:
        raise ValueError(f"corpus is missing reference clips: {sorted(missing)}")
    protected = {current, args.reference.resolve(), *(path.resolve() for path in clips.values())}
    if args.output.resolve() in protected:
        raise ValueError("output must not overwrite the reference, executable or corpus")
    executable_hash = digest(current.read_bytes())
    rows = []
    with tempfile.TemporaryDirectory(prefix="snacka-frozen-traces-") as temporary:
        root = Path(temporary)
        pcm = root / "input.f32"
        trace_file = root / "current.trace"
        for name in sorted({key[0] for key in expected}):
            flac = clips[name]
            source_hash = digest(flac.read_bytes())
            subprocess.run([args.ffmpeg, "-v", "error", "-y", "-i", str(flac), "-ar", "48000",
                            "-ac", "1", "-f", "f32le", str(pcm)], check=True)
            clean = pcm.read_bytes()
            if not clean or len(clean) % 4:
                raise ValueError(f"{name}: invalid converted PCM length")
            clean_values = floats(clean)
            if any(not math.isfinite(value) or abs(value) > 1 for value in clean_values):
                raise ValueError(f"{name}: invalid converted PCM values")
            for condition in ("clean", "noise20", "noise10"):
                keys = sorted(key for key in expected if key[:2] == (name, condition))
                if not keys:
                    continue
                data, clipped = (clean, 0) if condition == "clean" else noisy(
                    clean_values, name, int(condition[5:]))
                pcm.write_bytes(data)
                input_hash = digest(data)
                for key in keys:
                    _, _, duration, bitrate = key
                    frozen = expected[key]
                    trace_file.unlink(missing_ok=True)
                    try:
                        for field, value in (("flac_sha256", source_hash), ("pcm_sha256", input_hash),
                                             ("samples", len(data) // 4), ("clipped_samples", clipped)):
                            if value != frozen[field]:
                                raise ValueError(f"{field}: {value} != frozen {frozen[field]}")
                        command = [str(current), "trace", str(pcm), str(trace_file),
                                   str(bitrate), str(duration)]
                        if args.harmonic:
                            command.append("--harmonic")
                        subprocess.run(command, check=True)
                        result = trace_result(trace_file.read_bytes(), duration, bitrate, len(data) // 4)
                        for field, value in result.items():
                            if value != frozen[field]:
                                raise ValueError(f"{field}: {value} != frozen {frozen[field]}")
                    except (OSError, ValueError, subprocess.CalledProcessError) as error:
                        if args.diagnostics:
                            args.diagnostics.mkdir(parents=True, exist_ok=True)
                            diagnostic = Path(tempfile.mkdtemp(prefix="snacka-trace-mismatch-",
                                                             dir=args.diagnostics))
                            (diagnostic / "input.f32").write_bytes(data)
                            if trace_file.exists():
                                (diagnostic / "current.trace").write_bytes(trace_file.read_bytes())
                            (diagnostic / "failure.txt").write_text(f"{key}: {error}\n", encoding="utf-8")
                            print(f"Diagnostics retained in {diagnostic}", file=sys.stderr)
                        raise RuntimeError(f"{key}: {error}") from error
                    rows.append(dict(clip=name, flac_sha256=source_hash, pcm_sha256=input_hash,
                                     condition=condition, frame_ms=duration, bitrate=bitrate,
                                     samples=len(data) // 4, clipped_samples=clipped,
                                     current_exe_sha256=executable_hash, harmonic_enabled=args.harmonic,
                                     **result))
            print(f"exact {name}", flush=True)
    if len(rows) != len(expected):
        raise RuntimeError("verified row count differs from the reference")
    write_csv(args.output, rows)


if __name__ == "__main__":
    main()
