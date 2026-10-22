#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Check every teacher's classical lost/recovery PCM against matched SPLS2 export."""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import struct
import numpy as np


def main():
    p=argparse.ArgumentParser();p.add_argument('input',type=Path);p.add_argument('audit',type=Path);p.add_argument('--verify-teacher',action='store_true');a=p.parse_args()
    if a.verify_teacher:
        expected=json.loads(a.audit.read_text());provenance=json.loads((a.input/'teacher-provenance.json').read_text())
        actual={row['flac_sha256']:row['classical_audit_sha256'] for row in provenance['utterances']}
        if actual!=expected:raise ValueError('teacher changed classical lost/recovery PCM or source pairing')
        print(json.dumps(dict(status='PASS',utterances=len(actual),classical_lost_and_recovery_pcm_bit_exact=True)));return
    if a.audit.exists():p.error('fresh audio audit required')
    data={}
    for path in a.input.glob('*.npz'):
        with np.load(path,allow_pickle=False) as f:data[path.stem]=f['baseline']
    counters=defaultdict(int);result={};provenance=json.loads((a.input/'sequence-provenance.json').read_text())
    for row in provenance['utterances']:
        digest=hashlib.sha256(b'SPLA\x01\0\0\0');n=row['duration_ms']*48
        for event in range(row['events']):
            h=[1,2,4,6][event%4];key=f"{row['split']}-{row['language']}-{n}-{h}";at=counters[key];counters[key]+=1
            digest.update(struct.pack('<HBB',n,h,0));digest.update(data[key][at].astype('<f4').tobytes())
        result[row['flac_sha256']]=digest.hexdigest()
    if any(counters[k]!=len(v) for k,v in data.items()):raise ValueError('unpaired sequence events')
    a.audit.write_text(json.dumps(result,sort_keys=True)+'\n')

if __name__=='__main__':main()
