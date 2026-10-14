# SPDX-License-Identifier: MIT
"""Paired complete-decoder benchmark against a separately installed libopus.

Generate deterministic voice packets through the public reference encoder API,
then validate and measure the same packets and loss history on both decoders.
All artifacts remain outside Git. The reference library is used by this tester
only; Snacka's production dependency graph remains entirely owned Rust.

Full coverage: SILK NB/MB/WB 10/20/40/60 ms, Hybrid SWB/FB 10/20/40/60 ms,
CELT NB/WB/SWB/FB 2.5/5/10/20 ms; mono/stereo; all five output rates; normal,
two losses followed by PLC return, and PLC plus FEC followed by normal return.
No geometry failure is silently omitted. Percentiles are batch averages, not
individual frame deadline tails. Full defaults use 1000 alternating paired
batches per case; --quick uses 100 and remains exploratory evidence.
"""

import argparse
import ctypes as ct
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import struct
import subprocess
import sys

from check_opus_rtp import library

RATES = (8000, 12000, 16000, 24000, 48000)
SCHEDULES = ("decode", "plc-return", "fec-return")
BANDS = {"NB": 1101, "MB": 1102, "WB": 1103, "SWB": 1104, "FB": 1105}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources(root):
    files = [root / "Cargo.toml", root / "Cargo.lock"]
    files += sorted((root / "src").rglob("*.rs"))
    files += [root / "tools/reference/benchmark_opus_decode.rs",
              root / "tools/reference/benchmark_opus_matrix.py",
              root / "tools/reference/check_opus_rtp.py"]
    return {str(path.relative_to(root)): sha256(path) for path in files}


def geometries():
    for mode, bands, durations in (
        ("SILK", ("NB", "MB", "WB"), (10, 20, 40, 60)),
        ("Hybrid", ("SWB", "FB"), (10, 20, 40, 60)),
        ("CELT", ("NB", "WB", "SWB", "FB"), (2.5, 5, 10, 20)),
    ):
        for band in bands:
            for duration in durations:
                yield mode, band, duration


