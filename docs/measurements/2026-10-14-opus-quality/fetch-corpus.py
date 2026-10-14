#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Fetch the pinned CMU AN4 selection for this recorded offline evaluation."""
import argparse
import hashlib
import json
from pathlib import Path
from urllib.request import urlopen

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('output', type=Path, help='new corpus directory outside the repository')
args = parser.parse_args()
recorded = Path(__file__).resolve().parent
manifest = json.loads((recorded / 'an4-manifest.json').read_text())
args.output.mkdir(parents=True, exist_ok=True)
if any(args.output.iterdir()):
    parser.error('output directory must be empty')
for clip in manifest['clips']:
    name = clip['clip']
    if Path(name).name != name:
        raise ValueError('invalid corpus filename')
    with urlopen(clip['url']) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != clip['sha256']:
        raise ValueError(f'original audio hash mismatch: {name}')
    (args.output / name).write_bytes(data)
(args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
(args.output / 'license.txt').write_bytes((recorded / 'LICENSE-AN4.txt').read_bytes())
print(f'Fetched and verified {len(manifest["clips"])} original recordings; see license.txt.')
