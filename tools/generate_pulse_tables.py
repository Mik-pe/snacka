#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Generate or check exact signed-multiset costs and fixed-point gain weights."""
import argparse
import ast
import math
from pathlib import Path
import re


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    dimensions = (20, 40, 80, 160, 320, 400, 800)
    costs = [[(math.comb(n + k - 1, k) - 1).bit_length() + k
              for k in range(388)] for n in dimensions]
    weights = [0] + [round(2 ** (c / 4 if c <= 64 else 16 + (c - 64) / 8))
                     for c in range(1, 209)]
    if args.check:
        source = (Path(__file__).resolve().parents[1] / 'src/pulse.rs').read_text()
        for name, expected in (('PULSE_COST', costs), ('ENERGY_WEIGHT', weights)):
            match = re.search(r'const ' + name + r':.*?=\s*(\[.*?\]);', source, re.S)
            if match is None or ast.literal_eval(match.group(1)) != expected:
                raise ValueError(f'{name} does not match its mathematical definition')
        print('Exact pulse costs and fixed-point gain weights match')
    else:
        print('const PULSE_COST: [[u16; 388]; 7] = [')
        for row in costs:
            print('    [' + ', '.join(map(str, row)) + '],')
        print('];')
        print('const ENERGY_WEIGHT: [u64; 209] = [' + ', '.join(map(str, weights)) + '];')


if __name__ == '__main__':
    main()
