#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Freeze exact archived PCM fingerprints using the original preprocessing host.

FLAC/source hashes and every archived clean/noisy input hash must match.
freeze_row also accepts a byte-stream loader so a raw corpus need not be
stored on the preprocessing host.
"""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import subprocess
import numpy as np
from evaluate_speech import noisy


def freeze_row(row, reference, read_source=None):
    source=read_source(row) if read_source else (Path(row['corpus_root'])/row['path']).read_bytes()
    assert hashlib.sha256(source).hexdigest()==row['flac_sha256']
    result=subprocess.run(['ffmpeg','-v','error','-i','pipe:0','-ar','48000','-ac','1','-f','f32le','pipe:1'],input=source,capture_output=True,check=True)
    raw=np.frombuffer(result.stdout,dtype='<f4');clipped=int((np.abs(raw)>1).sum())
    pcm=np.clip(raw,-1,1).astype('<f4').tobytes();clean=np.frombuffer(pcm,dtype='<f4').astype(np.float64)
    payload={};conditions={}
    for condition in ('clean','noise20','noise10'):
        data=pcm if condition=='clean' else noisy(clean,Path(row['path']).stem,int(condition[5:]))[0]
        expected={x['input_sha256'] for x in reference if x['source']==row['source'] and x['group_id']==str(row['group_id']) and x['clip']==Path(row['path']).stem and x['condition']==condition}
        sha=hashlib.sha256(data).hexdigest();assert expected=={sha},f'preprocessing does not reproduce {condition} fingerprint for {row["path"]}'
        name=f'files/{row["flac_sha256"]}.{condition}.f32';payload[name]=data
        conditions[condition]={'file':name,'sha256':sha,'bytes':len(data)}
    return payload,dict(source=row['source'],path=row['path'],group_id=row['group_id'],native_rate=row['native_rate'],resampling_clipped_samples=clipped,conditions=conditions)


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('selection',type=Path);p.add_argument('reference',type=Path);p.add_argument('output',type=Path);p.add_argument('--language')
    a=p.parse_args()
    if a.output.exists():p.error('use a fresh output directory')
    a.output.mkdir(parents=True);(a.output/'files').mkdir()
    with a.reference.open() as stream:reference=list(csv.DictReader(stream))
    rows=json.loads(a.selection.read_text());sources={}
    for row in rows:
        if a.language and row['language']!=a.language:continue
        payload,meta=freeze_row(row,reference)
        for name,data in payload.items():(a.output/name).write_bytes(data)
        sources[row['flac_sha256']]=meta
    manifest=dict(reference_sha256=hashlib.sha256(a.reference.read_bytes()).hexdigest(),numpy=np.__version__,
                  ffmpeg=subprocess.check_output(['ffmpeg','-version'],text=True).splitlines()[0],
                  generator_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),sources=sources)
    (a.output/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    print('Frozen',len(sources),'sources; all archived fingerprints reproduced')


if __name__=='__main__':main()
