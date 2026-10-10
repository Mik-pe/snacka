#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Check the optional reference against FFmpeg's float encoder and decoder."""
import argparse
import csv
import hashlib
import math
from pathlib import Path
import subprocess
import sys
import tempfile

from evaluate_speech import floats


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path, help='normalized 48 kHz mono f32 PCM')
    parser.add_argument('output', type=Path, help='CSV verification results')
    parser.add_argument('--ffmpeg', default='ffmpeg')
    args = parser.parse_args()
    source = args.input.resolve(strict=True)
    data = source.read_bytes()
    values = floats(data)
    if not values or not all(math.isfinite(v) and abs(v) <= 1 for v in values):
        parser.error('input must be nonempty, finite normalized PCM')
    adapter = Path(__file__).with_name('opus_roundtrip.py')
    rows = []
    with tempfile.TemporaryDirectory(prefix='snacka-reference-') as temporary:
        directory = Path(temporary)
        api, independent, packet = (directory / name for name in ('api.f32', 'ffmpeg.f32', 'audio.opus'))
        for duration in (10, 20):
            for bitrate in (12000, 24000, 32000):
                result = subprocess.run(
                    [sys.executable, str(adapter), str(source), str(api), str(bitrate), str(duration)],
                    check=True, capture_output=True, text=True,
                )
                fields = dict(field.split('=', 1) for field in result.stdout.strip().split(','))
                subprocess.run(
                    [args.ffmpeg, '-v', 'error', '-y', '-f', 'f32le', '-ar', '48000', '-ac', '1',
                     '-i', str(source), '-c:a', 'libopus', '-sample_fmt', 'flt', '-application', 'lowdelay',
                     '-frame_duration', str(duration), '-b:a', str(bitrate), '-vbr', 'off',
                     '-compression_level', '10', str(packet)], check=True,
                )
                subprocess.run(
                    [args.ffmpeg, '-v', 'error', '-y', '-c:a', 'libopus', '-request_sample_fmt', 'flt',
                     '-i', str(packet), '-f', 'f32le', str(independent)], check=True,
                )
                output = api.read_bytes()
                if output != independent.read_bytes():
                    raise ValueError('reference PCM differs from FFmpeg; verify library version and float formats')
                if len(output) != len(data):
                    raise ValueError('reference changed the input sample count')
                rows.append(dict(
                    fields, input_sha256=hashlib.sha256(data).hexdigest(), frame_ms=duration, bitrate=bitrate,
                    samples=len(values), output_sha256=hashlib.sha256(output).hexdigest(),
                    ffmpeg_float_pcm_identical=True,
                ))
    with args.output.open('w', newline='') as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    print(f'verified {len(rows)} profiles against FFmpeg')


if __name__ == '__main__':
    main()
