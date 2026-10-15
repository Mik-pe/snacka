#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Probe public native PCM entry points; no codec implementation source is read.

This records format scaling, clipping and rounding observations from one
installed libopus build. It does not establish codec audio or speed parity.
"""

import argparse
import ctypes as ct
import hashlib
import json
import math
from pathlib import Path
import platform

from check_opus_rtp import library

SAMPLES = 960


def prepare(path):
    lib = library(path)
    for name, sample in (("opus_encode", ct.c_int16), ("opus_encode24", ct.c_int32)):
        function = getattr(lib, name)
        function.argtypes = [ct.c_void_p, ct.POINTER(sample), ct.c_int,
                             ct.POINTER(ct.c_ubyte), ct.c_int]
        function.restype = ct.c_int
    for name, sample in (("opus_decode", ct.c_int16), ("opus_decode24", ct.c_int32)):
        function = getattr(lib, name)
        function.argtypes = [ct.c_void_p, ct.POINTER(ct.c_ubyte), ct.c_int,
                             ct.POINTER(sample), ct.c_int, ct.c_int]
        function.restype = ct.c_int
    lib.opus_pcm_soft_clip.argtypes = [ct.POINTER(ct.c_float), ct.c_int, ct.c_int,
                                       ct.POINTER(ct.c_float)]
    lib.opus_pcm_soft_clip.restype = None
    return lib


def encoder(lib, lsb=24):
    error = ct.c_int()
    state = lib.opus_encoder_create(48_000, 1, 2051, ct.byref(error))
    assert state and error.value == 0
    # Public controls: fixed bitrate, CBR, complexity and input precision.
    for request, value in ((4002, 96_000), (4006, 0), (4010, 10), (4036, lsb)):
        assert lib.opus_encoder_ctl(state, request, ct.c_int(value)) == 0
    return state


def encode(lib, state, values, api="opus_encode_float", kind=ct.c_float):
    pcm = (kind * SAMPLES)(*values)
    data = (ct.c_ubyte * 1276)()
    length = getattr(lib, api)(state, pcm, SAMPLES, data, len(data))
    assert length > 0, length
    return bytes(data[:length])


def packets(lib):
    state = encoder(lib)
    try:
        return [encode(lib, state, [
            0.8 * math.sin((frame * SAMPLES + index) * 0.057)
            + 0.13 * math.sin((frame * SAMPLES + index) * 0.113)
            for index in range(SAMPLES)]) for frame in range(8)]
    finally:
        lib.opus_encoder_destroy(state)


def integer_encoder(lib):
    states = [encoder(lib, 16) for _ in range(3)]
    equal = []
    try:
        for frame in range(4):
            values = [round(30_000 * math.sin((frame * SAMPLES + index) * 0.073))
                      for index in range(SAMPLES)]
            encoded = [encode(lib, states[0], values, "opus_encode", ct.c_int16),
                       encode(lib, states[1], [value * 256 for value in values],
                              "opus_encode24", ct.c_int32),
                       encode(lib, states[2], [value / 32768 for value in values])]
            equal.append(encoded[0] == encoded[1] == encoded[2])
    finally:
        for state in states:
            lib.opus_encoder_destroy(state)
    state = encoder(lib)
    try:
        # The public 24-bit API admits samples slightly above the nominal range.
        extended_input = [round(8_500_000 * math.sin(index * 0.057))
                          for index in range(SAMPLES)]
        extended = encode(lib, state, extended_input,
                          "opus_encode24", ct.c_int32)
    finally:
        lib.opus_encoder_destroy(state)
    return {"matched_lsb_depth": 16, "matching_i16_i24_float_packets": equal,
            "extended_i24_input_peak": max(abs(value) for value in extended_input),
            "extended_i24_packet_bytes": len(extended)}


def inspect(path):
    lib = prepare(path)
    result = {
        "reference": lib.opus_get_version_string().decode(),
        "reference_library": str(path),
        "reference_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "platform": platform.platform(),
        "scope": "public PCM API observations only; no codec audio or CPU parity",
        "rounding_oracle": "Python arbitrary-precision integers and nearest/even round of exactly represented binary floats",
        "integer_encoder": integer_encoder(lib),
        "decoder_cases": [],
    }
    encoded = packets(lib)
    for gain_db in (0, 12, 60):
        error = ct.c_int()
        states = [lib.opus_decoder_create(48_000, 1, ct.byref(error)) for _ in range(3)]
        assert all(states) and error.value == 0
        memory = (ct.c_float * 1)()
        try:
            for state in states:
                assert lib.opus_decoder_ctl(state, 4034, ct.c_int(gain_db * 256)) == 0
            for frame, packet in enumerate(encoded):
                data = (ct.c_ubyte * len(packet)).from_buffer_copy(packet)
                floating = (ct.c_float * SAMPLES)()
                integer16 = (ct.c_int16 * SAMPLES)()
                integer24 = (ct.c_int32 * SAMPLES)()
                for state, api, output in zip(states,
                        ("opus_decode_float", "opus_decode", "opus_decode24"),
                        (floating, integer16, integer24)):
                    assert getattr(lib, api)(state, data, len(packet), output, SAMPLES, 0) == SAMPLES
                hard16 = [min(32767, max(-32768, round(value * 32768))) for value in floating]
                extended24 = [min(2147483647, max(-2147483648, round(value * 8388608)))
                              for value in floating]
                soft = (ct.c_float * SAMPLES)(*floating)
                lib.opus_pcm_soft_clip(soft, SAMPLES, 1, memory)
                # Observed native i16 conversion can use half ties away from zero.
                soft16 = [min(32767, max(-32768,
                          int(math.copysign(math.floor(abs(value * 32768) + 0.5), value))))
                          for value in soft]
                half24 = sum(abs(value * 8388608) % 1 == 0.5 for value in floating)
                mismatches24 = sum(actual != wanted for actual, wanted in zip(integer24, extended24))
                assert mismatches24 == 0, (gain_db, frame, mismatches24)
                result["decoder_cases"].append({
                    "gain_db": gain_db, "frame": frame,
                    "float_peak": max(abs(value) for value in floating),
                    "i16_min": min(integer16), "i16_max": max(integer16),
                    "i24_min": min(integer24), "i24_max": max(integer24),
                    "i16_hard_ties_even_mismatches": sum(a != b for a, b in zip(integer16, hard16)),
                    "i16_soft_ties_away_mismatches": sum(a != b for a, b in zip(integer16, soft16)),
                    "i24_full_i32_ties_even_mismatches": mismatches24,
                    "i24_exact_half_ties": half24,
                })
        finally:
            for state in states:
                lib.opus_decoder_destroy(state)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    serialized = json.dumps(inspect(args.library.resolve()), indent=2) + "\n"
    if args.output:
        args.output.write_text(serialized)
    else:
        print(serialized, end="")


if __name__ == "__main__":
    main()
