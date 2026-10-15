#!/usr/bin/env python3
"""Untimed mapped-controls complete-JSON replay against accepted phase567d, source-equal to963b903.

Preparation only until root separately authorizes the exact candidate proof.
No build, input generation, calibration or timing command exists in this file.
--check-plan reads/hashes only. --proof-run-confirmed executes matrix BATCHES=0.
"""
import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys

HERE = Path(__file__).resolve().parent
BASELINE_SHA = "cfa52de680990e051366b83cd47836fa8bbe477742d1563ad1ec9b0ac8f23693"
BASE_COMMIT = "963b903173ff3a55cb60cfa22b189c5d76e78d48"
CHANGED_PATHS = ["src/opus/encoder.rs", "src/opus/multistream.rs", "src/opus/projection.rs"]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def read_json(path):
    def reject(value):
        raise ValueError("Non-finite JSON constant: " + value)
    return json.loads(Path(path).read_text(), parse_constant=reject)


def dump(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def pinned(record):
    require(sha(record["path"]) == record["sha256"], "Hash changed: " + record["path"])


def sources(directory, expected):
    directory = Path(directory)
    members = {str(p.relative_to(directory)) for p in (directory / "src").rglob("*") if p.is_file()}
    members.update(["Cargo.toml", "Cargo.lock"])
    require(members == set(expected), "Production source membership changed")
    for name, digest in expected.items():
        require(sha(directory / name) == digest, "Source changed: " + name)


def bindings(candidate_path, candidate_hash):
    require(sha(HERE / "baseline-binding.json") == BASELINE_SHA, "Baseline binding changed")
    before = read_json(HERE / "baseline-binding.json")
    pinned(before["manifest"])
    sources(before["source_directory"], before["source_files"])
    for key in ["library", "binary", "bridge", "retained_rows", "retained_summary", "cases", "inputs"]:
        pinned(before[key])
    require(sha(before["actual_bridge_source"]) == before["bridge"]["sha256"], "Baseline compiled bridge source changed")
    for path, digest in before["build_logs"].items():
        require(sha(path) == digest, "Baseline log changed: " + path)
    require(sha(candidate_path) == candidate_hash, "Candidate manifest changed")
    after = read_json(candidate_path)
    require(after.get("baseline_main") == BASE_COMMIT, "Candidate base must be frozen public963b903")
    require(after["baseline_manifest"]["sha256"] == before["manifest"]["sha256"], "Candidate accepted-before binding differs")
    require(len(after.get("candidate_patch_sha256", "")) == 64, "Frozen candidate patch identity is required")
    after["_proof_source_identity"] = after.get("source_commit") or "frozen-patch:" + after["candidate_patch_sha256"]
    sources(after["source_directory"], after["source_files"])
    require(set(after["source_files"]) == set(before["source_files"]), "Candidate source membership differs")
    changed = [p for p in before["source_files"] if before["source_files"][p] != after["source_files"][p]]
    require(changed == CHANGED_PATHS, "Candidate changes must be the three reviewed mapped-control owners only: " + repr(changed))
    for key in ["library", "binary"]:
        pinned(after[key])
    bridge = Path(after["source_directory"]) / "tools/reference/benchmark_opus_decode.rs"
    require(sha(bridge) == before["bridge"]["sha256"], "Common bridge must remain unchanged")
    require(after["library_command"] == before["library_command"], "Cargo build arguments differ")
    rustc = before["tools"]["rustc"]["path"]
    expected_bridge = [rustc, "--edition=2024", "-O", "-C", "overflow-checks=yes", str(bridge),
                       "--extern", "snacka=" + after["library"]["path"], "-L",
                       "dependency=" + str(Path(after["library"]["path"]).parent / "deps"),
                       "-L", "native=/opt/homebrew/lib", "-o", after["binary"]["path"]]
    require(after["bridge_command"] == expected_bridge, "Direct bridge flags/path differ")
    for label, manifest in [("before", before), ("after", after)]:
        for name, tool in manifest["tools"].items():
            require(tool["sha256"] == before["tools"][name]["sha256"], "Toolchain differs")
            pinned(tool)
        native = manifest["native"]
        require(native["sha256"] == before["native"]["sha256"], "Reference library differs")
        require(Path(native["requested_path"]).resolve() == Path(native["real_path"]), "Reference symlink changed")
        require(sha(native["real_path"]) == native["sha256"], "Reference bytes changed")
    log_path = Path(candidate_path).parent / "library-build.log"
    require(after["logs"].get("library-build.log") == sha(log_path), "Candidate build log must be pinned")
    commands = [line for line in log_path.read_text().splitlines() if "Running" in line and "--crate-name snacka" in line]
    require(len(commands) == 1, "One actual Cargo rustc command is required")
    actual = commands[0]
    for flag in ["-C opt-level=3", "-C codegen-units=16", "-C overflow-checks=on", "-C strip=debuginfo"]:
        require(flag in actual, "Candidate Cargo profile flag missing: " + flag)
    require(not any(flag in actual for flag in ["-C lto=", "-C target-cpu=", "-C target-feature="]),
            "Candidate must retain default local ThinLTO/CPU/ISA, without explicit lto=off")
    cases = read_json(before["cases"]["path"])
    inputs = read_json(before["inputs"]["path"])
    require(len(cases) == 2130 and len(inputs) == 142, "Full fixed matrix is required")
    for item in inputs:
        pinned(item)
    return before, after, cases, {item["name"]: item for item in inputs}


def finite(value):
    if isinstance(value, float):
        return math.isfinite(value)
    if isinstance(value, dict):
        return all(finite(v) for v in value.values())
    if isinstance(value, list):
        return all(finite(v) for v in value)
    return True


def terminate_owned(child):
    actions = []
    if child is not None and child.poll() is None:
        child.terminate()
        actions.append("SIGTERM_own_child")
        try:
            child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            child.kill()
            actions.append("SIGKILL_own_child")
            child.wait(timeout=2)
    return actions


def run(args, before, after, cases, inputs):
    baseline = [json.loads(line) for line in Path(before["retained_rows"]["path"]).read_text().splitlines()]
    require(len(baseline) == 2130, "Retained rows incomplete")
    for i, row in enumerate(baseline):
        require(row["index"] == i and row["case"] == cases[i] and row["status"] == "EXACT", "Retained row identity/status mismatch")
        require(row["result"]["timed_batches"] == 0 and finite(row["result"]), "Retained rows must be finite and untimed")
    out = args.output.resolve()
    out.mkdir()  # Refuse previous/partial results. No retry/resume mode.
    raw = out / "raw"
    raw.mkdir()
    environment = os.environ.copy()
    for key in list(environment):
        if key.startswith(("DYLD_", "Malloc")) or key in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUST_BACKTRACE"]:
            environment.pop(key, None)
    environment.update(SNACKA_BENCH_QOS="user-initiated", MallocNanoZone="0", RUST_LOG="warn")
    receipt = {"kind": "UNTIMED_COMPLETE_JSON_COMPARISON", "started_utc": utc(),
               "before_compiled_commit": before["compiled_source_commit"], "base_equivalent_commit": BASE_COMMIT,
               "candidate_source_commit": after["_proof_source_identity"], "candidate_manifest": str(args.candidate_manifest),
               "candidate_manifest_sha256": args.candidate_manifest_sha256, "baseline_binding_sha256": BASELINE_SHA,
               "runner_sha256": sha(__file__), "expected": 2130, "attempted": 0, "exact": 0, "failed": 0,
               "workers": 1, "timing_warmup_calibration": False, "inputs_generated": False,
               "cases_with_actual_fec": 0, "actual_fec_events": 0, "recovery_calls": 0, "fallback_calls": 0,
               "status": "RUNNING", "failures": []}
    dump(out / "binding-before-run.json", receipt)
    child = None
    error = None
    sink = (out / "rows.jsonl").open("x")
    def stop(number, frame):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        raise SystemExit(128 + number)
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        for i, case in enumerate(cases):
            command = [after["binary"]["path"], "matrix", str(case["rate"]), str(case["channels"]),
                       case["schedule"], "0", "200000", inputs[case["stream"]]["path"]]
            stdout_path, stderr_path = raw / f"{i:04d}.stdout", raw / f"{i:04d}.stderr"
            row = {"index": i, "case": case, "command": command, "started_utc": utc(), "status": "RUNNING", "cleanup": []}
            try:
                with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
                    child = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=environment)
                    row["pid"] = child.pid
                    try:
                        child.wait(timeout=90)
                    except subprocess.TimeoutExpired:
                        row["status"] = "TIMEOUT_NO_RETRY"
                        row["cleanup"] += terminate_owned(child)
                row["return_code"] = child.returncode
                require(row["status"] != "TIMEOUT_NO_RETRY", "Child validation timeout")
                require(child.returncode == 0, "Child validation exit: " + str(child.returncode))
                value = read_json(stdout_path)
                row["result"] = value
                require(value["kind"] == "paired_decoder_matrix" and value["timed_batches"] == 0, "Wrong protocol")
                require([value["sample_rate"], value["channels"], value["schedule"]] == [case["rate"], case["channels"], case["schedule"]], "Wrong geometry")
                require(value["all_sample_counts_match"] and value["ordinary_and_plc_final_ranges_match"] and finite(value), "Validation/finite-PCM failure")
                require(not any(k in value for k in ["owned_batch_ns", "reference_batch_ns", "minimum_observed_batch_ns", "p50_ns_per_operation"]), "Timed output is forbidden")
                expected = baseline[i]["result"]
                row["changed_keys"] = [k for k in sorted(set(value) | set(expected)) if value.get(k) != expected.get(k)]
                require(value == expected, "Full parsed JSON differs: " + repr(row["changed_keys"]))
                row["status"] = "EXACT"
                fec = value["fec_events"]
                calls = len(value["recovery_intervals"])
                receipt["cases_with_actual_fec"] += int(fec > 0)
                receipt["actual_fec_events"] += fec
                receipt["recovery_calls"] += calls
                receipt["fallback_calls"] += calls - fec
            except Exception as problem:
                if row["status"] == "RUNNING":
                    row["status"] = "FAILED_NO_RETRY"
                row["error_type"], row["error"] = type(problem).__name__, str(problem)
            finally:
                row["cleanup"] += terminate_owned(child)
                if child is not None:
                    row["return_code"] = child.returncode
                child = None
                if row["status"] == "RUNNING":
                    row["status"] = "INTERRUPTED_NO_RETRY"
                row["finished_utc"] = utc()
                row["raw_files"] = {str(p): {"sha256": sha(p), "bytes": p.stat().st_size} for p in [stdout_path, stderr_path] if p.exists()}
                sink.write(json.dumps(row, separators=(",", ":"), allow_nan=False) + "\n")
                sink.flush()
                receipt["attempted"] += 1
                if row["status"] == "EXACT":
                    receipt["exact"] += 1
                else:
                    receipt["failed"] += 1
                    receipt["failures"].append({"case_id": case["id"], "status": row["status"], "error": row.get("error")})
                if receipt["attempted"] % 100 == 0:
                    dump(out / "progress.json", receipt)
                    print(receipt["attempted"], "/2130 exact", receipt["exact"], "failed", receipt["failed"], flush=True)
    except BaseException as problem:
        error = problem
        receipt.update(error_type=type(problem).__name__, error=str(problem))
    finally:
        terminate_owned(child)
        sink.close()
        try:
            bindings(args.candidate_manifest, args.candidate_manifest_sha256)
            require(sha(__file__) == receipt["runner_sha256"], "Proof reader changed during replay")
            receipt["integrity_unchanged"] = True
        except Exception as problem:
            receipt.update(integrity_unchanged=False, integrity_error=str(problem))
        receipt["not_attempted_case_ids"] = [c["id"] for c in cases[receipt["attempted"]:]]
        receipt["all_owned_children_reaped"] = child is None
        receipt["finished_utc"] = utc()
        receipt["rows_sha256"] = sha(out / "rows.jsonl")
        receipt["status"] = "COMPLETE_EXACT" if error is None and receipt.get("integrity_unchanged") and receipt["exact"] == 2130 and receipt["failed"] == 0 else "INCOMPLETE_OR_FAILED"
        dump(out / "summary.json", receipt)
    print(json.dumps(receipt, allow_nan=False), flush=True)
    if receipt["status"] != "COMPLETE_EXACT":
        raise SystemExit(1)


def main():
    require(sys.flags.optimize == 0, "Python optimization must be disabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-manifest", type=Path, required=True)
    parser.add_argument("--candidate-manifest-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check-plan", action="store_true")
    mode.add_argument("--proof-run-confirmed", action="store_true")
    args = parser.parse_args()
    before, after, cases, inputs = bindings(args.candidate_manifest, args.candidate_manifest_sha256)
    if args.check_plan:
        print("PINS PASS; no codec/build/calibration/timing executed")
    else:
        run(args, before, after, cases, inputs)


if __name__ == "__main__":
    main()
