#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Offline black-box quality oracle; requires SNACKA_REFERENCE_OPUS_LIBRARY."""
import argparse
import ctypes as ct
import hashlib
import math
import os
from pathlib import Path

from evaluate_speech import floats, pcm


APPLICATION_RESTRICTED_LOWDELAY = 2051
SET_BITRATE, SET_VBR, SET_COMPLEXITY, SET_DTX = 4002, 4006, 4010, 4016
GET_LOOKAHEAD = 4027


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('bitrate', type=int)
    parser.add_argument('frame_ms', type=int, choices=(10, 20))
    args = parser.parse_args()
    if not 12000 <= args.bitrate <= 32000 or args.bitrate * args.frame_ms % 8000:
        parser.error('matched CBR requires 12000..32000 and an integral per-frame byte budget')
    path = os.environ.get('SNACKA_REFERENCE_OPUS_LIBRARY')
    if not path:
        parser.error('set SNACKA_REFERENCE_OPUS_LIBRARY to an installed shared library')
    library = Path(path).resolve(strict=True)
    values = floats(args.input.read_bytes())
    if not all(math.isfinite(v) and abs(v) <= 1 for v in values):
        parser.error('input must be finite normalized 48 kHz mono f32 PCM')
    lib = ct.CDLL(str(library))
    integer, pointer = ct.c_int, ct.c_void_p
    float_pointer, byte_pointer = ct.POINTER(ct.c_float), ct.POINTER(ct.c_ubyte)
    declarations = {
        'opus_encoder_create': ([integer, integer, integer, ct.POINTER(integer)], pointer),
        'opus_decoder_create': ([integer, integer, ct.POINTER(integer)], pointer),
        'opus_encoder_destroy': ([pointer], None),
        'opus_decoder_destroy': ([pointer], None),
        'opus_encode_float': ([pointer, float_pointer, integer, byte_pointer, integer], integer),
        'opus_decode_float': ([pointer, byte_pointer, integer, float_pointer, integer, integer], integer),
        'opus_get_version_string': ([], ct.c_char_p),
        # Declare the fixed arguments for the variadic ABI, including Apple ARM64.
        'opus_encoder_ctl': ([pointer, integer], integer),
    }
    for name, (arguments, result) in declarations.items():
        function = getattr(lib, name)
        function.argtypes, function.restype = arguments, result

    def check(result):
        if result < 0:
            raise RuntimeError(f'Opus API returned error {result}')
        return result

    error = integer()
    encoder = lib.opus_encoder_create(48000, 1, APPLICATION_RESTRICTED_LOWDELAY, ct.byref(error))
    check(error.value)
    if not encoder:
        raise RuntimeError('Opus returned no encoder')
    decoder = None
    try:
        decoder = lib.opus_decoder_create(48000, 1, ct.byref(error))
        check(error.value)
        if not decoder:
            raise RuntimeError('Opus returned no decoder')
        for request, value in ((SET_BITRATE, args.bitrate), (SET_VBR, 0), (SET_COMPLEXITY, 10), (SET_DTX, 0)):
            check(lib.opus_encoder_ctl(encoder, request, integer(value)))
        delay = integer()
        check(lib.opus_encoder_ctl(encoder, GET_LOOKAHEAD, ct.byref(delay)))
        if delay.value != 120:
            raise RuntimeError('restricted-lowdelay reference does not match the 120-sample delay')
        samples = 48 * args.frame_ms
        budget = args.bitrate * args.frame_ms // 8000
        packet = (ct.c_ubyte * budget)()
        decoded_frame = (ct.c_float * samples)()
        decoded = []
        packet_bytes, drain_bytes = 0, 0
        ordinary = (len(values) + samples - 1) // samples
        for index in range(ordinary + math.ceil(delay.value / samples)):
            chunk = values[index * samples:(index + 1) * samples]
            frame = (ct.c_float * samples)(*chunk)
            length = check(lib.opus_encode_float(encoder, frame, samples, packet, budget))
            if length != budget:
                raise RuntimeError('Opus did not return the matched constant byte budget')
            count = check(lib.opus_decode_float(decoder, packet, length, decoded_frame, samples, 0))
            if count != samples:
                raise RuntimeError('Opus returned a different frame duration')
            decoded.extend(decoded_frame)
            if index < ordinary:
                packet_bytes += length
            else:
                drain_bytes += length
        output = decoded[delay.value:delay.value + len(values)]
        if len(output) != len(values) or not all(math.isfinite(v) for v in output):
            raise RuntimeError('invalid decoded PCM')
        version = lib.opus_get_version_string().decode('ascii').replace(' ', '_')
        digest = hashlib.sha256(library.read_bytes()).hexdigest()
        args.output.write_bytes(pcm(output))
        print(f'samples={len(values)},frame_ms={args.frame_ms},packet_bytes={packet_bytes},'
              f'drain_bytes={drain_bytes},algorithmic_delay_samples={delay.value},codec=libopus,'
              f'reference_version={version},reference_library_sha256={digest},'
              'reference_application=restricted_lowdelay,reference_complexity=10,reference_vbr=0,reference_dtx=0')
    finally:
        if decoder:
            lib.opus_decoder_destroy(decoder)
        lib.opus_encoder_destroy(encoder)


if __name__ == '__main__':
    main()
