# SPDX-License-Identifier: MIT
"""Run the selected timings, or --full-validate for the untimed 2,130-case proof."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import statistics
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--full-validate", action="store_true")
args = parser.parse_args()
BASE = Path(os.environ["SNACKA_BENCH_ARTIFACTS"]).resolve()
build = json.loads((BASE / "build.json").read_text())
sys.path.insert(0, str(BASE / "after/tools/reference"))
from benchmark_opus_matrix import generate, geometries
from check_opus_rtp import library

reference = Path(build["reference"]["path"])
assert hashlib.sha256(reference.read_bytes()).hexdigest() == build["reference"]["sha256"]
lib = library(reference)
assert lib.opus_get_version_string().decode() == "libopus 1.6.1"
environment = dict(os.environ, SNACKA_BENCH_QOS="user-initiated")
for name in ("DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"):
    environment[name] = str(reference.parent) + os.pathsep + environment.get(name, "")
cases = [("SILK", "WB", 20, 1, 8000, "decode"), ("SILK", "WB", 20, 2, 48000, "decode"),
         ("SILK", "WB", 20, 1, 48000, "decode"), ("SILK", "WB", 20, 1, 48000, "plc-return"),
         ("SILK", "WB", 20, 1, 48000, "fec-return"), ("SILK", "MB", 20, 1, 8000, "decode"),
         ("SILK", "NB", 20, 1, 48000, "decode"), ("Hybrid", "FB", 20, 2, 48000, "decode"),
         ("Hybrid", "FB", 20, 1, 8000, "decode"), ("CELT", "FB", 2.5, 1, 48000, "decode")]
if args.full_validate:
    cases = [(mode, band, duration, channels, rate, schedule) for mode, band, duration in geometries()
             for channels in (1, 2) for rate in (8000, 12000, 16000, 24000, 48000)
             for schedule in ("decode", "plc-return", "fec-return")]
for role, variant in build["variants"].items():
    assert hashlib.sha256((BASE / f"{role}-decode").read_bytes()).hexdigest() == variant["binary_sha256"]
    assert all(hashlib.sha256((BASE / role / path).read_bytes()).hexdigest() == digest
               for path, digest in variant["sources"].items())
streams = BASE / "streams"
streams.mkdir(exist_ok=True)
output = BASE / ("validation" if args.full_validate else "timing")
output.mkdir(exist_ok=True)
rows = []
for mode, band, duration, channels, rate, schedule in cases:
    name = f"{mode}-{band}-{duration}ms-{channels}ch"
    packet = streams / f"{name}.bit"
    if not packet.exists():
        generate(lib, mode, band, duration, channels, 24, packet)
    label = f"{name}-{rate}-{schedule}"
    repetitions = []
    for repetition in range(1 if args.full_validate else 3):
        pair = {}
        for role in (("before", "after") if repetition % 2 == 0 else ("after", "before")):
            result = subprocess.run([str(BASE / f"{role}-decode"), "matrix", str(rate), str(channels), schedule,
                                     "0" if args.full_validate else "1000", "200000", str(packet)],
                                    env=environment, capture_output=True, text=True, check=True)
            pair[role] = json.loads(result.stdout)
            if not args.full_validate:
                assert pair[role]["all_batches_meet_minimum"]
            (output / f"{label}-{repetition}-{role}.json").write_text(result.stdout)
        assert pair["before"]["owned_pcm_hash"] == pair["after"]["owned_pcm_hash"]
        if args.full_validate:
            assert pair["before"] == pair["after"]
        else:
            repetitions.append({f"p{pct}": pair["after"][f"p{pct}_ns_per_operation"]["ratio"] /
                                 pair["before"][f"p{pct}_ns_per_operation"]["ratio"] - 1 for pct in (50, 95, 99)})
    rows.append({"case": label, "stream_sha256": hashlib.sha256(packet.read_bytes()).hexdigest(),
                 "repetitions": repetitions,
                 "median_changes": {key: statistics.median(rep[key] for rep in repetitions)
                                    for key in ("p50", "p95", "p99")} if repetitions else {}})
    print(f"{len(rows)}/{len(cases)} {label}", flush=True)
(output / "summary.json").write_text(json.dumps(rows, indent=2) + "\n")
