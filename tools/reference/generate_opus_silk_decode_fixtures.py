# SPDX-License-Identifier: MIT
"""Freeze complete SILK layer decoding through a separate libopus 1.6.1 oracle.

The synthetic signal contains two harmonics and independent stereo channels.
The fixtures cover 8/12/16 kHz internal rates, mono/stereo, 10/20/40/60 ms
frames, and in-band redundancy. One-byte concealment frames are excluded;
both decoders reset before the next recorded coded frame.
"""

import argparse
import ctypes
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

from generate_entropy_fixtures import configure

ROOT = Path(__file__).resolve().parents[2]


def configure_packets(library):
    pointer, integer = ctypes.c_void_p, ctypes.c_int
    signatures = {
        "encoder_create": ([integer, integer, integer, pointer], pointer),
        "encoder_destroy": ([pointer], None),
        "encoder_ctl": ([pointer, integer, integer], integer),
        "encode_float": ([pointer, pointer, integer, pointer, integer], integer),
        "packet_parse": ([pointer, integer, pointer, pointer, pointer, pointer], integer),
        "decoder_create": ([integer, integer, pointer], pointer),
        "decoder_destroy": ([pointer], None),
        "decoder_reset": ([pointer], integer),
        "decoder_range": ([pointer, pointer], integer),
        "decode": ([pointer, pointer, integer, pointer, integer], integer),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(library, "oracle_opus_" + name)
        function.argtypes, function.restype = arguments, result


def generate(library, path):
    cases, stream = [], 0
    for band, rate in enumerate((8000, 12000, 16000)):
        for duration in (10, 20, 40, 60):
            for channels in (1, 2):
                error = ctypes.c_int()
                encoder = library.oracle_opus_encoder_create(rate, channels, 2048,
                                                              ctypes.byref(error))
                if error.value or not encoder:
                    raise RuntimeError("reference encoder creation failed")
                decoder = library.oracle_opus_decoder_create(rate, channels, ctypes.byref(error))
                if error.value or not decoder:
                    raise RuntimeError("reference decoder creation failed")
                try:
                    # Public Opus control constants: bitrate, bandwidth limits,
                    # voice signal, constant rate, FEC and expected loss.
                    settings = ((4002, 24000 * channels), (4008, 1101 + band),
                                (4004, 1101 + band), (4024, 3001), (4006, 0),
                                (4012, 1), (4014, 25))
                    for request, value in settings:
                        if library.oracle_opus_encoder_ctl(encoder, request, value):
                            raise RuntimeError(f"reference encoder rejected control {request}")
                    reset = 1
                    count = rate * duration // 1000
                    for frame in range(40):
                        signal = (ctypes.c_float * (count * channels))()
                        for i in range(count):
                            time = (frame * count + i) / rate
                            amplitude = 0.0 if frame in (0, 11, 12) else (0.12 if frame % 3 else 0.3)
                            for channel in range(channels):
                                signal[i * channels + channel] = amplitude * (
                                    math.sin(2 * math.pi * (180 + channel * 31) * time)
                                    + 0.3 * math.sin(2 * math.pi * 540 * time))
                        packet = (ctypes.c_ubyte * 1275)()
                        length = library.oracle_opus_encode_float(encoder, signal, count,
                                                                   packet, len(packet))
                        if length <= 0:
                            raise RuntimeError("reference encoding failed")
                        # Let encoder rate/bandwidth adaptation settle, then
                        # start the independent decoder at a coded boundary.
                        if frame < 20:
                            continue
                        toc = packet[0]
                        if toc >> 3 >= 12 or toc >> 5 != band or bool(toc & 4) != (channels == 2):
                            raise RuntimeError("reference switched away from requested SILK geometry")
                        frames = (ctypes.c_void_p * 48)()
                        sizes = (ctypes.c_int16 * 48)()
                        offset, parsed_toc = ctypes.c_int(), ctypes.c_ubyte()
                        parts = library.oracle_opus_packet_parse(packet, length,
                                                                 ctypes.byref(parsed_toc), frames,
                                                                 sizes, ctypes.byref(offset))
                        if parts != 1:
                            raise RuntimeError("reference packet contains multiple Opus frames")
                        payload = ctypes.string_at(frames[0], sizes[0])
                        if len(payload) <= 1:
                            if library.oracle_opus_decoder_reset(decoder):
                                raise RuntimeError("reference decoder reset failed")
                            reset = 1
                            continue
                        pcm = (ctypes.c_int16 * (count * channels))()
                        produced = library.oracle_opus_decode(decoder, packet, length, pcm, count)
                        final_range = ctypes.c_uint32()
                        if produced != count or library.oracle_opus_decoder_range(
                                decoder, ctypes.byref(final_range)):
                            raise RuntimeError("reference decoder failed")
                        if final_range.value == 0:
                            raise RuntimeError("reference concealed a recorded coded frame")
                        cases.append(struct.pack("<HBBBHHIII", stream, band, duration, channels,
                                                 len(payload), count, final_range.value, reset, 0)
                                     + payload + bytes(pcm))
                        reset = 0
                finally:
                    library.oracle_opus_encoder_destroy(encoder)
                    library.oracle_opus_decoder_destroy(decoder)
                stream += 1
    data = b"SNKSLD01" + struct.pack("<I", len(cases)) + b"".join(cases)
    path.write_bytes(data)
    print(f"libopus 1.6.1: {len(cases)} coded SILK frames, {stream} streams, {len(data)} bytes")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="separately installed libopus 1.6.1 static archive")
    parser.add_argument("--output", type=Path, default=ROOT / "tests/fixtures/opus-silk-decode.bin")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="snacka-silk-decode-oracle-") as temporary:
        path = Path(temporary) / "oracle.so"
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", str(Path(__file__).with_name("entropy_bridge.c")),
                        str(Path(__file__).with_name("silk_packet_bridge.c")),
                        str(args.archive.resolve()), "-lm", "-o", str(path)], check=True)
        library = ctypes.CDLL(str(path))
        configure(library)
        configure_packets(library)
        generate(library, args.output)


if __name__ == "__main__":
    main()
