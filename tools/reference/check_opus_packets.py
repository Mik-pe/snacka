# SPDX-License-Identifier: MIT
"""Black-box packet framing differential and optional libopus timing comparison.

Requires Python's standard library, rustc, and an installed libopus shared
library. --benchmark additionally requires a C compiler. All generated records
and compiled probes live in a temporary directory; no libopus code or FFI is
linked into Snacka. Frame payloads here are synthetic, not playable audio.
The oracle uses the public opus_packet_parse and packet introspection APIs:
https://opus-codec.org/docs/opus_api-1.5/group__opus__decoder.html
Framing requirements are RFC 6716 section 3:
https://datatracker.ietf.org/doc/html/rfc6716#section-3

Timing compares Rust's optimized, statically compiled parser with the installed
libopus parser through its public shared-library entry point. Both loops parse
and consume frame lengths without inspecting audio data. Loading, allocation,
file I/O, Python, and process startup are excluded from timed intervals.

Example:
  python3 tools/reference/check_opus_packets.py \
    /opt/homebrew/opt/opus/lib/libopus.0.dylib --benchmark
"""

import argparse
from collections import Counter
import ctypes
import hashlib
import json
import math
from pathlib import Path
import platform
import random
import statistics
import struct
import subprocess
import sys
import tempfile


RATES = (8000, 12000, 16000, 24000, 48000)
FRAME_SAMPLES = (
    480, 960, 1920, 2880, 480, 960, 1920, 2880,
    480, 960, 1920, 2880, 480, 960, 480, 960,
    120, 240, 480, 960, 120, 240, 480, 960,
    120, 240, 480, 960, 120, 240, 480, 960,
)


def configure(path):
    library = ctypes.CDLL(str(path))
    pointer = ctypes.c_void_p
    integer = ctypes.c_int
    library.opus_packet_parse.argtypes = (
        pointer, ctypes.c_int32, ctypes.POINTER(ctypes.c_ubyte),
        ctypes.POINTER(pointer), ctypes.POINTER(ctypes.c_int16),
        ctypes.POINTER(integer),
    )
    library.opus_packet_parse.restype = integer
    for name in ("opus_packet_get_nb_channels", "opus_packet_get_bandwidth"):
        function = getattr(library, name)
        function.argtypes, function.restype = (pointer,), integer
    library.opus_packet_get_samples_per_frame.argtypes = (pointer, ctypes.c_int32)
    library.opus_packet_get_samples_per_frame.restype = integer
    library.opus_packet_get_nb_samples.argtypes = (pointer, ctypes.c_int32, ctypes.c_int32)
    library.opus_packet_get_nb_samples.restype = integer
    library.opus_get_version_string.restype = ctypes.c_char_p
    return library


def reference(library, data):
    storage = ctypes.create_string_buffer(data, max(len(data), 1))
    toc, offset = ctypes.c_ubyte(), ctypes.c_int()
    frames = (ctypes.c_void_p * 48)()
    lengths = (ctypes.c_int16 * 48)()
    count = library.opus_packet_parse(storage, len(data), ctypes.byref(toc),
                                      frames, lengths, ctypes.byref(offset))
    if count < 0:
        return None
    base = ctypes.addressof(storage)
    descriptions = [(lengths[index], frames[index] - base) for index in range(count)]
    payload_end = descriptions[-1][1] + descriptions[-1][0]
    return {
        "toc": toc.value,
        "channels": library.opus_packet_get_nb_channels(storage),
        "bandwidth": library.opus_packet_get_bandwidth(storage) - 1101,
        "vbr": bool(data[0] & 3 == 2 or data[0] & 3 == 3 and data[1] & 128),
        "samples": [(library.opus_packet_get_samples_per_frame(storage, rate),
                     library.opus_packet_get_nb_samples(storage, len(data), rate)) for rate in RATES],
        "padding": len(data) - payload_end,
        "frames": descriptions,
    }


