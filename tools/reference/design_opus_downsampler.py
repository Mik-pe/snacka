# SPDX-License-Identifier: MIT
"""Generate Snacka's original 48 kHz -> SILK causal antialias FIR model.

Only Python's standard math library is used. Windowed sinc coefficients are
projected to exact unit DC gain and fractional first moment. The target delay
aligns downsampling + SILK's one-sample unmix delay + RFC 6716 Table 54's
decoder resampler with the 120-sample CELT overlap at the low-frequency limit.
This creates mathematical model data, not observations or external codec code.
Run --write to replace the marked constants in src/opus/silk/downsample.rs.
"""

import argparse
import cmath
import math
from pathlib import Path
import struct


TAPS = 177
CASES = ((6, 88.176), (4, 82.784), (3, 83.112))


def i0(value):
    result = term = 1.0
    for index in range(1, 64):
        term *= (value * value / 4) / (index * index)
        result += term
        if term < result * 1e-17:
            break
    return result


def model(ratio, delay):
    cutoff = 0.472 / ratio
    beta = 5.8
    weights, raw = [], []
    for index in range(TAPS):
        offset = index - delay
        relative = offset / delay
        window = i0(beta * math.sqrt(max(0, 1 - relative * relative))) / i0(beta) if abs(relative) <= 1 else 0
        sinc = 2 * cutoff if offset == 0 else math.sin(2 * math.pi * cutoff * offset) / (math.pi * offset)
        weights.append(window)
        raw.append(window * sinc)
    mass = sum(weights)
    moment = sum(weight * (index - delay) for index, weight in enumerate(weights))
    square = sum(weight * (index - delay) ** 2 for index, weight in enumerate(weights))
    wanted_mass = 1 - sum(raw)
    wanted_moment = -sum(value * (index - delay) for index, value in enumerate(raw))
    determinant = mass * square - moment * moment
    constant = (wanted_mass * square - wanted_moment * moment) / determinant
    linear = (wanted_moment * mass - wanted_mass * moment) / determinant
    values = [value + weight * (constant + linear * (index - delay)) for index, (value, weight) in enumerate(zip(raw, weights))]
    return [struct.unpack('<f', struct.pack('<f', value))[0] for value in values]


def response(values, frequency):
    return abs(sum(value * cmath.exp(-2j * math.pi * frequency * index) for index, value in enumerate(values)))


def literal(value):
    """Shortest decimal that round-trips to the generated binary32 value."""
    bits = struct.pack('<f', value)
    for precision in range(1, 10):
        text = format(value, f'.{precision}g')
        if struct.pack('<f', float(text)) == bits:
            return text if '.' in text or 'e' in text else text + '.0'
    raise AssertionError('nine significant digits must preserve binary32')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--write', action='store_true')
    args = parser.parse_args()
    models = [model(*case) for case in CASES]
    if args.write:
        path = Path(__file__).resolve().parents[2] / 'src/opus/silk/downsample.rs'
        text = path.read_text()
        begin = text.index('// BEGIN GENERATED MODEL')
        end = text.index('// END GENERATED MODEL') + len('// END GENERATED MODEL')
        data = '// BEGIN GENERATED MODEL\nconst COEFFICIENTS: [[f32; TAPS]; 3] = [\n'
        for values in models:
            data += '    [\n'
            for index in range(0, len(values), 4):
                data += '        ' + ', '.join(literal(value) for value in values[index:index+4]) + ',\n'
            data += '    ],\n'
        data += '];\n// END GENERATED MODEL'
        path.write_text(text[:begin] + data + text[end:])
    for (ratio, delay), values in zip(CASES, models):
        print(f'ratio={ratio} dc={sum(values):.10f} delay={sum(i*v for i,v in enumerate(values))/sum(values):.9f} target={delay} l1={sum(abs(v) for v in values):.5f} pass={response(values,.4/ratio):.6f} stop={response(values,.6/ratio):.7f}')


if __name__ == '__main__':
    main()
