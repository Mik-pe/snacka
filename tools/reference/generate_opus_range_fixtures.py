# SPDX-License-Identifier: MIT
"""Generate independent ICDF/Laplace fixtures using installed libopus 1.6.1.

Only external function calls are compiled. No reference implementation source
is read or copied. Every generated stream is decoded by the reference before
being saved, including every intermediate interval and bit-usage measurement.
"""

import argparse
import ctypes
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

from generate_entropy_fixtures import configure


TABLES = [([29, 26, 0], 5), ([200, 200, 128, 0], 8), ([7, 4, 2, 0], 3), ([0], 1)]
PARAMETERS = [(10000, 10000), (6000, 12000), (256, 16300), (10000, 0),
              (30000, 1000), (1, 16384), (32736, 0), (32736, 16384), (1, 0)]

BRIDGE = """
extern void ec_enc_icdf(void *, int, const unsigned char *, unsigned);
void oracle_icdf_encode(void *s, int x, const unsigned char *t, unsigned b) {
    ec_enc_icdf(s, x, t, b);
}
extern int ec_dec_icdf(void *, const unsigned char *, unsigned);
int oracle_icdf_decode(void *s, const unsigned char *t, unsigned b) {
    return ec_dec_icdf(s, t, b);
}
extern void ec_laplace_encode(void *, int *, unsigned, int);
void oracle_laplace_encode(void *s, int *x, unsigned f, int d) {
    ec_laplace_encode(s, x, f, d);
}
extern int ec_laplace_decode(void *, unsigned, int);
int oracle_laplace_decode(void *s, unsigned f, int d) {
    return ec_laplace_decode(s, f, d);
}
"""


def configure_models(library):
    pointer, unsigned, integer = ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int
    signatures = {
        "icdf_encode": ([pointer, integer, pointer, unsigned], None),
        "icdf_decode": ([pointer, pointer, unsigned], integer),
        "laplace_encode": ([pointer, ctypes.POINTER(integer), unsigned, integer], None),
        "laplace_decode": ([pointer, unsigned, integer], integer),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(library, "oracle_" + name)
        function.argtypes, function.restype = arguments, result


def icdf_table(index):
    table, bits = TABLES[index]
    return (ctypes.c_ubyte * len(table))(*table), bits


def verify_packet(library, packet, events, case):
    state = (ctypes.c_uint64 * 32)()
    storage = ctypes.create_string_buffer(packet)
    library.oracle_ec_dec_init(state, storage, len(packet))
    for index, event in enumerate(events):
        kind, first, second, _requested, actual, interval, tell, fraction = event
        if kind == 0:
            table, bits = icdf_table(first)
            decoded = library.oracle_icdf_decode(state, table, bits)
        elif kind == 1:
            decoded = library.oracle_laplace_decode(state, first, second)
        elif kind == 2:
            decoded = library.oracle_ec_dec_bit_logp(state, first)
        elif kind == 3:
            decoded = library.oracle_ec_dec_bits(state, first)
        else:
            decoded = library.oracle_ec_dec_uint(state, first)
        observed = (library.oracle_range(state), library.oracle_tell(state),
                    library.oracle_ec_tell_frac(state))
        if decoded & 0xFFFFFFFF != actual or observed != (interval, tell, fraction):
            raise RuntimeError(f"reference decode failed for frame {case}, event {index}")
        if library.oracle_error(state):
            raise RuntimeError(f"reference decode error for frame {case}, event {index}")


def generate(library, output):
    data = bytearray(b"SNKOPR01" + struct.pack("<I", 32))
    for seed in range(1, 33):
        state = (ctypes.c_uint64 * 32)()
        packet = ctypes.create_string_buffer(4096)
        library.oracle_ec_enc_init(state, packet, len(packet))
        random, events = seed, []
        for index in range(96):
            random ^= (random << 13) & 0xFFFFFFFF
            random ^= random >> 17
            random ^= (random << 5) & 0xFFFFFFFF
            kind = index % 5
            if kind == 0:
                first = random % len(TABLES)
                table_values, second = TABLES[first]
                allowed = [i for i, value in enumerate(table_values)
                           if ((1 << second) if i == 0 else table_values[i - 1]) > value]
                requested = allowed[(random >> 4) % len(allowed)]
                table, bits = icdf_table(first)
                library.oracle_icdf_encode(state, requested, table, bits)
                actual = requested
            elif kind == 1:
                first, second = PARAMETERS[(random >> 8) % len(PARAMETERS)]
                requested = [0, -1, 1, -2, 2, -10, 10, -100, 100,
                             -10000, 10000, -20000, 20000][(random >> 4) % 13]
                value = ctypes.c_int(requested)
                library.oracle_laplace_encode(state, ctypes.byref(value), first, second)
                actual = value.value
            elif kind == 2:
                first, second = 1 + random % 16, 0
                requested = actual = (random >> 4) & 1
                library.oracle_ec_enc_bit_logp(state, requested, first)
            elif kind == 3:
                first, second = 1 + random % 25, 0
                requested = actual = (random >> 4) & ((1 << first) - 1)
                library.oracle_ec_enc_bits(state, requested, first)
            else:
                first = 0xFFFFFFFF if index % 2 else 2 + (random & 65535)
                second = 0
                requested = actual = (random >> 4) % first
                library.oracle_ec_enc_uint(state, requested, first)
            events.append((kind, first, second, requested & 0xFFFFFFFF, actual & 0xFFFFFFFF,
                           library.oracle_range(state), library.oracle_tell(state),
                           library.oracle_ec_tell_frac(state)))
        length = (library.oracle_tell(state) + 7) // 8
        library.oracle_ec_enc_shrink(state, length)
        library.oracle_ec_enc_done(state)
        if library.oracle_error(state):
            raise RuntimeError(f"reference encoder failed for frame {seed}")
        encoded = packet.raw[:length]
        verify_packet(library, encoded, events, seed)
        data += struct.pack("<HH", length, len(events)) + encoded
        for event in events:
            data += struct.pack("<BIIIIIII", *event)
    output.write_bytes(data)
    print(f"libopus 1.6.1: 32 frames, 3,072 operations, {len(data)} fixture bytes")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="installed libopus 1.6.1 static archive")
    parser.add_argument("--output", type=Path,
                        default=Path(__file__).resolve().parents[2] /
                        "src/opus/testdata/range-libopus-1.6.1.bin")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="snacka-opus-range-oracle-") as temporary:
        source = Path(temporary) / "bridge.c"
        source.write_text(Path(__file__).with_name("entropy_bridge.c").read_text() + BRIDGE)
        library = source.with_suffix(".so")
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", str(source), str(args.archive.resolve()),
                        "-lm", "-o", str(library)], check=True)
        oracle = ctypes.CDLL(str(library))
        configure(oracle)
        configure_models(oracle)
        generate(oracle, args.output)


if __name__ == "__main__":
    main()
