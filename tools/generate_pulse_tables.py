#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Generate or check pulse bounds, harmonic bounds, class priors and gain weights."""
import argparse
import ast
import math
from pathlib import Path
import re


DIMENSIONS = (20, 40, 80, 160, 320, 400, 800)
HARMONIC_DIMENSIONS = (4, 8, 16)
MAX_PULSES = 387
CLASS_LIMIT = 80
TOTAL = 65535


def tables():
    costs, offsets, masks, boundaries = [], [], [], []
    for n in DIMENSIONS:
        row, indices, mask = [0], [0], [0, 0]
        for k in range(1, MAX_PULSES + 1):
            multiset = (math.comb(n + k - 1, k) - 1).bit_length() + min(n, k)
            if k <= CLASS_LIMIT:
                indices.append(len(boundaries))
                classes = [math.comb(n, j) * math.comb(k - 1, j - 1) * 2 ** j
                           for j in range(1, min(n, k) + 1)]
                count = sum(classes)
                units = TOTAL - len(classes)
                widths = [1 + c * units // count for c in classes]
                remainders = [c * units % count for c in classes]
                ordering = sorted(range(len(classes)),
                                  key=lambda j: (remainders[j], -j), reverse=True)
                for j in ordering[:TOTAL - sum(widths)]:
                    widths[j] += 1
                cumulative = [0]
                for width in widths:
                    cumulative.append(cumulative[-1] + width)
                assert cumulative[-1] == TOTAL
                boundaries.extend(cumulative)
                # ceil(log2(C_j*TOTAL/w_j)) plus one bit bounds interval rounding.
                bits = max(((c * TOTAL + w - 1) // w - 1).bit_length()
                           for c, w in zip(classes, widths)) + 1
                if bits < multiset:
                    mask[k // 64] |= 1 << (k % 64)
                budget = min(multiset, bits)
            else:
                budget = multiset
            row.append(max(row[-1], budget))
        costs.append(row)
        offsets.append(indices)
        masks.append(mask)
    weights = [0] + [round(2 ** (c / 4 if c <= 64 else 16 + (c - 64) / 8))
                     for c in range(1, 209)]
    small_costs = [[0] + [(math.comb(n + k - 1, k) - 1).bit_length() + min(n, k)
                         for k in range(1, MAX_PULSES + 1)]
                   for n in HARMONIC_DIMENSIONS]
    return dict(PULSE_COST=costs, ENERGY_WEIGHT=weights, OFFSETS=offsets,
                BOUNDARIES=boundaries, MASKS=masks, SMALL_COST=small_costs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    expected = tables()
    root = Path(__file__).resolve().parents[1]
    sources = {'pulse': (root / 'src/pulse.rs').read_text(),
               'classes': (root / 'src/pulse_classes.rs').read_text(),
               'harmonic': (root / 'src/harmonic.rs').read_text()}
    if args.check:
        for name, value in expected.items():
            source = sources['harmonic' if name == 'SMALL_COST' else
                             'pulse' if name in ('PULSE_COST', 'ENERGY_WEIGHT') else 'classes']
            match = re.search(r'(?:const|static) ' + name + r':.*?=\s*(\[.*?\]);', source, re.S)
            if match is None or ast.literal_eval(match.group(1)) != value:
                raise ValueError(f'{name} does not match its mathematical definition')
        print('Exact pulse and harmonic bounds, class priors and gain weights match')
    else:
        declarations = {'PULSE_COST': '[[u16; 388]; 7]', 'ENERGY_WEIGHT': '[u64; 209]',
                        'OFFSETS': '[[u32; 81]; 7]', 'BOUNDARIES': f'[u16; {len(expected["BOUNDARIES"])}]',
                        'MASKS': '[[u64; 2]; 7]', 'SMALL_COST': '[[u16; 388]; 3]'}
        for name, value in expected.items():
            qualifier = 'pub(super) static' if name == 'BOUNDARIES' else 'pub(super) const' if name in ('OFFSETS', 'MASKS') else 'const'
            print(f'{qualifier} {name}: {declarations[name]} = {value};')


if __name__ == '__main__':
    main()
