# SPDX-License-Identifier: MIT
"""Freeze independent SILK excitation results from an installed libopus 1.6.1.

Only the optional generator links the oracle. Cargo uses the frozen fixture;
no external codec implementation is part of a Snacka build or test runtime.
"""

import argparse
import ast
import ctypes
from pathlib import Path
import random
import struct
import subprocess
import sys
import tempfile

from generate_entropy_fixtures import configure


ROOT = Path(__file__).resolve().parents[2]
SAMPLES = (80, 120, 160, 160, 240, 320)


def table(name):
    """Read the small normative probability data, never oracle source code."""
    text = (ROOT / "src/opus/silk/tables.rs").read_text()
    start = text.index("= [", text.index("const " + name)) + 2
    depth, end = 0, start
    while end < len(text):
        char = text[end]
        depth += (char == "[") - (char == "]")
        end += 1
        if depth == 0:
            break
    return ast.literal_eval("\n".join(line.split("//")[0] for line in text[start:end].splitlines()))


def encoded_case(oracle, shape, kind, rate, depth, pulses, placement):
    """Construct targeted grammar cases with RFC PDFs through oracle range coding."""
    rate_weights, count_weights, shell_weights, sign_weights = (
        table(name) for name in ("RATE_WEIGHTS", "COUNT_WEIGHTS", "SHELL_WEIGHTS", "NEGATIVE_WEIGHTS")
    )
    state = (ctypes.c_uint64 * 32)()
    storage = ctypes.create_string_buffer(2048)
    oracle.oracle_ec_enc_init(state, storage, len(storage))

    def symbol(weights, value):
        if not weights[value]:
            raise ValueError("targeted symbol has zero probability")
        low = sum(weights[:value])
        oracle.oracle_ec_encode(state, low, low + weights[value], 256)

    symbol(rate_weights[kind // 2 == 2], rate)
    for block in range((SAMPLES[shape] + 15) // 16):
        if block == 0:
            if depth:
                symbol(count_weights[rate], 17)
                for extra in range(1, depth):
                    symbol(count_weights[9], 17)
                symbol(count_weights[10 if depth == 10 else 9], pulses)
            else:
                symbol(count_weights[rate], pulses)
        else:
            symbol(count_weights[rate], 0)

    raw = [0] * 16
    raw[placement] = pulses

    def split(values, level):
        total = sum(values)
        if not total or len(values) == 1:
            return
        half = len(values) // 2
        symbol(shell_weights[level][total - 1], sum(values[:half]))
        split(values[:half], level + 1)
        split(values[half:], level + 1)

    split(raw, 0)
    for index, value in enumerate(raw):
        for bit in range(depth - 1, -1, -1):
            # Every coefficient gets LSBs, including the initially empty ones.
            next_bit = (index >> (bit % 4)) & 1
            symbol((136, 120), next_bit)
            value = 2 * value + next_bit
        raw[index] = value
    negative = sign_weights[kind][min(pulses, 6)]
    for index, value in enumerate(raw):
        if value:
            symbol((negative, 256 - negative), index & 1)
    length = (oracle.oracle_tell(state) + 7) // 8
    oracle.oracle_ec_enc_shrink(state, length)
    oracle.oracle_ec_enc_done(state)
    if oracle.oracle_error(state):
        raise RuntimeError("targeted entropy case exceeded its temporary buffer")
    return storage.raw[:length]


def generate(oracle, output):
    oracle.oracle_silk_decode_pulses.argtypes = [ctypes.c_void_p, ctypes.c_void_p,
                                               ctypes.c_int, ctypes.c_int, ctypes.c_int]
    oracle.oracle_silk_decode_pulses.restype = None
    rng = random.Random(6716)
    cases = []
    for shape in range(6):
        for kind in range(6):
            for packet in (bytes(128), bytes([255]) * 128, *(rng.randbytes(128) for _ in range(6))):
                cases.append((shape, kind, packet))
    # Each legal escape depth, every pulse count, every rate and every placement
    # are exercised. Full-width tails are parsed even in a 10 ms MB frame.
    for kind in range(6):
        for depth in range(11):
            pulses = 0 if depth % 2 else 16
            shape = depth % 6
            cases.append((shape, kind, encoded_case(oracle, shape, kind, depth % 9, depth,
                                                   pulses, depth % 16)))
    for pulses in range(1, 17):
        for placement in range(16):
            kind = (pulses + placement) % 6
            cases.append((0, kind, encoded_case(oracle, 0, kind, pulses % 9, 0, pulses, placement)))

    data = bytearray(b"SNKSLK01" + struct.pack("<I", len(cases)))
    maximum = 0
    for shape, kind, packet in cases:
        state = (ctypes.c_uint64 * 32)()
        storage = ctypes.create_string_buffer(packet)
        output_pulses = (ctypes.c_int16 * 320)()
        oracle.oracle_ec_dec_init(state, storage, len(packet))
        oracle.oracle_silk_decode_pulses(state, output_pulses, kind // 2, kind & 1, SAMPLES[shape])
        if oracle.oracle_error(state):
            raise RuntimeError("oracle SILK decoder reported an entropy error")
        coded = (SAMPLES[shape] + 15) // 16 * 16
        maximum = max(maximum, *(abs(value) for value in output_pulses[:coded]))
        data += struct.pack("<BBHHIII", shape, kind, len(packet), coded,
                            oracle.oracle_range(state), oracle.oracle_tell(state),
                            oracle.oracle_ec_tell_frac(state))
        data += packet + struct.pack("<" + "h" * coded, *output_pulses[:coded])
    output.write_bytes(data)
    print(f"libopus 1.6.1: {len(cases)} SILK pulse/state cases, {len(data)} bytes, max magnitude {maximum}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="separately installed libopus 1.6.1 static archive")
    parser.add_argument("--output", type=Path, default=ROOT / "tests/fixtures/opus-silk-pulses.bin")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="snacka-silk-oracle-") as temporary:
        path = Path(temporary) / "oracle.so"
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", str(Path(__file__).with_name("entropy_bridge.c")),
                        str(Path(__file__).with_name("silk_bridge.c")), str(args.archive.resolve()),
                        "-lm", "-o", str(path)], check=True)
        oracle = ctypes.CDLL(str(path))
        configure(oracle)
        generate(oracle, args.output)


if __name__ == "__main__":
    main()
