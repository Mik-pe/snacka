#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Compare fixed-frame process CPU on speech, recording packet/PCM trace identity."""
import argparse
import csv
import hashlib
import resource
import subprocess
import tempfile
import time
from pathlib import Path


def measured(executable, mode, pcm, frames, duration):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    result = subprocess.run(
        [str(executable), mode, str(pcm), str(frames), "24000", str(duration)],
        capture_output=True, text=True, check=True,
    )
    wall = time.perf_counter() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if result.stdout.strip() != f"frames={frames}":
        raise RuntimeError("probe did not complete the requested frames")
    cpu = after.ru_utime - before.ru_utime + after.ru_stime - before.ru_stime
    if cpu <= 0:
        raise RuntimeError("CPU timer resolution is insufficient; increase frame counts")
    return cpu, wall


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--current", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--encode-frames", type=int, default=50000)
    parser.add_argument("--decode-frames", type=int, default=200000)
    parser.add_argument("--operations", nargs="+", choices=("encode", "decode", "loss", "burst"),
                        default=["encode", "decode"])
    parser.add_argument("--allow-trace-changes", action="store_true",
                        help="measure a separately evaluated codec change without requiring identical traces")
    args = parser.parse_args()
    if min(args.repetitions, args.encode_frames, args.decode_frames) <= 0:
        parser.error("repetitions and frame counts must be positive")
    args.current = args.current.resolve(strict=True)
    args.baseline = args.baseline.resolve(strict=True)
    clips = sorted(args.corpus.glob("*.flac"))
    if not clips:
        parser.error("corpus must contain FLAC files")
    rows = []
    with tempfile.TemporaryDirectory(prefix="snacka-cpu-") as temporary:
        root = Path(temporary)
        for clip in clips:
            pcm = root / "input.f32"
            subprocess.run([
                "ffmpeg", "-v", "error", "-y", "-i", str(clip), "-ar", "48000",
                "-ac", "1", "-f", "f32le", str(pcm),
            ], check=True)
            hashes = {}
            for duration in (10, 20):
                for bitrate in (12000, 24000, 32000):
                    for label, executable in (("baseline", args.baseline), ("current", args.current)):
                        subprocess.run([
                            str(executable), "trace", str(pcm), str(root / f"{label}.trace"),
                            str(bitrate), str(duration),
                        ], check=True)
                    before = (root / "baseline.trace").read_bytes()
                    after = (root / "current.trace").read_bytes()
                    if before != after and not args.allow_trace_changes:
                        raise RuntimeError(f"packet/PCM mismatch: {clip.stem}, {duration} ms, {bitrate}")
                    if bitrate == 24000:
                        hashes[duration] = (hashlib.sha256(before).hexdigest(), hashlib.sha256(after).hexdigest(), before == after)
            for duration in (10, 20):
                for mode in args.operations:
                    frames = args.encode_frames if mode == "encode" else args.decode_frames
                    for repetition in range(args.repetitions):
                        order = [("baseline", args.baseline), ("current", args.current)]
                        if repetition % 2:
                            order.reverse()
                        values = {label: measured(executable, mode, pcm, frames, duration)
                                  for label, executable in order}
                        row = {
                            "clip": clip.stem,
                            "flac_sha256": hashlib.sha256(clip.read_bytes()).hexdigest(),
                            "pcm_sha256": hashlib.sha256(pcm.read_bytes()).hexdigest(),
                            "baseline_trace_sha256": hashes[duration][0],
                            "current_trace_sha256": hashes[duration][1],
                            "traces_identical": hashes[duration][2],
                            "frame_ms": duration, "bitrate": 24000, "operation": mode,
                            "frames": frames, "repetition": repetition,
                        }
                        for label in ("baseline", "current"):
                            row[f"{label}_process_cpu_ns_per_frame"] = f"{values[label][0] * 1e9 / frames:.3f}"
                            row[f"{label}_wall_ns_per_frame"] = f"{values[label][1] * 1e9 / frames:.3f}"
                        row["cpu_change_percent"] = f"{100 * (values['current'][0] / values['baseline'][0] - 1):.4f}"
                        rows.append(row)
            trace_status = "trace hashes recorded" if args.allow_trace_changes else "exact traces match"
            print(f"{clip.stem}: {trace_status}; CPU measurements complete", flush=True)
    with args.output.open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


if __name__ == "__main__":
    main()
