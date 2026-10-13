#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Generate start-band-17 folding regressions against installed libopus 1.6.1.

RFC 8251 section 9 updates Hybrid folding to extend its 8-bin first band for
the 12-bin second band. This file calls the external reference through its ABI;
no codec algorithms or source are included in the Rust build or this bridge.
"""

import argparse
import ctypes
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

from generate_entropy_fixtures import configure


BRIDGE = """
extern const void *opus_custom_mode_create(int, int, int *);
extern void quant_all_bands(int, const void *, int, int, float *, float *,
    unsigned char *, const float *, int *, int, int, int, int, int *, int,
    int, void *, int, int, uint32_t *, int, int, int);
void oracle_hybrid_bands(void *state, int lm, int channels, int transient,
    int choice, uint32_t *seed, float *x, unsigned char *masks) {
    int error;
    const void *mode = opus_custom_mode_create(48000, 960, &error);
    const int budget[3] = {0, 72, 360};
    const int intensity[3] = {17, 19, 21};
    int pulses[21] = {0}, tf[21] = {0};
    float amplitudes[42];
    for (int i = 0; i < 21; ++i) {
        if (i >= 17) pulses[i] = budget[choice];
        amplitudes[i] = amplitudes[i+21] = 1;
    }
    quant_all_bands(0, mode, 17, 21, x, channels == 2 ? x+960 : 0,
        masks, amplitudes, pulses, transient ? 1<<lm : 0, 2,
        channels == 2 && choice == 1, intensity[choice], tf, 4000*64,
        0, state, lm, 21, seed, 0, 0, 0);
}
"""


def generate(library, output):
    library.oracle_hybrid_bands.argtypes = [ctypes.c_void_p, ctypes.c_int,
        ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_void_p,
        ctypes.c_void_p, ctypes.c_void_p]
    library.oracle_hybrid_bands.restype = None
    data = bytearray(b"SNKHYB01" + struct.pack("<I", 24))
    for lm in [2, 3]:
        for transient in [0, 1]:
            for channels in [1, 2]:
                for choice in range(3):
                    packet = ctypes.create_string_buffer(bytes(
                        (i*73+lm*17+transient*31+channels*49+2*11+choice*29)&255
                        for i in range(4000)))
                    state = (ctypes.c_uint64 * 32)()
                    library.oracle_ec_dec_init(state, packet, 4000)
                    coefficients = (ctypes.c_float * 1920)()
                    masks = (ctypes.c_ubyte * 42)()
                    seed = ctypes.c_uint32(0xc0dec0de)
                    library.oracle_hybrid_bands(state, lm, channels, transient,
                        choice, ctypes.byref(seed), coefficients, masks)
                    data += struct.pack("<8I", lm, transient, channels, choice,
                        library.oracle_range(state), library.oracle_ec_tell_frac(state),
                        seed.value, library.oracle_error(state))
                    data += bytes(masks)
                    for channel in range(channels):
                        data += struct.pack(f"<{60<<lm}f", *coefficients[
                            channel*960+(40<<lm):channel*960+(100<<lm)])
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(data)
    print(f"24 Hybrid band cases, {len(data)} bytes: {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="installed libopus 1.6.1 static archive")
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[2]
                        / "src/opus/testdata/bands-hybrid-libopus-1.6.1.bin")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="snacka-hybrid-bands-") as temporary:
        source = Path(temporary) / "bridge.c"
        source.write_text(Path(__file__).with_name("entropy_bridge.c").read_text() + BRIDGE)
        library = source.with_suffix(".so")
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", str(source), str(args.archive.resolve()),
                        "-lm", "-o", str(library)], check=True)
        oracle = ctypes.CDLL(str(library))
        configure(oracle)
        generate(oracle, args.output)


if __name__ == "__main__":
    main()