def length_bytes(length):
    if length < 252:
        return bytes((length,))
    return bytes((252 + length % 4, (length - 252) // 4))


def make_packet(config, channels, code, lengths, padding=None):
    toc = config << 3 | (channels - 1) << 2 | code
    header = bytearray((toc,))
    if code == 2:
        header.extend(length_bytes(lengths[0]))
    elif code == 3:
        vbr = len(set(lengths)) > 1
        header.append(len(lengths) | (128 if vbr else 0) | (64 if padding is not None else 0))
        if padding is not None:
            remaining = padding
            while remaining >= 255:
                header.append(255)
                remaining -= 254
            header.append(remaining)
        if vbr:
            for length in lengths[:-1]:
                header.extend(length_bytes(length))
    elif padding is not None:
        raise ValueError("only code 3 packets carry Opus padding")
    for index, length in enumerate(lengths):
        header.extend(bytes((index * 29 & 255,)) * length)
    if padding:
        # Decoders must accept arbitrary trailing padding byte values.
        header.extend(b"\xa5" * padding)
    return bytes(header)


def cases(seed, random_cases):
    rng = random.Random(seed)
    valid = []
    for config in range(32):
        maximum = 5760 // FRAME_SAMPLES[config]
        for channels in (1, 2):
            for size in (0, 1, 80, 251, 252, 1275):
                for code, lengths in ((0, [size]), (1, [size, size])):
                    valid.append(make_packet(config, channels, code, lengths))
            for lengths in ((0, 0), (0, 1), (1, 0), (251, 252),
                            (252, 251), (1275, 1275), (1, 1275)):
                valid.append(make_packet(config, channels, 2, lengths))
            for count in sorted({1, min(3, maximum), maximum}):
                sizes = [[size] * count for size in (0, 80, 1275)]
                sizes.append([(0, 1, 251, 252, 1275)[index % 5] for index in range(count)])
                for lengths in sizes:
                    for padding in (None, 0, 1, 254, 255, 510):
                        valid.append(make_packet(config, channels, 3, lengths, padding))
    yield from (("structured", packet) for packet in valid)

    for config in range(32):
        for channels in (1, 2):
            for control in range(256):
                toc = config << 3 | (channels - 1) << 2 | 3
                yield "control_grid", bytes((toc, control, 252, 0, 255))

    # Mutation is independent of the parser and reaches deep lacing boundaries.
    for packet in rng.sample(valid, min(1000, len(valid))):
        cut = rng.randrange(len(packet) + 1)
        yield "truncation", packet[:cut]
        altered = bytearray(packet)
        if altered:
            altered[rng.randrange(min(len(altered), 128))] = rng.randrange(256)
        yield "mutation", bytes(altered)

    for _ in range(random_cases):
        bucket = rng.randrange(100)
        length = (rng.randrange(9) if bucket < 60 else rng.randrange(9, 129) if bucket < 85
                  else rng.randrange(129, 4097) if bucket < 99 else rng.randrange(4097, 65537))
        yield "random", rng.randbytes(length)

    for config in (16, 20, 24, 28):
        for channels in (1, 2):
            for padding in (None, 16384):
                yield "maximum_payload", make_packet(config, channels, 3, [1275] * 48, padding)
                # Force VBR lacing with the same 48-frame duration.
                yield "maximum_payload", make_packet(config, channels, 3, [1275] * 47 + [1274], padding)
    for config in range(32):
        for code, header in ((0, b""), (1, b""), (2, b"\0"), (3, b"\1"), (3, b"\x81")):
            yield "implicit_oversize", bytes((config << 3 | code,)) + header + b"x" * (2552 if code == 1 else 1276)


def write_records(path, packets):
    with path.open("wb") as output:
        for packet in packets:
            output.write(struct.pack("<I", len(packet)))
            output.write(packet)


def read_probe(data, cursor):
    accepted = data[cursor]
    cursor += 1
    if accepted == 0:
        return None, cursor
    if accepted != 1:
        raise RuntimeError("invalid Rust probe acceptance flag")
    toc, count, channels, bandwidth, vbr = struct.unpack_from("<5B", data, cursor)
    cursor += 5
    samples = [struct.unpack_from("<HH", data, cursor + index * 4) for index in range(5)]
    cursor += 20
    padding, = struct.unpack_from("<I", data, cursor)
    cursor += 4
    frames = [struct.unpack_from("<HI", data, cursor + index * 6) for index in range(count)]
    cursor += count * 6
    return {"toc": toc, "channels": channels, "bandwidth": bandwidth,
            "vbr": bool(vbr), "samples": samples, "padding": padding, "frames": frames}, cursor


def differential(library, probe, directory, seed, random_cases):
    generated = list(cases(seed, random_cases))
    records = directory / "differential.bin"
    write_records(records, (packet for _, packet in generated))
    result = subprocess.run([str(probe), str(records)], check=True, capture_output=True).stdout
    cursor = 0
    counts, accepted, oversize = Counter(), 0, 0
    for index, (category, packet) in enumerate(generated):
        counts[category] += 1
        actual, cursor = read_probe(result, cursor)
        expected = reference(library, packet)
        if actual != expected:
            # Some oracle versions permit RFC-forbidden implicit >1275 lengths.
            # Allow only that observable, strictly narrower acceptance difference.
            if actual is None and expected is not None and any(length > 1275 for length, _ in expected["frames"]):
                oversize += 1
                continue
            digest = hashlib.sha256(packet).hexdigest()
            raise RuntimeError(f"case {index} ({category}, {len(packet)} bytes, sha256 {digest}): "
                               f"Rust={actual}, libopus={expected}; first 32 bytes={packet[:32].hex()}")
        accepted += actual is not None
    if cursor != len(result):
        raise RuntimeError("unexpected trailing Rust probe output")
    return {"cases": len(generated), "accepted": accepted, "rejected": len(generated) - accepted,
            "reference_oversize_only": oversize, "categories": dict(counts),
            "records_sha256": hashlib.sha256(records.read_bytes()).hexdigest()}


def benchmark_groups():
    yield "single_frame", [make_packet((1, 5, 9, 13, 15, 19, 23, 27, 31)[index % 9],
                                        1 + index % 2, 0, [80 + index]) for index in range(128)]
    yield "compound", [make_packet(31, 1 + index % 2, 3,
                                   [80 + index % 32 + frame * (index % 2) for frame in range(2 + index % 5)])
                       for index in range(128)]
    yield "padding", [make_packet(31, 1, 3, [80, 81], (255, 510, 4096)[index % 3])
                      for index in range(128)]
    yield "maximum_48_frames", [make_packet(16, 1 + index % 2, 3,
                                            [1275] * 47 + [1275 - index % 2]) for index in range(16)]
    malformed = (b"", b"\x02", b"\x02\xfc", b"\x02\x01", b"\x03\x00",
                 b"\x03\x41\xff", b"\x83\x31", b"\x83\x82\xfc", b"\x83\x82\x02x")
    yield "malformed", [malformed[index % len(malformed)] for index in range(128)]


def timing(command):
    elapsed, checksum = subprocess.run(command, check=True, capture_output=True, text=True).stdout.split()
    return float(elapsed), int(checksum)


def benchmark(probe, native, library_path, directory, operations, runs):
    results = {}
    for name, packets in benchmark_groups():
        records = directory / f"benchmark-{name}.bin"
        write_records(records, packets)
        repetitions = math.ceil(operations / len(packets))
        commands = ([str(probe), str(records), str(repetitions)],
                    [str(native), str(library_path), str(records), str(repetitions)])
        for command in commands:
            timing(command)  # Warm code and dynamic-library pages before sampling.
        rust_times, native_times = [], []
        for run in range(runs):
            # Alternate process order to reduce systematic temperature/order bias.
            samples = [None, None]
            for index in ((0, 1) if run % 2 == 0 else (1, 0)):
                samples[index] = timing(commands[index])
            if samples[0][1] != samples[1][1]:
                raise RuntimeError(f"{name} benchmark checksums differ: {samples}")
            rust_times.append(samples[0][0])
            native_times.append(samples[1][0])
        calls = len(packets) * repetitions
        rust_ns = statistics.median(rust_times) * 1e9 / calls
        native_ns = statistics.median(native_times) * 1e9 / calls
        results[name] = {"calls_per_run": calls, "runs": runs, "rust_ns": rust_ns,
                         "libopus_ns": native_ns, "rust_over_libopus": rust_ns / native_ns,
                         "rust_seconds": rust_times, "libopus_seconds": native_times}
        print(f"  {name}: Rust {rust_ns:.2f} ns, libopus {native_ns:.2f} ns, "
              f"Rust/libopus {rust_ns / native_ns:.3f}")
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("library", type=Path, help="path to separately installed libopus shared library")
    parser.add_argument("--seed", type=lambda value: int(value, 0), default=0x6716)
    parser.add_argument("--random-cases", type=int, default=10000)
    parser.add_argument("--benchmark", action="store_true")
    parser.add_argument("--benchmark-operations", type=int, default=2_000_000)
    parser.add_argument("--benchmark-runs", type=int, default=5)
    parser.add_argument("--json-output", type=Path, help="save verification and timing evidence outside the repository")
    args = parser.parse_args()
    if args.random_cases < 0 or args.benchmark_operations < 1 or args.benchmark_runs < 1:
        parser.error("case count must be nonnegative; benchmark operations and runs must be positive")
    library_path = args.library.resolve(strict=True)
    library = configure(library_path)
    source = Path(__file__).resolve().parent
    version = library.opus_get_version_string().decode()
    report = {"reference": version, "seed": args.seed, "platform": platform.platform(),
              "machine": platform.machine(), "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip()}
    with tempfile.TemporaryDirectory(prefix="snacka-packet-oracle-") as temporary:
        directory = Path(temporary)
        probe = directory / "packet-probe"
        subprocess.run(["rustc", "--edition=2024", "-C", "opt-level=3", "-C", "codegen-units=1",
                        "-C", "overflow-checks=yes", str(source / "packet_probe.rs"), "-o", str(probe)], check=True)
        report["differential"] = differential(library, probe, directory, args.seed, args.random_cases)
        result = report["differential"]
        print(f"{version}: {result['cases']:,} cases; {result['accepted']:,} accepted, "
              f"{result['rejected']:,} rejected; {result['reference_oversize_only']} RFC size-limit differences")
        if args.benchmark:
            native = directory / "packet-bench"
            command = ["cc", "-O3", "-Wall", "-Wextra", "-Werror", str(source / "packet_bench.c"), "-o", str(native)]
            if sys.platform != "darwin":
                command.append("-ldl")
            subprocess.run(command, check=True)
            report["cc"] = subprocess.check_output(["cc", "--version"], text=True).splitlines()[0]
            print("Packet parsing and frame-length consumption only; median compiled loops, no Python FFI timing:")
            report["benchmark"] = benchmark(probe, native, library_path, directory,
                                             args.benchmark_operations, args.benchmark_runs)
    if args.json_output:
        args.json_output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