def packet_geometry(payload):
    config = payload[0] >> 3
    if config < 12:
        return "SILK", ("NB", "MB", "WB")[config // 4]
    if config < 16:
        return "Hybrid", ("SWB", "FB")[(config - 12) // 2]
    return "CELT", ("NB", "WB", "SWB", "FB")[(config - 16) // 4]


def generate(lib, mode, band, duration, channels, packets, path):
    error = ct.c_int()
    state = lib.opus_encoder_create(48000, channels, 2051 if mode == "CELT" else 2048,
                                    ct.byref(error))
    assert state and error.value == 0
    bitrate = ({"SILK": 32000, "Hybrid": 40000, "CELT": 64000}[mode]) * channels
    controls = [(4002, bitrate), (4004, BANDS[band]), (4008, BANDS[band]),
                (4006, 0), (4010, 10), (4022, channels), (4024, 3001),
                (4012, int(mode != "CELT")), (4014, 30)]
    samples = int(48 * duration)
    data = bytearray()
    actual = {}
    random = 0xA531C872
    try:
        for request, value in controls:
            result = lib.opus_encoder_ctl(state, request, value)
            assert result == 0, (request, value, result)
        for frame in range(packets):
            pcm = (ct.c_float * (samples * channels))()
            for index in range(samples):
                seconds = (frame * samples + index) / 48000
                envelope = 0.3 + 0.7 * math.sin(math.pi * (seconds % 0.19) / 0.19)**2
                voice = sum(math.sin(2 * math.pi * 137 * h * seconds) / h**2
                            for h in range(1, 7))
                for channel in range(channels):
                    random ^= (random << 13) & 0xffffffff
                    random ^= random >> 17
                    random ^= (random << 5) & 0xffffffff
                    noise = (random / 0xffffffff - 0.5) * 0.004
                    pcm[index * channels + channel] = 0.2 * envelope * voice + noise
            encoded = (ct.c_ubyte * 8000)()
            length = lib.opus_encode_float(state, pcm, samples, encoded, len(encoded))
            assert length > 0, length
            payload = bytes(encoded[:length])
            geometry = packet_geometry(payload)
            actual["/".join(geometry)] = actual.get("/".join(geometry), 0) + 1
            assert geometry == (mode, band), (frame, geometry, mode, band)
            config, frame_code = payload[0] >> 3, payload[0] & 3
            frame_count = (payload[1] & 63) if frame_code == 3 else (1 if frame_code == 0 else 2)
            frame_ms = ((10, 20, 40, 60)[config % 4] if config < 12 else
                        ((10, 20)[config % 2] if config < 16 else (2.5, 5, 10, 20)[config % 4]))
            assert frame_count * frame_ms == duration, (frame, "actual duration", frame_count * frame_ms)
            assert 1 + ((payload[0] >> 2) & 1) == channels, (frame, "coded channels")
            interval = ct.c_uint32()
            assert lib.opus_encoder_ctl(state, 4031, ct.byref(interval)) == 0
            data += struct.pack("!II", length, interval.value) + payload
        path.write_bytes(data)
        return {"bitrate": bitrate, "packet_sha256": sha256(path),
                "generated_packet_geometries": actual}
    finally:
        lib.opus_encoder_destroy(state)


def build(root, destination, library_path):
    target = destination / "target"
    environment = {**os.environ, "CARGO_TARGET_DIR": str(target)}
    subprocess.run(["cargo", "build", "--release", "--features", "opus", "--lib"],
                   cwd=root, env=environment, check=True)
    binary = destination / "benchmark-opus-decode"
    subprocess.run(["rustc", "--edition=2024", "-O", "-C", "overflow-checks=yes",
                    str(root / "tools/reference/benchmark_opus_decode.rs"),
                    "--extern", f"snacka={target / 'release/libsnacka.rlib'}",
                    "-L", f"dependency={target / 'release/deps'}",
                    "-L", f"native={library_path.parent}", "-o", str(binary)], check=True)
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path, help="installed public libopus shared library")
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path, help="reuse previously built oracle")
    parser.add_argument("--quick", action="store_true", help="100 batches; exploratory only")
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--require-parity", action="store_true",
                        help="fail unless the full frozen-source timed matrix is no slower at p50/p95/p99")
    parser.add_argument("--packets", type=int, default=24)
    parser.add_argument("--case", action="append", help="explicit geometry filter, e.g. SILK/WB/20; partial coverage")
    args = parser.parse_args()
    if args.require_parity and (args.quick or args.validate_only or args.case):
        parser.error("--require-parity requires the full timed matrix, without --quick, --validate-only or --case")
    root = Path(__file__).resolve().parents[2]
    destination = args.output_dir.resolve()
    if destination == root or root in destination.parents:
        parser.error("output directory must be outside the repository")
    if args.packets < 12:
        parser.error("at least 12 packets are needed for the fixed loss schedule")
    destination.mkdir(parents=True, exist_ok=True)
    library_path = args.library.resolve()
    initial_sources = sources(root)
    binary = args.binary.resolve() if args.binary else build(root, destination, library_path)
    if sources(root) != initial_sources:
        parser.error("source changed while building; rebuild a coherent source snapshot")
    provenance_path = binary.with_suffix(".sources.json")
    provenance = {"sources": initial_sources, "binary_sha256": sha256(binary),
                  "reference_sha256": sha256(library_path)}
    if args.binary:
        if not provenance_path.is_file() or json.loads(provenance_path.read_text()) != provenance:
            parser.error("supplied binary lacks matching source/reference provenance; build it with this runner")
    else:
        provenance_path.write_text(json.dumps(provenance, indent=2) + "\n")
    lib = library(library_path)
    reference_environment = {**os.environ}
    for key in ("DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"):
        reference_environment[key] = str(library_path.parent) + os.pathsep + reference_environment.get(key, "")
    batches = 0 if args.validate_only else (100 if args.quick else 1000)
    rows, failures, generated = [], [], []
    geometry_list = list(geometries())
    if args.case:
        selected = set(args.case)
        geometry_list = [g for g in geometry_list if "/".join(map(str, g)) in selected]
        if len(geometry_list) != len(selected):
            parser.error("unknown or duplicate --case selection")
    planned = len(geometry_list) * 2 * len(RATES) * len(SCHEDULES)
    metadata = {
        "reference": lib.opus_get_version_string().decode(),
        "reference_library": str(library_path), "reference_sha256": sha256(library_path),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "platform": platform.platform(), "machine": platform.machine(),
        "binary_sha256": sha256(binary), "sources": initial_sources,
        "binary_source_binding": "matching build source/reference SHA256 manifest",
        "timed_batches_per_case": batches, "minimum_batch_ns": 200000,
        "coverage": "partial explicit selection" if args.case else "complete generated geometry matrix",
        "evidence": "validation only" if args.validate_only else ("exploratory quick" if args.quick else "paired performance"),
        "planned_cases": planned, "generated_packets_per_stream": args.packets,
        "loss_schedule": "packet8 PLC; packet9 PLC or recovery from packet10; packet10 ordinary decode on same decoder",
        "percentile_scope": "ordered continuous cached-stream batch averages; not individual frame tails",
        "quality_scope": "sample counts, finite PCM and ordinary packet entropy intervals are validated; reported unshifted PCM error is diagnostic, not a quality acceptance gate; published vector conformance is checked separately at 48 kHz",
    }
    with (destination / "rows.jsonl").open("w") as output:
        for mode, band, duration in geometry_list:
            for channels in (1, 2):
                name = f"{mode}-{band}-{duration}ms-{channels}ch"
                path = destination / f"{name}.bit"
                try:
                    packet_metadata = generate(lib, mode, band, duration, channels, args.packets, path)
                    generated.append({"case": name, **packet_metadata})
                except (AssertionError, OSError) as error:
                    failures.append({"case": name, "stage": "public reference generation", "error": repr(error),
                                     "affected_cases": len(RATES) * len(SCHEDULES)})
                    print(f"FAIL generation {name}: {error}", file=sys.stderr, flush=True)
                    continue
                for rate in RATES:
                    for schedule in SCHEDULES:
                        case = f"{name}-{rate}Hz-{schedule}"
                        result = subprocess.run([str(binary), "matrix", str(rate), str(channels),
                                                 schedule, str(batches), "200000", str(path)],
                                                capture_output=True, text=True, env=reference_environment)
                        try:
                            if result.returncode:
                                raise RuntimeError(result.stderr.strip())
                            row = json.loads(result.stdout)
                            if row["libopus_version"] != metadata["reference"]:
                                raise RuntimeError("packet generator and timing bridge loaded different reference versions")
                            row.update({"case": case, "requested_mode": mode, "requested_bandwidth": band,
                                        "packet_duration_ms": duration, **packet_metadata})
                            if batches and not row["all_batches_meet_minimum"]:
                                raise RuntimeError("measured batch below 200us minimum")
                            rows.append(row)
                            output.write(json.dumps(row) + "\n")
                            output.flush()
                            print(f"{len(rows)}/{planned} {case}", file=sys.stderr, flush=True)
                        except (ValueError, KeyError, RuntimeError) as error:
                            failures.append({"case": case, "stage": "validation/timing", "error": str(error),
                                             "stdout": result.stdout, "stderr": result.stderr})
                            print(f"FAIL {case}: {error}", file=sys.stderr, flush=True)
    unchanged = sources(root) == initial_sources
    slower = [{"case": row["case"], "ratios": {f"p{pct}": row[f"p{pct}_ns_per_operation"]["ratio"]
                                               for pct in (50, 95, 99)}} for row in rows
              if batches and any(row[f"p{pct}_ns_per_operation"]["ratio"] > 1 for pct in (50, 95, 99))]
    observed_not_slower = bool(batches) and not failures and not slower
    metadata.update({"source_unchanged": unchanged, "completed_cases": len(rows),
                     "generated_streams": generated, "failures": failures,
                     "slower_cases": slower, "observed_selected_cases_not_slower": observed_not_slower,
                     "all_cases_not_slower": observed_not_slower and unchanged and not args.quick and not args.case,
                     "actual_fec_cases": sum(row["fec_events"] > 0 for row in rows),
                     "complete": len(rows) == planned and not failures and unchanged})
    (destination / "summary.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(json.dumps({key: metadata[key] for key in ("planned_cases", "completed_cases", "complete",
                                                   "source_unchanged", "actual_fec_cases", "all_cases_not_slower")}, indent=2))
    passed = metadata["complete"] and (not args.require_parity or metadata["all_cases_not_slower"])
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
