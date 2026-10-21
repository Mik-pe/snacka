#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Extract native PLC statistics on CPU; split membership always comes from manifest.

One deterministic duration/bitrate/mode/noise/gain combination per utterance.
All 1-6 loss horizons share only received past, with no future teacher forcing.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import numpy as np

RECORD = np.dtype([("samples", "<u2"), ("horizon", "u1"), ("harmonic", "u1"), ("values", "<f4", (128,))])


def extract(row, root, binary, stride):
    path = root / row["path"]
    if hashlib.sha256(path.read_bytes()).hexdigest() != row["flac_sha256"]:
        raise ValueError(f"source hash mismatch: {row['path']}")
    key = int(hashlib.sha256(("snacka-plc-v1:" + row["path"]).encode()).hexdigest()[:16], 16)
    bitrate = (12000, 24000, 32000)[key % 3]
    duration = (10, 20)[(key // 3) % 2]
    harmonic = (key // 6) % 2
    condition = (key // 12) % 3
    gain_db = (0, -12, -24)[(key // 36) % 3]
    raw = subprocess.check_output(["ffmpeg", "-v", "error", "-i", str(path), "-ac", "1", "-ar", "48000", "-f", "f32le", "pipe:1"])
    pcm = np.frombuffer(raw, dtype="<f4").copy()
    if condition:
        noise = np.random.default_rng(key).normal(size=len(pcm))
        power = np.mean(pcm.astype(np.float64) ** 2)
        noise *= np.sqrt(power / (np.mean(noise ** 2) * 10 ** ((20 if condition == 1 else 10) / 10)))
        pcm += noise.astype(np.float32)
    pcm *= np.float32(10 ** (gain_db / 20))
    clipped = int(np.count_nonzero(np.abs(pcm) > 1))
    pcm = np.clip(pcm, -1, 1)
    with tempfile.TemporaryDirectory(prefix="snacka-plc-records-") as temporary:
        directory = Path(temporary)
        input_path, output = directory / "input.f32", directory / "records.bin"
        input_path.write_bytes(pcm.astype("<f4").tobytes())
        subprocess.run([str(binary), str(input_path), str(output), str(bitrate), str(duration), str(harmonic), str(stride), str(key % stride)], check=True, capture_output=True)
        data = output.read_bytes()
    if data[:8] != b"SPLC\x01\0\0\0" or (len(data)-8) % RECORD.itemsize:
        raise ValueError("invalid native PLC record framing")
    records = np.frombuffer(data, dtype=RECORD, offset=8)
    values = records["values"]
    if not np.isfinite(values).all():
        raise ValueError("non-finite native record")
    # Exact quadratic sufficient statistics for bandwise MDCT waveform error.
    total = np.maximum(values[:, 80:96].sum(axis=1, keepdims=True)
                       + values[:, 112:128].sum(axis=1, keepdims=True), 1e-10)
    arrays = dict(x=values[:, :80].astype(np.float32),
                  pp=(values[:, 80:96]/total).astype(np.float32),
                  pt=(values[:, 96:112]/total).astype(np.float32),
                  tt=(values[:, 112:128]/total).astype(np.float32),
                  language=np.full(len(records), row["language"] == "sv", dtype=np.uint8),
                  horizon=records["horizon"].copy(), samples=records["samples"].copy())
    meta = {**row, "bitrate": bitrate, "duration_ms": duration, "harmonic": harmonic,
            "condition": ("clean", "noise20", "noise10")[condition], "gain_db": gain_db,
            "seed": key, "clipped_samples": clipped, "records": len(records)}
    return arrays, meta


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--stride", type=int, default=24)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; use a new directory")
    args.output.mkdir(parents=True)
    rows = [json.loads(line) for line in (args.corpus / "manifest.jsonl").read_text().splitlines()]
    written = []
    provenance = {"schema": 2, "statistics_normalization":"sum(prediction_energy + target_energy)", "stride": args.stride, "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                  "corpus_manifest_sha256": hashlib.sha256((args.corpus/"manifest.jsonl").read_bytes()).hexdigest(),
                  "records": {}, "shards": []}
    for split in ("train", "validation"):
        selected = sorted((r for r in rows if r["split"] == split), key=lambda r: r["path"])
        total = 0
        for start in range(0, len(selected), 64):
            batch = selected[start:start+64]
            with ThreadPoolExecutor(max_workers=args.workers) as pool:
                results = list(pool.map(lambda row: extract(row, args.corpus, args.binary.resolve(), args.stride), batch))
            arrays = {key: np.concatenate([r[0][key] for r in results]) for key in results[0][0]}
            name = f"{split}-{start//64:04d}.npz"
            path = args.output/name
            np.savez_compressed(path, **arrays)
            count = len(arrays["x"])
            total += count
            written.extend(r[1] for r in results)
            provenance["shards"].append(dict(path=name, split=split, records=count, sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
            print(split, start+len(batch), "/", len(selected), "records", total, flush=True)
        provenance["records"][split] = total
    (args.output/"utterances.jsonl").write_text("".join(json.dumps(r, sort_keys=True)+"\n" for r in written))
    (args.output/"provenance.json").write_text(json.dumps(provenance, indent=2)+"\n")
    print(json.dumps(provenance["records"]), flush=True)


if __name__ == "__main__":
    main()
