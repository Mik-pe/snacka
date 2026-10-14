#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Offline matched Opus speech quality; libopus is only a measurement oracle.

Input: WAV clips and manifest.json with clips containing clip, speaker and split.
Archive provenance separately. No downloaded corpus is part of Snacka's runtime.
Paired speaker-bootstrap intervals are exploratory evidence, not listening scores.
"""

import argparse
import csv
import ctypes as ct
import hashlib
from importlib.metadata import version as package_version
import json
import math
from pathlib import Path
import platform
import struct
import subprocess
import sys
import tempfile

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from evaluate_quality import intelligibility, spectral_distance, spectrum
from evaluate_speech import floats, noisy, pcm


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_snapshot(root):
    files = [root / "Cargo.toml", root / "Cargo.lock", *sorted((root / "src").rglob("*.rs"))]
    return {str(path.relative_to(root)): digest(path) for path in files}


class Reference:
    """Only documented public float APIs and controls of the supplied library."""

    def __init__(self, path, samples, bandwidth, bitrate, mode="celt"):
        self.lib = lib = ct.CDLL(str(path))
        self.samples = samples
        self.budget = bitrate * samples // 384000
        self.mode = mode
        integer, pointer = ct.c_int, ct.c_void_p
        self.float_pointer = ct.POINTER(ct.c_float)
        byte_pointer = ct.POINTER(ct.c_ubyte)
        definitions = {
            "opus_encoder_create": ([integer, integer, integer, ct.POINTER(integer)], pointer),
            "opus_decoder_create": ([integer, integer, ct.POINTER(integer)], pointer),
            "opus_encoder_destroy": ([pointer], None),
            "opus_decoder_destroy": ([pointer], None),
            "opus_encoder_ctl": ([pointer, integer], integer),
            "opus_decoder_ctl": ([pointer, integer], integer),
            "opus_encode_float": ([pointer, self.float_pointer, integer, byte_pointer, integer], integer),
            "opus_decode_float": ([pointer, byte_pointer, integer, self.float_pointer, integer, integer], integer),
            "opus_packet_get_bandwidth": ([byte_pointer], integer),
            "opus_packet_get_nb_channels": ([byte_pointer], integer),
            "opus_get_version_string": ([], ct.c_char_p),
        }
        for name, (arguments, result) in definitions.items():
            getattr(lib, name).argtypes = arguments
            getattr(lib, name).restype = result
        self.encoder = None
        self.decoders = []
        try:
            error = integer()
            self.encoder = lib.opus_encoder_create(48000, 1, 2051 if mode == "celt" else 2048, ct.byref(error))
            self.check(error.value)
            if not self.encoder:
                raise RuntimeError("null reference encoder")
            for _ in range(2):
                decoder = lib.opus_decoder_create(48000, 1, ct.byref(error))
                self.check(error.value)
                if not decoder:
                    raise RuntimeError("null reference decoder")
                self.decoders.append(decoder)
            self.bandwidth = {"NB": 1101, "MB": 1102, "WB": 1103, "SWB": 1104, "FB": 1105}[bandwidth]
            for request, value in [(4002, bitrate), (4006, 0), (4010, 10),
                                   (4008, self.bandwidth), (4016, 0), (4022, 1)]:
                self.check(lib.opus_encoder_ctl(self.encoder, request, integer(value)))
            if mode != "celt":
                self.check(lib.opus_encoder_ctl(self.encoder, 4024, integer(3001)))
            delay = integer()
            self.check(lib.opus_encoder_ctl(self.encoder, 4027, ct.byref(delay)))
            self.delay = delay.value
            if mode == "celt" and self.delay != 120:
                raise ValueError("reference lookahead differs from owned CELT")
            if not 0 < self.delay <= samples:
                raise ValueError("reference lookahead exceeds the declared drain frame")
        except BaseException:
            self.close()
            raise

    @staticmethod
    def check(status):
        if status < 0:
            raise RuntimeError(f"libopus API error {status}")
        return status

    def close(self):
        for decoder in self.decoders:
            self.lib.opus_decoder_destroy(decoder)
        self.decoders.clear()
        if self.encoder:
            self.lib.opus_encoder_destroy(self.encoder)
            self.encoder = None

    def decode(self, decoder, packet, expected_range=None, lost=False):
        source = (ct.c_ubyte * len(packet)).from_buffer_copy(packet)
        output = np.zeros(self.samples, dtype=np.float32)
        count = self.check(self.lib.opus_decode_float(
            decoder, None if lost else source, 0 if lost else len(packet),
            output.ctypes.data_as(self.float_pointer), self.samples, 0))
        if count != self.samples or not np.isfinite(output).all():
            raise ValueError("reference decoded sample count or finite-PCM mismatch")
        if not lost and expected_range is not None:
            interval = ct.c_uint32()
            self.check(self.lib.opus_decoder_ctl(decoder, 4031, ct.byref(interval)))
            if interval.value != expected_range:
                raise ValueError("owned packet final interval disagrees with libopus")
        return output

    def roundtrip(self, original, packets, losses):
        outputs, peer = [], []
        for index, (owned_packet, owned_range) in enumerate(packets):
            frame = np.zeros(self.samples, dtype=np.float32)
            start = index * self.samples
            available = min(self.samples, max(0, len(original) - start))
            frame[:available] = original[start:start + available]
            storage = (ct.c_ubyte * self.budget)()
            length = self.check(self.lib.opus_encode_float(
                self.encoder, frame.ctypes.data_as(self.float_pointer),
                self.samples, storage, self.budget))
            wire = storage[0] >> 3
            mode = "silk" if wire < 12 else "hybrid" if wire < 16 else "celt"
            if (length != self.budget or self.lib.opus_packet_get_bandwidth(storage) != self.bandwidth
                    or self.lib.opus_packet_get_nb_channels(storage) != 1 or mode != self.mode):
                raise ValueError("reference packet budget or actual wire geometry differs")
            outputs.append(self.decode(self.decoders[0], bytes(storage), lost=index in losses))
            peer.append(self.decode(self.decoders[1], owned_packet, owned_range, index in losses))
        return (np.concatenate(outputs)[self.delay:self.delay + len(original)],
                np.concatenate(peer)[120:120 + len(original)])


def packet_archive(path, budget):
    data = path.read_bytes()
    if data[:8] != b"SNKQ0001":
        raise ValueError("invalid packet archive magic")
    cursor, packets = 8, []
    while cursor < len(data):
        if cursor + 8 > len(data):
            raise ValueError("truncated packet record")
        length, interval = struct.unpack_from("<II", data, cursor)
        cursor += 8
        if length != budget or cursor + length > len(data):
            raise ValueError("truncated or unequal-budget packet")
        packets.append((data[cursor:cursor + length], interval))
        cursor += length
    if not packets:
        raise ValueError("empty packet archive")
    return packets


def metrics(clean, original, decoded, key):
    if len(decoded) != len(original) or not np.isfinite(decoded).all():
        raise ValueError("decoded PCM length/finite mismatch")
    error = float(np.sum((original - decoded) ** 2))
    energy = float(np.sum(original ** 2))
    result = {
        **intelligibility(clean, decoded, key),
        "snr_db": 10 * math.log10(max(energy, 1e-30) / max(error, 1e-30)),
        "lsd_db": spectral_distance(spectrum(original), decoded),
    }
    if not all(math.isfinite(value) for value in result.values()):
        raise ValueError("non-finite quality metric")
    return result


def summarize(rows):
    groups = {}
    for row in rows:
        names = ("condition", "frame_ms", "bitrate", "bandwidth", "loss")
        key = tuple(row[name] for name in names)
        groups.setdefault(key, []).append(row)
    summary = []
    for key, cases in sorted(groups.items()):
        record = dict(zip(("condition", "frame_ms", "bitrate", "bandwidth", "loss"), key))
        speakers = sorted({r["speaker"] for r in cases})
        record.update(clips=len(cases), speakers=len(speakers))
        seed = int.from_bytes(hashlib.sha256(repr(key).encode()).digest()[:8], "little")
        indexes = np.random.default_rng(seed).integers(0, len(speakers), (10000, len(speakers)))
        parity = len(speakers) >= 2
        for metric in ("stoi", "estoi", "lsd_db", "snr_db"):
            deltas = np.array([r[f"current_{metric}"] - r[f"reference_{metric}"] for r in cases])
            by_speaker = np.array([np.mean([d for r, d in zip(cases, deltas) if r["speaker"] == s])
                                   for s in speakers])
            low, high = np.quantile(by_speaker[indexes].mean(axis=1), [0.025, 0.975])
            record[f"delta_{metric}"] = float(by_speaker.mean())
            record[f"delta_{metric}_ci_low"] = float(low)
            record[f"delta_{metric}_ci_high"] = float(high)
            record[f"{metric}_regressions"] = int(np.sum(deltas > 0 if metric == "lsd_db" else deltas < 0))
            if metric in ("stoi", "estoi"):
                parity &= low >= 0
            elif metric == "lsd_db":
                parity &= high <= 0
        record["objective_parity"] = bool(parity)
        summary.append(record)
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("output", type=Path, help="new artifact directory outside repository")
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--reference-library", type=Path, required=True)
    parser.add_argument("--baseline-probe", type=Path)
    parser.add_argument("--baseline-commit", help="provenance of the separately built baseline library")
    parser.add_argument("--split", choices=("development", "evaluation"), required=True)
    parser.add_argument("--bitrates", nargs="+", type=int, default=[12000, 24000, 32000])
    parser.add_argument("--frame-ms", nargs="+", type=int, choices=(10, 20), default=[10, 20])
    parser.add_argument("--mode", choices=("silk", "hybrid", "celt"), default="celt")
    parser.add_argument("--bandwidth", choices=("NB", "MB", "WB", "SWB", "FB"), default="WB")
    parser.add_argument("--conditions", nargs="+", choices=("clean", "noise20", "noise10"),
                        default=["clean", "noise20", "noise10"])
    parser.add_argument("--loss", choices=("none", "isolated", "burst"), default="none")
    parser.add_argument("--require-parity", action="store_true", help="fail unless objective gates pass on evaluation split; listening remains separate")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    if output == root or root in output.parents or output.exists():
        parser.error("output must be a new directory outside the repository")
    if args.require_parity and args.split != "evaluation":
        parser.error("parity acceptance requires the evaluation split")
    if args.require_parity and args.mode != "celt":
        parser.error("SILK/Hybrid delay differs: this driver cannot close matched-delay parity")
    if ((args.mode == "silk" and args.bandwidth not in ("NB", "MB", "WB"))
            or (args.mode == "hybrid" and args.bandwidth not in ("SWB", "FB"))
            or (args.mode == "celt" and args.bandwidth == "MB")):
        parser.error("unsupported mode/bandwidth combination")
    if any(b <= 0 or b * ms % 8000 for b in args.bitrates for ms in args.frame_ms):
        parser.error("bitrates must give positive integral packet byte budgets")
    corpus = json.loads((args.corpus / "manifest.json").read_text())
    speaker_splits = {}
    for clip in corpus["clips"]:
        speaker_splits.setdefault(clip["speaker"], set()).add(clip["split"])
    if any(len(splits) != 1 for splits in speaker_splits.values()):
        parser.error("development and evaluation speakers must be disjoint")
    clips = [x for x in corpus["clips"] if x["split"] == args.split]
    if not clips or len({x["clip"] for x in clips}) != len(clips):
        parser.error("split must contain nonempty uniquely named clips")
    probe = args.probe.resolve(strict=True)
    reference_path = args.reference_library.resolve(strict=True)
    baseline = args.baseline_probe.resolve(strict=True) if args.baseline_probe else None
    initial_hashes = {"probe": digest(probe), "reference_library": digest(reference_path),
                      "evaluator": digest(__file__), "metric_tool": digest(root / "tools/evaluate_quality.py"),
                      "noise_tool": digest(root / "tools/evaluate_speech.py"),
                      "corpus_manifest": digest(args.corpus / "manifest.json")}
    if baseline:
        initial_hashes["baseline_probe"] = digest(baseline)
    sources = source_snapshot(root)
    input_hashes = {clip["clip"]: clip["sha256"] for clip in clips}
    output.mkdir(parents=True)
    rows = []
    version = None
    with tempfile.TemporaryDirectory(prefix="snacka-opus-quality-") as temporary:
        temporary = Path(temporary)
        source, decoded, archive = (temporary / name for name in ("input.f32", "decoded.f32", "packets.bin"))
        for clip in clips:
            wav = (args.corpus / clip["clip"]).resolve(strict=True)
            if wav.parent != args.corpus.resolve() or digest(wav) != clip["sha256"]:
                raise ValueError("corpus path/hash mismatch")
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(wav), "-ar", "48000",
                            "-ac", "1", "-f", "f32le", str(source)], check=True)
            clean_bytes = source.read_bytes()
            clean = np.asarray(floats(clean_bytes), dtype=np.float64)
            if len(clean) < 48000 or not np.isfinite(clean).all() or np.max(np.abs(clean)) > 1:
                raise ValueError("quality clips must have at least one second of finite normalized speech")
            for condition in args.conditions:
                data, clipped = (clean_bytes, 0) if condition == "clean" else noisy(
                    floats(clean_bytes), clip["clip"], int(condition[5:]))
                if clipped:
                    raise ValueError("added noise clips evaluation input")
                original = np.asarray(floats(data), dtype=np.float64)
                source.write_bytes(data)
                for ms in args.frame_ms:
                    for bitrate in args.bitrates:
                        samples = ms * 48
                        total = (len(clean) + samples - 1) // samples + 1
                        losses = set()
                        if args.loss != "none":
                            for index in range(11, total - 3, 23):
                                losses.update(range(index, index + (3 if args.loss == "burst" else 1)))
                        loss_argument = ",".join(map(str, sorted(losses))) or "-"
                        row = {"clip": clip["clip"], "speaker": clip["speaker"], "split": args.split,
                               "input_sha256": clip["sha256"], "pcm_sha256": hashlib.sha256(data).hexdigest(),
                               "condition": condition, "frame_ms": ms, "bitrate": bitrate,
                               "bandwidth": args.bandwidth, "mode": args.mode,
                               "loss": args.loss, "lost_packets": len(losses)}
                        for label, executable in [("current", probe)] + ([("baseline", baseline)] if baseline else []):
                            command = [str(executable), str(source), str(decoded), str(archive), args.mode,
                                       args.bandwidth, str(ms), "1", str(bitrate), loss_argument]
                            stats = json.loads(subprocess.check_output(command, text=True))
                            packets = packet_archive(archive, bitrate * ms // 8000)
                            if (stats["samples"] != len(clean) or stats["packets"] != total
                                    or len(packets) != total or stats["algorithmic_delay_samples"] != 120
                                    or stats["channels"] != 1
                                    or stats["packet_bytes"] + stats["drain_bytes"] != total * bitrate * ms // 8000):
                                raise ValueError("owned delay, bytes or packet count mismatch")
                            values = np.frombuffer(decoded.read_bytes(), dtype="<f4").astype(np.float64)
                            ref = Reference(reference_path, samples, args.bandwidth, bitrate, args.mode)
                            row["reference_delay_samples"] = ref.delay
                            row["owned_delay_samples"] = stats["algorithmic_delay_samples"]
                            try:
                                reference_values, peer_values = ref.roundtrip(original, packets, losses)
                                version = ref.lib.opus_get_version_string().decode()
                            finally:
                                ref.close()
                            if not losses and args.mode == "celt":
                                error = float(np.max(np.abs(peer_values - values)))
                                if error > 2e-5:
                                    raise ValueError(f"owned CELT PCM disagrees with peer decode: {error}")
                            row[f"{label}_pcm_sha256"] = digest(decoded)
                            row[f"{label}_packet_sha256"] = digest(archive)
                            row[f"{label}_packet_bytes"] = stats["packet_bytes"]
                            row[f"{label}_drain_bytes"] = stats["drain_bytes"]
                            row[f"{label}_peer_max_error"] = float(np.max(np.abs(peer_values - values)))
                            key = f'{clip["clip"]}:{condition}:{ms}:{bitrate}:{args.bandwidth}:{args.loss}'
                            for name, value in metrics(clean, original, values, key).items():
                                row[f"{label}_{name}"] = value
                            if label == "current":
                                for name, value in metrics(clean, original, reference_values, key).items():
                                    row[f"reference_{name}"] = value
                                row["reference_pcm_sha256"] = hashlib.sha256(reference_values.tobytes()).hexdigest()
                            elif hashlib.sha256(reference_values.tobytes()).hexdigest() != row["reference_pcm_sha256"]:
                                raise ValueError("reference self-control differs between paired probes")
                        rows.append(row)
            print(f'evaluated {clip["clip"]}: {len(rows)} cases', flush=True)
    # Binary/source mutation during scoring must invalidate the entire run.
    paths = {"probe": probe, "reference_library": reference_path, "evaluator": Path(__file__),
             "metric_tool": root / "tools/evaluate_quality.py", "noise_tool": root / "tools/evaluate_speech.py",
             "corpus_manifest": args.corpus / "manifest.json"}
    if baseline:
        paths["baseline_probe"] = baseline
    if any(digest(paths[name]) != expected for name, expected in initial_hashes.items()):
        raise ValueError("measurement source or binary changed during scoring")
    if source_snapshot(root) != sources or any(digest(args.corpus / name) != expected
                                              for name, expected in input_hashes.items()):
        raise ValueError("library source or corpus changed during scoring")
    with (output / "quality.csv").open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=list(rows[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)
    summary = summarize(rows)
    metadata = {"schema": 1, "platform": platform.platform(), "python": platform.python_version(),
                "numpy": np.__version__, "scipy": package_version("scipy"),
                "pystoi": package_version("pystoi"),
                "ffmpeg": subprocess.check_output(["ffmpeg", "-version"], text=True).splitlines()[0],
                "reference_version": version, "source_unchanged": True,
                "library_source_sha256": sources,
                "sha256": initial_hashes, "corpus": corpus, "arguments": vars(args),
                "cases": len(rows), "summary": summary, "bootstrap": "10000 paired speaker-resamples; deterministic 95% percentile intervals, no multiplicity correction",
                "controls": "mono 48 kHz, matched signaled mode/bandwidth and total CBR packet bytes including one drain frame, separately declared/aligned codec delays, complexity10 reference, DTX disabled; received-packet intervals exact. SILK/Hybrid delay differs and is exploration only.",
                "full_quality_goal_met": False, "limitations": "Speech objective metrics on this declared corpus; blind listening, Swedish/multilingual speech, music, realistic microphone noise and full codec/mode/CPU acceptance remain separate."}
    (output / "source.json").write_text(json.dumps(metadata, indent=2, default=str) + "\n")
    failures = sum(not item["objective_parity"] for item in summary)
    print(f"Objective parity: {len(summary) - failures}/{len(summary)} conditions; full quality goal remains open.")
    if args.require_parity and failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
