#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Record decode process totals and estimate steady costs at two frame counts."""
import argparse
import csv
import hashlib
import subprocess
import tempfile
from pathlib import Path

from measure_cpu import measured


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_table(path, rows):
    with path.open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("output_prefix", type=Path)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--current", type=Path, required=True)
    parser.add_argument("--bitrate", type=int, default=12000)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--short-frames", type=int, default=20000)
    parser.add_argument("--long-frames", type=int, default=40000)
    parser.add_argument("--harmonic", action="store_true",
                        help="enable harmonic speech in the current executable")
    parser.add_argument("--baseline-harmonic", action="store_true",
                        help="enable harmonic speech in the baseline executable")
    parser.add_argument("--require-identical-traces", action="store_true")
    args = parser.parse_args()
    if not 12000 <= args.bitrate <= 32000:
        parser.error("bitrate must be within 12000..32000")
    if args.repetitions <= 0 or not 0 < args.short_frames < args.long_frames:
        parser.error("repetitions must be positive and 0 < short frames < long frames")
    clips = sorted(args.corpus.glob("*.flac"))
    if not clips:
        parser.error("corpus must contain FLAC files")
    executables = {"baseline": args.baseline.resolve(strict=True),
                   "current": args.current.resolve(strict=True)}
    enabled = {"baseline": args.baseline_harmonic, "current": args.harmonic}
    probes = {label: digest(executable) for label, executable in executables.items()}
    source = Path(__file__).resolve()
    evaluator_hash = digest(source)
    timer_hash = digest(source.with_name("measure_cpu.py"))
    raw = []
    slopes = []
    with tempfile.TemporaryDirectory(prefix="snacka-decoder-slopes-") as temporary:
        root = Path(temporary)
        for clip in clips:
            pcm = root / "input.f32"
            subprocess.run([
                "ffmpeg", "-v", "error", "-y", "-i", str(clip), "-ar", "48000",
                "-ac", "1", "-f", "f32le", str(pcm),
            ], check=True)
            inputs = {"clip": clip.stem, "flac_sha256": digest(clip), "pcm_sha256": digest(pcm)}
            for duration in (10, 20):
                traces = {}
                for label, executable in executables.items():
                    trace = root / f"{label}.trace"
                    subprocess.run([
                        str(executable), "trace", str(pcm), str(trace), str(args.bitrate), str(duration),
                    ] + (["--harmonic"] if enabled[label] else []), check=True)
                    traces[label] = digest(trace)
                identical = traces["baseline"] == traces["current"]
                if args.require_identical_traces and not identical:
                    raise RuntimeError(f"packet/PCM mismatch: {clip.stem}, {duration} ms")
                settings = {
                    **inputs, "frame_ms": duration, "bitrate": args.bitrate,
                    "short_frames": args.short_frames, "long_frames": args.long_frames,
                    "baseline_harmonic": args.baseline_harmonic, "current_harmonic": args.harmonic,
                    "baseline_trace_sha256": traces["baseline"], "current_trace_sha256": traces["current"],
                    "traces_identical": identical, "baseline_probe_sha256": probes["baseline"],
                    "current_probe_sha256": probes["current"], "slope_source_sha256": evaluator_hash,
                    "measure_source_sha256": timer_hash,
                }
                for repetition in range(args.repetitions):
                    labels = ["baseline", "current"]
                    counts = [args.short_frames, args.long_frames]
                    if repetition % 2:
                        labels.reverse()
                    if repetition % 3:
                        counts.reverse()
                    # An ABBA order brackets the inner pair with the other executable.
                    order = [(labels[0], counts[0]), (labels[1], counts[0]),
                             (labels[1], counts[1]), (labels[0], counts[1])]
                    results = {}
                    for index, (label, frames) in enumerate(order):
                        cpu, wall = measured(executables[label], "decode", pcm, frames,
                                             args.bitrate, duration, enabled[label])
                        results[label, frames] = (cpu, wall)
                        raw.append({
                            **settings, "repetition": repetition, "order": index,
                            "label": label, "frames": frames,
                            "process_cpu_seconds": cpu, "wall_seconds": wall,
                        })
                    row = {**settings, "repetition": repetition}
                    delta = args.long_frames - args.short_frames
                    for label in executables:
                        short = results[label, args.short_frames]
                        long = results[label, args.long_frames]
                        row[f"{label}_cpu_ns_per_frame"] = (long[0] - short[0]) * 1e9 / delta
                        row[f"{label}_wall_ns_per_frame"] = (long[1] - short[1]) * 1e9 / delta
                    baseline = row["baseline_cpu_ns_per_frame"]
                    current = row["current_cpu_ns_per_frame"]
                    # Retain negative slopes and flag them; timer noise must remain visible.
                    row["nonpositive_cpu_slope"] = baseline <= 0 or current <= 0
                    row["cpu_change_percent"] = 100 * (current / baseline - 1) if baseline > 0 else ""
                    slopes.append(row)
            print(f"{clip.stem}: trace hashes recorded; decoder slopes complete", flush=True)
    write_table(Path(str(args.output_prefix) + "-raw.csv"), raw)
    write_table(Path(str(args.output_prefix) + "-slopes.csv"), slopes)
    print(f"raw_runs={len(raw)} slope_pairs={len(slopes)}", flush=True)


if __name__ == "__main__":
    main()
