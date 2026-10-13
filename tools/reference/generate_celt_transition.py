#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Generate the synthetic CELT-to-Hybrid reset regression packet.

This external reference tool uses public libopus APIs on locally generated
180/360/720 Hz tones. No official audio or external implementation source is
included. The retained packet was generated with libopus 1.6.1. Regeneration is
optional and has no role in ordinary Rust builds or test execution.

Example: python3 tools/reference/generate_celt_transition.py \
    --library /opt/homebrew/opt/opus/lib/libopus.dylib \
    --output src/opus/testdata/celt-to-hybrid.opus
"""

import argparse
import ctypes
import ctypes.util
import json
import math
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", default=ctypes.util.find_library("opus"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not args.library:
        parser.error("supply the path to an installed libopus reference library")
    c = ctypes
    library = c.CDLL(args.library)
    library.opus_encoder_create.argtypes = [c.c_int, c.c_int, c.c_int, c.POINTER(c.c_int)]
    library.opus_encoder_create.restype = c.c_void_p
    library.opus_encoder_ctl.argtypes = [c.c_void_p, c.c_int]
    library.opus_encoder_ctl.restype = c.c_int
    library.opus_encode_float.argtypes = [c.c_void_p, c.POINTER(c.c_float), c.c_int, c.POINTER(c.c_ubyte), c.c_int]
    library.opus_encode_float.restype = c.c_int
    library.opus_encoder_destroy.argtypes = [c.c_void_p]
    library.opus_get_version_string.restype = c.c_char_p
    error = c.c_int()
    encoder = library.opus_encoder_create(48000, 1, 2049, c.byref(error))
    if not encoder or error.value:
        raise RuntimeError(f"reference encoder creation failed: {error.value}")

    def control(request, value):
        status = library.opus_encoder_ctl(encoder, request, c.c_int(value))
        if status:
            raise RuntimeError(f"reference control {request} failed: {status}")

    try:
        control(4002, 128000)  # OPUS_SET_BITRATE
        control(4006, 0)  # OPUS_SET_VBR, constant byte budget
        control(4008, 1105)  # OPUS_SET_BANDWIDTH, fullband
        control(4024, 3002)  # OPUS_SET_SIGNAL, music
        pcm = (c.c_float * 960)()
        output = (c.c_ubyte * 1276)()
        configurations = []
        for frame in range(9):
            if frame == 8:
                control(4002, 32000)
                control(4024, 3001)  # OPUS_SIGNAL_VOICE
            for sample in range(960):
                time = (frame * 960 + sample) / 48000
                pcm[sample] = sum(amplitude * math.sin(2 * math.pi * frequency * time)
                                  for frequency, amplitude in ((180, 0.2), (360, 0.07), (720, 0.03)))
            size = library.opus_encode_float(encoder, pcm, 960, output, len(output))
            if size < 1:
                raise RuntimeError(f"reference encoding failed: {size}")
            packet = bytes(output[:size])
            configurations.append(packet[0] >> 3)
        if not all(configuration >= 16 for configuration in configurations[:8]) \
                or not 12 <= configurations[-1] <= 15:
            raise RuntimeError(f"reference version did not select the required transition: {configurations}")
        args.output.write_bytes(packet)
        print(json.dumps({"reference": library.opus_get_version_string().decode(),
                          "bytes": len(packet), "configurations": configurations,
                          "output": str(args.output)}))
    finally:
        library.opus_encoder_destroy(encoder)


if __name__ == "__main__":
    main()
