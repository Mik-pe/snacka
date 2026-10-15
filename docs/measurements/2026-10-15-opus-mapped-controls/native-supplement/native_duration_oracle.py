"""SUPPLEMENT SQUARE GEOMETRY ONLY: public duration observations, no encoder or timers.

Do not execute before root releases the CPU, reviews this source and freezes its
source/tool/library/header/old-fixture manifest. There is no native selection or
partial-error assertion in this tool. Keep every unexpected row and raw buffer.
"""
import argparse
import ctypes as C
import json
import math
import pathlib
import struct
import sys

I, P, U8 = C.c_int, C.c_void_p, C.c_ubyte
IP, FP, BP = C.POINTER(I), C.POINTER(C.c_float), C.POINTER(U8)
LAST, RESET, CHILD, FINAL = 4039, 4028, 5122, 4031
SENTINEL = 0x7FC01234
RATES = (8000, 12000, 16000, 24000, 48000)
MODES = ('silk', 'hybrid', 'celt', 'silk-fec')
# Fixed before any native result: none is assumed to produce a content error.
LATE_PAYLOADS = (
    ('zero2', bytes.fromhex('0000')),
    ('ones2', bytes.fromhex('ffff')),
    ('zero3', bytes.fromhex('000000')),
    ('ones3', bytes.fromhex('ffffff')),
    ('zero8', bytes(8)),
    ('ones8', bytes([255]) * 8),
)
# Supplemental supported square projection: three streams, first stereo and
# two mono; C=K=4. Original rectangular constructor failures remain retained.
LAYOUTS = (('projection', 'square', (0, 1, 2, 3)),)


def declare(lib):
    lib.opus_get_version_string.argtypes = []
    lib.opus_get_version_string.restype = C.c_char_p
    for prefix in ('opus', 'opus_multistream', 'opus_projection'):
        ctl = getattr(lib, prefix + '_decoder_ctl')
        # Fixed variadic prefix is essential on Apple Silicon. Every extra
        # argument below is explicitly a promoted int or live typed pointer.
        ctl.argtypes, ctl.restype = [P, I], I
        if prefix != 'opus':
            # Audio processing and destruction are parent-only. Borrowed
            # ordinary children are used only for documented CTL operations.
            decode = getattr(lib, prefix + '_decode_float')
            decode.argtypes, decode.restype = [P, P, I, FP, I, I], I
            destroy = getattr(lib, prefix + '_decoder_destroy')
            destroy.argtypes, destroy.restype = [P], None
    lib.opus_multistream_decoder_create.argtypes = [I, I, I, I, P, IP]
    lib.opus_multistream_decoder_create.restype = P
    lib.opus_projection_decoder_create.argtypes = [I, I, I, I, P, I, IP]
    lib.opus_projection_decoder_create.restype = P
    lib.opus_packet_parse.argtypes = [BP, I, BP, C.POINTER(BP), C.POINTER(C.c_short), IP]
    lib.opus_packet_parse.restype = I


def emit(row):
    SINK.write(json.dumps(row, separators=(',', ':'), allow_nan=False) + '\n')
    SINK.flush()


