# SPDX-License-Identifier: MIT
"""Generate CELT CWRS vectors using only installed libopus's external functions.

No implementation source or precomputed table is read. Independent combinatorial
counts select every N=2..176, K=1..128 codebook in libopus's 31-bit table domain.
Mathematical 32-bit edge codebooks exceed that private oracle table. Uniform reference
integers are decoded into reference vectors and re-encoded as pulse vectors;
their bytes, final interval and bit usage must agree. The small default fixture
samples all dimensions and boundary cases; --full saves the entire comparison.
"""

import argparse
import ctypes
from math import comb
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

from generate_entropy_fixtures import configure


BRIDGE = """
extern void encode_pulses(const int *, int, int, void *);
extern void decode_pulses(int *, int, int, void *);
void oracle_pvq_encode(void *s, int n, int k, const int *y) {
    encode_pulses(y, n, k, s);
}
void oracle_pvq_decode(void *s, int n, int k, int *y) {
    decode_pulses(y, n, k, s);
}
"""


def total(n, k):
    return sum(comb(n, m) * comb(k - 1, m - 1) * (1 << m)
               for m in range(1, min(n, k) + 1))


def configure_pvq(library):
    pointer, integer = ctypes.c_void_p, ctypes.c_int
    for name in ["pvq_encode", "pvq_decode"]:
        function = getattr(library, "oracle_" + name)
        function.argtypes = [pointer, integer, integer, pointer]
        function.restype = None


def encode(library, n, k, count, index, vector=None):
    state = (ctypes.c_uint64 * 32)()
    packet = ctypes.create_string_buffer(8)
    library.oracle_ec_enc_init(state, packet, 8)
    if vector is None:
        library.oracle_ec_enc_uint(state, index, count)
    else:
        library.oracle_pvq_encode(state, n, k, vector)
    interval = library.oracle_range(state)
    tell = library.oracle_tell(state)
    fraction = library.oracle_ec_tell_frac(state)
    library.oracle_ec_enc_done(state)
    if library.oracle_error(state):
        raise RuntimeError(f"reference encode failed: N={n}, K={k}, i={index}")
    return packet.raw, interval, tell, fraction


def generate(library, output, full):
    cases, codebooks, checked = [], 0, 0
    random = 0xC3116716
    for n in range(2, 177):
        for k in range(1, 129):
            count = total(n, k)
            # Calling private CWRS entry points outside the table domain does
            # not exercise a supported libopus band path. Its split allocator
            # keeps these larger combinations out of decode_pulses.
            if count > 0x7FFFFFFF:
                break
            codebooks += 1
            indices = [0, count // 2, count - 1]
            for _ in range(4):
                random ^= (random << 13) & 0xFFFFFFFF
                random ^= random >> 17
                random ^= (random << 5) & 0xFFFFFFFF
                indices.append(random % count)
            if count <= 64:
                indices = range(count)
            for index in dict.fromkeys(indices):
                encoded = encode(library, n, k, count, index)
                packet, interval, tell, fraction = encoded
                state = (ctypes.c_uint64 * 32)()
                storage = ctypes.create_string_buffer(packet)
                vector = (ctypes.c_int * n)()
                library.oracle_ec_dec_init(state, storage, 8)
                library.oracle_pvq_decode(state, n, k, vector)
                actual = (library.oracle_range(state), library.oracle_tell(state),
                          library.oracle_ec_tell_frac(state))
                if actual != (interval, tell, fraction) or library.oracle_error(state):
                    raise RuntimeError(f"reference count disagreement: N={n}, K={k}, V={count}, i={index}, "
                                       f"expected={(interval, tell, fraction)}, actual={actual}")
                if sum(abs(value) for value in vector) != k:
                    raise RuntimeError(f"reference pulse count failed: N={n}, K={k}")
                if encode(library, n, k, count, index, vector) != encoded:
                    raise RuntimeError(f"reference rank disagreement: N={n}, K={k}, i={index}")
                checked += 1
                # Keep at least one vector for every N, together with low-N,
                # high-K and pseudorandom rank boundaries. Full is reproducible.
                keep = full or (k == 1 and index == count - 1) or (
                    n in [2, 3, 5, 8, 16, 22, 44, 88, 176]
                    and k in [2, 4, 8, 16, 32, 64, 128])
                if keep:
                    cases.append((n, k, count, index, interval, tell, fraction, packet, vector[:]))
    data = bytearray(b"SNKPVQ01" + struct.pack("<I", len(cases)))
    for n, k, count, index, interval, tell, fraction, packet, vector in cases:
        data += struct.pack("<HHIIIII", n, k, count, index, interval, tell, fraction)
        data += packet + struct.pack("<" + "h" * n, *vector)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(data)
    print(f"libopus 1.6.1: {codebooks:,} codebooks, {checked:,} exact vector/entropy comparisons; "
          f"{len(cases):,} fixtures, {len(data):,} bytes")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="installed libopus 1.6.1 static archive")
    parser.add_argument("--output", type=Path,
                        default=Path(__file__).resolve().parents[2] /
                        "src/opus/testdata/pvq-libopus-1.6.1.bin")
    parser.add_argument("--full", action="store_true", help="save every differential case")
    parser.add_argument("--verify-native", action="store_true",
                        help="compile the independent Rust probe and check every saved case")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="snacka-opus-pvq-oracle-") as temporary:
        source = Path(temporary) / "bridge.c"
        source.write_text(Path(__file__).with_name("entropy_bridge.c").read_text() + BRIDGE)
        library = source.with_suffix(".so")
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", str(source), str(args.archive.resolve()),
                        "-lm", "-o", str(library)], check=True)
        oracle = ctypes.CDLL(str(library))
        configure(oracle)
        configure_pvq(oracle)
        generate(oracle, args.output, args.full)
        if args.verify_native:
            probe = Path(temporary) / "pvq-probe"
            subprocess.run(["rustc", "--edition=2024", "-O", "-C", "overflow-checks=yes",
                            str(Path(__file__).with_name("pvq_probe.rs")), "-o", str(probe)],
                           check=True)
            subprocess.run([str(probe), str(args.output)], check=True)


if __name__ == "__main__":
    main()
