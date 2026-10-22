#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Freeze English speaker/utterance IDs from metadata only, before model selection."""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
from prepare_plc_causal_corpus import rank,SALT

def main():
    p=argparse.ArgumentParser();p.add_argument('metadata',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    if a.output.exists():p.error('holdout is immutable; fresh output required')
    data=json.loads(a.metadata.read_text());groups=defaultdict(list)
    if len(data)!=2620:raise ValueError('complete test-clean metadata required')
    for item in data:
        row=item['row']
        if 25<=len(row['text'].split())<=50:groups[str(row['speaker_id'])].append(row)
    if len(groups)<12:raise ValueError('insufficient eligible speakers')
    selected=[]
    for speaker in sorted(groups,key=rank)[:12]:
        row=min(groups[speaker],key=lambda r:rank(r['id']))
        selected.append(dict(source='librispeech-test-clean',speaker_id=speaker,id=row['id'],split='test',language='en',native_rate=16000))
    report=dict(schema=1,dataset='openslr/librispeech_asr',config='clean',split='test',
        source='https://huggingface.co/api/datasets/openslr/librispeech_asr/parquet/clean/test/0.parquet',
        metadata_sha256=hashlib.sha256(a.metadata.read_bytes()).hexdigest(),salt=SALT,selection=selected,
        stage='metadata only; no test PCM or metrics; freeze before training',license='CC-BY-4.0')
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(dict(speakers=len(selected),metadata_sha256=report['metadata_sha256'])))

if __name__=='__main__':main()
