#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Verify identical causal features across native sequence and phase-teacher exports."""
import argparse
from collections import Counter,defaultdict
import hashlib
import json
from pathlib import Path
import numpy as np


def main():
    p=argparse.ArgumentParser();p.add_argument('directory',type=Path);p.add_argument('audit',type=Path)
    p.add_argument('--verify-teacher',action='store_true');a=p.parse_args();counts=defaultdict(Counter)
    for path in sorted(a.directory.glob('*.npz')):
        pieces=path.stem.split('-')
        if pieces[0] not in ('train','validation'):raise ValueError('test/unknown feature file')
        with np.load(path,allow_pickle=False) as d:
            if a.verify_teacher:
                split,language=pieces;values=d['x']
            else:
                split,language,_,_=pieces;x=d['x'];prediction=d['prediction']
                eligible=(x[:,:,78]>=1)&(x[:,:,76]<.985)&(x[:,:,73]<x[:,:,77])&(np.abs(prediction).sum(-1)>0)
                values=x[eligible];language='0' if language=='en' else '1'
            for row in values:counts[split+'-'+language][hashlib.sha256(row.astype('<f4').tobytes()).hexdigest()]+=1
    result={k:dict(sorted(v.items())) for k,v in sorted(counts.items())}
    if a.verify_teacher:
        if result!=json.loads(a.audit.read_text()):raise ValueError('teacher causal features differ from matched classical sequences')
        print(json.dumps(dict(status='PASS',records=sum(sum(v.values()) for v in counts.values()),cross_export_features_bit_exact=True)))
    else:
        if a.audit.exists():p.error('fresh feature audit required')
        a.audit.write_text(json.dumps(result,sort_keys=True)+'\n')

if __name__=='__main__':main()
