# SPDX-License-Identifier: MIT
"""Generate test data by invoking a separately installed libopus 1.6.1 archive."""

import argparse
import ctypes
from pathlib import Path
import struct
import subprocess
import sys
import tempfile


def configure(library):
    pointer, unsigned, integer = ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int
    signatures = {
        "ec_enc_init": ([pointer, pointer, unsigned], None),
        "ec_encode": ([pointer, unsigned, unsigned, unsigned], None),
        "ec_enc_bit_logp": ([pointer, integer, unsigned], None),
        "ec_enc_uint": ([pointer, unsigned, unsigned], None),
        "ec_enc_bits": ([pointer, unsigned, unsigned], None),
        "ec_enc_shrink": ([pointer, unsigned], None),
        "ec_enc_done": ([pointer], None),
        "ec_dec_init": ([pointer, pointer, unsigned], None),
        "ec_decode": ([pointer, unsigned], unsigned),
        "ec_dec_update": ([pointer, unsigned, unsigned, unsigned], None),
        "ec_dec_bit_logp": ([pointer, unsigned], integer),
        "ec_dec_uint": ([pointer, unsigned], unsigned),
        "ec_dec_bits": ([pointer, unsigned], unsigned),
        "ec_tell_frac": ([pointer], unsigned),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(library, "oracle_" + name)
        function.argtypes, function.restype = arguments, result
    for name in ["range", "tell", "error"]:
        function = getattr(library, "oracle_" + name)
        function.argtypes, function.restype = [pointer], unsigned
    library.oracle_version.restype = ctypes.c_char_p
    version = library.oracle_version().decode()
    if version != "libopus 1.6.1":
        raise RuntimeError(f"expected libopus 1.6.1, received {version}")
    if library.oracle_size() > 256:
        raise RuntimeError("oracle context exceeds allocated test storage")
    return version


def verify_packet(library, packet, events, case):
    boundaries = [0, 27, 200, 256]
    decoder = (ctypes.c_uint64 * 32)()
    storage = ctypes.create_string_buffer(packet)
    library.oracle_ec_dec_init(decoder, storage, len(packet))
    for index, (kind, parameter, value, interval, tell, eighths) in enumerate(events):
        if kind == 0:
            frequency = library.oracle_ec_decode(decoder, 256)
            actual = next(j for j in range(3) if boundaries[j] <= frequency < boundaries[j + 1])
            library.oracle_ec_dec_update(decoder, boundaries[actual], boundaries[actual + 1], 256)
        elif kind == 1:
            actual = library.oracle_ec_dec_bit_logp(decoder, parameter)
        elif kind == 2:
            actual = library.oracle_ec_dec_bits(decoder, parameter)
        else:
            actual = library.oracle_ec_dec_uint(decoder, parameter)
        state = (library.oracle_range(decoder), library.oracle_tell(decoder),
                 library.oracle_ec_tell_frac(decoder))
        if actual != value or state != (interval, tell, eighths) or library.oracle_error(decoder):
            raise RuntimeError(f"reference decode failed for frame {case}, event {index}")


def verify_file(library, path):
    data = path.read_bytes()
    if data[:8] != b"SNKENT01":
        raise RuntimeError("invalid entropy vector magic")
    cases, = struct.unpack_from("<I", data, 8)
    cursor, count = 12, 0
    for case in range(cases):
        length, event_count = struct.unpack_from("<HH", data, cursor)
        cursor += 4
        packet = data[cursor:cursor + length]
        cursor += length
        events = [struct.unpack_from("<BIIIII", data, cursor + index * 21)
                  for index in range(event_count)]
        cursor += event_count * 21
        verify_packet(library, packet, events, case)
        count += event_count
    if cursor != len(data):
        raise RuntimeError("trailing entropy vector bytes")
    print(f"libopus 1.6.1 independently decoded {cases} native frames, {count:,} operations")


def generate(library, output):
    data = bytearray(b"SNKENT01" + struct.pack("<I", 128))
    boundaries = [0, 27, 200, 256]
    for seed in range(1, 129):
        context = (ctypes.c_uint64 * 32)()
        packet = ctypes.create_string_buffer(4096)
        library.oracle_ec_enc_init(context, packet, len(packet))
        random, events = seed, []
        for index in range(128):
            random = (random ^ (random << 13)) & 0xFFFFFFFF
            random = (random ^ (random >> 17)) & 0xFFFFFFFF
            random = (random ^ (random << 5)) & 0xFFFFFFFF
            kind = index % 4
            if kind == 0:
                parameter, value = 3, random % 3
                library.oracle_ec_encode(context, boundaries[value], boundaries[value + 1], 256)
            elif kind == 1:
                parameter, value = 1 + random % 16, (random >> 16) & 1
                library.oracle_ec_enc_bit_logp(context, value, parameter)
            elif kind == 2:
                parameter = 1 + random % 24
                value = (random >> 7) & ((1 << parameter) - 1)
                library.oracle_ec_enc_bits(context, value, parameter)
            else:
                parameter = 0xFFFFFFFF if index % 32 == 31 else 2 + (random & 65535)
                value = (random >> 4) % parameter
                library.oracle_ec_enc_uint(context, value, parameter)
            events.append((kind, parameter, value, library.oracle_range(context),
                           library.oracle_tell(context), library.oracle_ec_tell_frac(context)))
        length = (library.oracle_tell(context) + 7) // 8
        library.oracle_ec_enc_shrink(context, length)
        library.oracle_ec_enc_done(context)
        if library.oracle_error(context):
            raise RuntimeError(f"reference encoder failed for seed {seed}")
        verify_packet(library, packet.raw[:length], events, seed)
        data += struct.pack("<HH", length, len(events)) + packet.raw[:length]
        for event in events:
            data += struct.pack("<BIIIII", *event)
    output.write_bytes(data)
    print(f"libopus 1.6.1: 128 frames, 16,384 operations, {len(data)} fixture bytes")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="path to libopus 1.6.1 libopus.a")
    parser.add_argument("--output", type=Path,
                        default=Path(__file__).resolve().parents[2] / "tests/fixtures/entropy-libopus-1.6.1.bin")
    parser.add_argument("--verify-file", type=Path, help="independently decode native entropy vectors")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="snacka-oracle-") as temporary:
        library = Path(temporary) / "oracle.so"
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", str(Path(__file__).with_name("entropy_bridge.c")),
                        str(args.archive.resolve()), "-lm", "-o", str(library)], check=True)
        oracle = ctypes.CDLL(str(library))
        configure(oracle)
        if args.verify_file:
            verify_file(oracle, args.verify_file)
        else:
            generate(oracle, args.output)


if __name__ == "__main__":
    main()