def frame_size(length):
    # RFC6716 section3 length syntax, not an encoder/decoder implementation.
    if not 0 <= length <= 1275:
        raise ValueError(('invalid-frame-length', length))
    if length < 252:
        return bytes([length])
    first = 252 + (length & 3)
    return bytes([first, (length - first) // 4])


def packet_from_frames(packet, self_delimited=False):
    toc, frames = packet
    if not 1 <= len(frames) <= 48:
        raise ValueError(('invalid-frame-count', len(frames)))
    # Canonical code3 VBR, no padding. AppendixB's self-delimited form adds the
    # final frame's size after the ordinary first(n-1) sizes and before payload.
    sizes = frames if self_delimited else frames[:-1]
    return bytes([(toc & 0xFC) | 3, 0x80 | len(frames)]) + b''.join(frame_size(len(f)) for f in sizes) + b''.join(frames)


def transport(pair):
    return b''.join(packet_from_frames(packet, index + 1 < len(pair)) for index, packet in enumerate(pair))


def parse_packet(lib, payload, label):
    data = (U8 * len(payload)).from_buffer_copy(payload)
    toc, offset = U8(), I(-999)
    pointers, sizes = (BP * 48)(), (C.c_short * 48)()
    returned = int(lib.opus_packet_parse(data, len(payload), C.byref(toc), pointers, sizes, C.byref(offset)))
    emit({'kind': 'public-packet-parse', 'label': label, 'code': returned,
          'toc': toc.value if returned > 0 else None, 'payload_offset': offset.value,
          'sizes': list(sizes[:max(0, min(returned, 48))])})
    if not 1 <= returned <= 48 or any(not 0 <= sizes[i] <= 1275 for i in range(max(0, min(returned, 48)))):
        raise RuntimeError(('existing-fixture-parse-failure', label, returned))
    return toc.value, [C.string_at(pointers[i], sizes[i]) for i in range(returned)]


def load_bank(lib, path, label):
    data = path.read_bytes()
    cursor, raw = 0, []
    while cursor < len(data):
        if cursor + 4 > len(data):
            raise RuntimeError(('truncated-old-bank-length', str(path), cursor))
        length = int.from_bytes(data[cursor:cursor + 4], 'little')
        cursor += 4
        if cursor + length > len(data):
            raise RuntimeError(('truncated-old-bank-packet', str(path), cursor, length))
        raw.append(data[cursor:cursor + length])
        cursor += length
    if len(raw) < 6:
        raise RuntimeError(('old-bank-too-short', str(path), len(raw)))
    emit({'kind': 'old-bank', 'path': str(path), 'records': len(raw), 'used_records': list(range(6))})
    return [parse_packet(lib, raw[i], label + '-' + str(i)) for i in range(6)]


class Decoder:
    def __init__(self, lib, root, family, layout, mapping, rate, mode):
        self.lib, self.root = lib, root
        self.family, self.layout = family, layout
        self.rate, self.mode, self.channels = rate, mode, len(mapping)
        self.state, self.serial = None, 0
        self.streams = 3 if family == 'projection' else 2
        self.prefix = 'opus_' + family if family == 'projection' else 'opus_multistream'
        self.context = {'family': family, 'layout': layout, 'rate': rate, 'mode': mode}
        error = I(-999)
        if family == 'mapped':
            data = (U8 * len(mapping))(*mapping)
            state = lib.opus_multistream_decoder_create(rate, len(mapping), 2, 1, data, C.byref(error))
        else:
            # The existing owned/public projection contract is column-major,
            # signed Q15 little-endian; all four coded channels have output rows.
            coefficients = [32767, 0, 0, 0, 0, -32768, 0, 0, 0, 0, 32767, 0, 0, 0, 0, 32767]
            matrix = struct.pack('<16h', *coefficients)
            data = C.create_string_buffer(matrix)
            state = lib.opus_projection_decoder_create(rate, 4, 3, 1, data, len(matrix), C.byref(error))
        classification = ('usable' if state and error.value == 0 else
                          'unsupported' if error.value == -5 else
                          'invalid-success-null' if error.value == 0 else 'error')
        emit(self.context | {'kind': 'create', 'code': error.value, 'nonnull': bool(state), 'classification': classification})
        if state and error.value != 0:
            getattr(lib, self.prefix + '_decoder_destroy')(state)
        elif state:
            self.state = state

    def close(self):
        if self.state:
            getattr(self.lib, self.prefix + '_decoder_destroy')(self.state)
            self.state = None

    def ctl(self, request, *arguments):
        return int(getattr(self.lib, self.prefix + '_decoder_ctl')(self.state, request, *arguments))

    def child(self, index):
        child = P()
        code = self.ctl(CHILD, I(index), C.byref(child))
        return code, child

    def values(self, prefix, state):
        count, interval = I(-12345), C.c_uint32(0xDEADBEEF)
        code = int(getattr(self.lib, prefix + '_decoder_ctl')(state, LAST, C.byref(count)))
        interval_code = int(getattr(self.lib, prefix + '_decoder_ctl')(state, FINAL, C.byref(interval)))
        return {'duration_code': code, 'samples': count.value, 'range_code': interval_code, 'range': interval.value}

    def snapshot(self):
        result = {'aggregate': self.values(self.prefix, self.state), 'children': []}
        for index in range(self.streams):
            code, child = self.child(index)
            result['children'].append({'stream': index, 'state_code': code, 'nonnull': bool(child),
                                       'status': self.values('opus', child) if code == 0 and child else None})
        return result

    def observe(self, label):
        result = self.snapshot()
        emit(self.context | {'kind': 'observe', 'event': label, 'status': result})
        return result

    def reset(self, label, child_index=None):
        before = self.snapshot()
        if child_index is None:
            code = self.ctl(RESET)
            child_code = None
        else:
            child_code, child = self.child(child_index)
            code = int(self.lib.opus_decoder_ctl(child, RESET)) if child_code == 0 and child else None
        emit(self.context | {'kind': 'reset', 'event': label, 'child': child_index,
                            'state_code': child_code, 'code': code, 'before': before, 'after': self.snapshot()})

    def step(self, label, payload, capacity, fec=False):
        # The physical allocation always holds120ms+7guards. Invalid duration
        # and small-output cases only reduce the advertised capacity.
        maximum = self.rate * 120 // 1000
        if not 0 <= capacity <= maximum:
            raise ValueError(('unsafe-advertised-capacity', capacity, maximum))
        before = self.snapshot()
        channels = self.channels
        words = (C.c_uint32 * (maximum * channels + 7))(*([SENTINEL] * (maximum * channels + 7)))
        pcm = C.cast(words, FP)
        data = C.create_string_buffer(payload) if payload is not None else None
        code = int(getattr(self.lib, self.prefix + '_decode_float')(self.state, data, 0 if payload is None else len(payload), pcm, capacity, int(fec)))
        stem = f'{self.family}-{self.layout}-{self.rate}-{self.mode}-{self.serial:03}'
        self.serial += 1
        filename = stem + '.pcm.f32le'
        # Array words preserve every output/NaN/sentinel bit. Serialize explicitly
        # little-endian rather than relying on native byte order.
        (self.root / filename).write_bytes(b''.join(struct.pack('<I', word) for word in words))
        packet_file = None
        if payload is not None:
            packet_file = stem + '.opus'
            (self.root / packet_file).write_bytes(payload)
        valid_return = 0 <= code <= capacity
        written = code * channels if valid_return else 0
        finite = all(math.isfinite(pcm[i]) for i in range(written)) if valid_return else None
        suffix = all(word == SENTINEL for word in words[written:]) if valid_return else None
        after = self.snapshot()
        row = self.context | {'kind': 'step', 'event': label,
                              'packet': packet_file, 'fec_requested': bool(fec), 'advertised_samples': capacity,
                              'code': code, 'valid_return': valid_return, 'channels': channels,
                              'physical_floats': len(words), 'guard_floats': 7, 'pcm': filename,
                              'finite_returned_pcm': finite, 'suffix_unchanged': suffix,
                              'before': before, 'after': after}
        emit(row)
        return row


def child_samples(snapshot, index):
    value = snapshot['children'][index]
    status = value['status']
    return status['samples'] if value['state_code'] == 0 and status and status['duration_code'] == 0 else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-native-after-freeze', action='store_true', required=True)
    parser.add_argument('--native-library', type=pathlib.Path, required=True)
    parser.add_argument('--old-fixture-dir', type=pathlib.Path, required=True)
    parser.add_argument('--new-output-dir', type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.new_output_dir.mkdir(exist_ok=False)
    global SINK
    with (args.new_output_dir / 'events.jsonl').open('x') as SINK:
        lib = C.CDLL(str(args.native_library.resolve()))
        declare(lib)
        emit({'kind': 'version', 'revision': 'supplement-square-C4-K4-streams3-coupled1', 'native_library': str(args.native_library),
              'version': lib.opus_get_version_string().decode(),
              'scope': 'native duration selection and fixed old-packet trajectories; no timers/quality/atomic transport claim'})
        banks = {(mode, coded): load_bank(lib, args.old_fixture_dir / f'{mode}-{coded}.packets', f'{mode}-{coded}')
                 for mode in MODES for coded in (1, 2)}
        candidates = {}
        for mode in MODES:
            toc = banks[(mode, 1)][1][0]
            for label, payload in LATE_PAYLOADS:
                candidate = (toc, [payload])
                # Verify canonical ordinary framing publicly; content acceptance
                # remains unknown and is never used to choose/replace a case.
                parse_packet(lib, packet_from_frames(candidate), f'late-{mode}-{label}')
                candidates[(mode, label)] = candidate
        summary = []
        for mode in MODES:
            pair = lambda index: (banks[(mode, 2)][index], banks[(mode, 1)][index], banks[(mode, 1)][index])
            for rate in RATES:
                quantum, maximum = rate // 400, rate * 120 // 1000
                for family, layout, mapping in LAYOUTS:
                    decoder = Decoder(lib, args.new_output_dir, family, layout, mapping, rate, mode)
                    if not decoder.state:
                        summary.append(decoder.context | {'coverage': 'NOT_COVERED', 'reason': 'constructor-rejected'})
                        continue
                    try:
                        decoder.observe('fresh')
                        # Only documented borrowed-child CTLs split status. The
                        # crossed pairs [0,20] and [40,0] distinguish selection
                        # without decoding audio through borrowed children.
                        decoder.step('scope-whole-20ms', transport(pair(0)), maximum)
                        decoder.reset('scope-reset-child0-only', child_index=0)
                        decoder.reset('scope-reset-whole-between-crossed-pairs')
                        decoder.step('scope-whole-plc-40ms', None, rate // 25)
                        decoder.reset('scope-reset-child1-only', child_index=1)
                        decoder.reset('scope-reset-whole-after-crossed-pairs')
                        decoder.step('coded-20ms-0', transport(pair(0)), maximum)
                        decoder.step('coded-20ms-1', transport(pair(1)), maximum)
                        decoder.step('plc-7.5ms', None, quantum * 3)
                        decoder.step('requested-fec-40ms-prefix-plus-tail', transport(pair(4)), rate // 25, fec=True)
                        decoder.step('normal-following-recovery-20ms', transport(pair(4)), maximum)
                        decoder.step('plc-40ms', None, rate // 25)
                        decoder.step('48-implicit-2.5ms-frames-120ms', transport(((0xE4, [b''] * 48), (0xE0, [b''] * 48), (0xE0, [b''] * 48))), maximum)
                        decoder.step('early-malformed-final-framing', b''.join(packet_from_frames(packet, True) for packet in pair(0)[:-1]) + b'\x03', maximum)
                        decoder.step('early-normal-capacity-one-sample', transport(pair(0)), 1)
                        decoder.step('early-invalid-plc-duration', None, quantum - 1)
                        decoder.reset('trajectory-reset')
                        decoder.step('fresh-implicit-hybrid-10ms', transport(((0x64, [b'']), (0x60, [b'']), (0x60, [b'']))), maximum)
                        decoder.step('coded-after-implicit-short-20ms', transport(pair(0)), maximum)
                        partial_count = 0
                        for label, _ in LATE_PAYLOADS:
                            # Every predetermined case starts the same way;
                            # always reset again after content errors.
                            decoder.reset('late-' + label + '-reset')
                            decoder.step('late-' + label + '-prime20', transport(pair(0)), maximum)
                            decoder.step('late-' + label + '-prime7.5', None, quantum * 3)
                            row = decoder.step('late-' + label, transport((pair(1)[0], candidates[(mode, label)], pair(1)[2])), maximum)
                            proved = bool(row and row['code'] < 0 and
                                          child_samples(row['before'], 0) == quantum * 3 and
                                          child_samples(row['before'], 1) == quantum * 3 and
                                          child_samples(row['after'], 0) == rate // 50 and
                                          child_samples(row['after'], 1) == quantum * 3)
                            partial_count += int(proved)
                            emit(decoder.context | {'kind': 'late-content-classification', 'candidate': label,
                                                    'coverage': 'OBSERVED' if proved else 'NOT_COVERED',
                                                    'definition': 'negative transport result; child0 advanced7.5->20ms while child1 retained7.5ms'})
                        summary.append(decoder.context | {'partial_error_coverage': 'OBSERVED' if partial_count else 'NOT_COVERED',
                                                          'observed_predetermined_cases': partial_count, 'tried': len(LATE_PAYLOADS)})
                    finally:
                        decoder.close()
        emit({'kind': 'summary', 'scope': 'raw observations only; no guessed aggregate policy or missing coverage relabelled', 'cases': summary})
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception as error:
        # Preserve the partial output directory/events and a nonzero exit.
        print(repr(error), file=sys.stderr)
        raise
