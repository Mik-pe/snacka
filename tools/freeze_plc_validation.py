#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Canonicalize validation inputs before native replay; never opens test paths."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import numpy as np
from evaluate_speech import noisy


def main():
    p=argparse.ArgumentParser();p.add_argument('selection',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    if a.output.exists():p.error('fresh canonical directory required')
    a.output.mkdir(parents=True);manifests={lang:dict(sources={}) for lang in ('en','sv')}
    for row in json.loads(a.selection.read_text()):
        if row['split']!='validation':raise ValueError('only validation audio is allowed')
        source=Path(row['corpus_root'])/row['path']
        if hashlib.sha256(source.read_bytes()).hexdigest()!=row['flac_sha256']:raise ValueError('source hash')
        pcm=subprocess.check_output(['ffmpeg','-v','error','-i',str(source),'-ar','48000','-ac','1','-f','f32le','pipe:1'])
        clean=np.clip(np.frombuffer(pcm,dtype='<f4'),-1,1).astype('<f4');native=clean.astype(np.float64)
        root=a.output/row['language']/('canonical-'+row['language']);root.mkdir(parents=True,exist_ok=True)
        conditions={}
        for name in ('clean','noise20','noise10'):
            data=clean.tobytes() if name=='clean' else noisy(native,source.stem,int(name[5:]))[0]
            relative=row['flac_sha256']+'/'+name+'.f32';path=root/relative;path.parent.mkdir(exist_ok=True);path.write_bytes(data)
            conditions[name]=dict(file=relative,bytes=len(data),sha256=hashlib.sha256(data).hexdigest())
        manifests[row['language']]['sources'][row['flac_sha256']]=dict(group_id=row['group_id'],path=row['path'],conditions=conditions)
    for lang,manifest in manifests.items():
        (a.output/lang/('canonical-'+lang)/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')

if __name__=='__main__':main()
