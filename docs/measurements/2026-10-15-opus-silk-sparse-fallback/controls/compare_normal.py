#!/usr/bin/env python3
"""Exact baseline/candidate archives and bounded public libopus wire validation."""
import ctypes as C
import hashlib
import json
import math
from pathlib import Path
import struct
import sys

HERE = Path(__file__).resolve().parent
BASE, CANDIDATE = (Path(p) for p in sys.argv[1:3])
opus = C.CDLL('/opt/homebrew/lib/libopus.dylib')
opus.opus_get_version_string.restype = C.c_char_p
opus.opus_decoder_create.argtypes = [C.c_int, C.c_int, C.POINTER(C.c_int)]
opus.opus_decoder_create.restype = C.c_void_p
opus.opus_decoder_destroy.argtypes = [C.c_void_p]
opus.opus_decode_float.argtypes = [C.c_void_p, C.c_void_p, C.c_int, C.POINTER(C.c_float), C.c_int, C.c_int]
opus.opus_decode_float.restype = C.c_int
opus.opus_decoder_ctl.argtypes = [C.c_void_p, C.c_int]
opus.opus_decoder_ctl.restype = C.c_int

def records(path):
    data = path.read_bytes()
    offset = 0
    while offset < len(data):
        length, interval, samples, flags, channels = struct.unpack_from('<5I', data, offset)
        offset += 20
        packet = data[offset:offset+length]
        assert len(packet) == length
        offset += length
        yield packet, interval, samples, flags, channels
    assert offset == len(data)

def decode_native(path, layer):
    native = None
    count = 0
    try:
        for packet, interval, samples, flags, channels in records(path):
            if native is None:
                error = C.c_int()
                native = opus.opus_decoder_create(16_000 if layer else 48_000, channels, C.byref(error))
                assert native and error.value == 0, error.value
            if flags & 1:
                assert opus.opus_decoder_ctl(native, 4028) == 0
            if layer:
                # Public raw WB/20ms SILK layer is framed with the specified TOC.
                packet = bytes([((4 * 2 + 1) << 3) | (4 if channels == 2 else 0)]) + packet
            output = (C.c_float * (samples * channels))()
            storage = C.create_string_buffer(packet)
            decoded = opus.opus_decode_float(native, storage, len(packet), output, samples, 0)
            assert decoded == samples, (path.name, count, decoded, samples)
            final_range = C.c_uint32()
            assert opus.opus_decoder_ctl(native, 4031, C.byref(final_range)) == 0
            assert final_range.value == interval, (path.name, count, hex(final_range.value), hex(interval))
            assert all(math.isfinite(x) for x in output), (path.name, count)
            count += 1
    finally:
        if native:
            opus.opus_decoder_destroy(native)
    return count

base_summary = [json.loads(x) for x in (BASE / 'streams.jsonl').read_text().splitlines()]
candidate_summary = [json.loads(x) for x in (CANDIDATE / 'streams.jsonl').read_text().splitlines()]
assert base_summary == candidate_summary, 'stream summaries differ'
result = {'baseline': str(BASE), 'candidate': str(CANDIDATE), 'libopus': opus.opus_get_version_string().decode(), 'streams': []}
for summary in base_summary:
    name = summary['name']
    details = dict(summary)
    for suffix in ['packets', 'pcm']:
        a = (BASE / f'{name}.{suffix}').read_bytes()
        b = (CANDIDATE / f'{name}.{suffix}').read_bytes()
        assert a == b, (name, suffix, next((i for i, (x,y) in enumerate(zip(a,b)) if x != y), None), len(a), len(b))
        details[f'{suffix}_sha256'] = hashlib.sha256(a).hexdigest()
        details[f'{suffix}_bytes'] = len(a)
    count = decode_native(CANDIDATE / f'{name}.packets', summary['kind'] == 'silk-layer')
    assert count == summary['packets']
    details['native_packets'] = count
    result['streams'].append(details)
result['stream_count'] = len(result['streams'])
result['packet_count'] = sum(x['packets'] for x in result['streams'])
result['fec_packets'] = sum(x['fec_packets'] for x in result['streams'])
result['transition_packets'] = sum(x['transition_packets'] for x in result['streams'])
(HERE / 'comparison.json').write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps({key: value for key, value in result.items() if key != 'streams'}, indent=2))
