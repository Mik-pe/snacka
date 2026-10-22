#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build causal-feature/noncausal-label shards, keeping teacher audio out of history."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import numpy as np


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('selection',type=Path);p.add_argument('output',type=Path)
    p.add_argument('--binary',type=Path,required=True);a=p.parse_args()
    if a.output.exists():p.error('fresh output required')
    a.output.mkdir(parents=True);groups={};receipts=[]
    for index,row in enumerate(json.loads(a.selection.read_text())):
        if row['split'] not in ('train','validation'):raise ValueError('test may not enter teacher extraction')
        source=Path(row['corpus_root'])/row['path']
        if hashlib.sha256(source.read_bytes()).hexdigest()!=row['flac_sha256']:raise ValueError('source hash')
        key=int(hashlib.sha256(('snacka-plc-v1:'+row['path']).encode()).hexdigest()[:16],16)
        rate=(12000,24000,32000)[key%3];duration=(10,20)[(key//3)%2];harmonic=(key//6)%2
        condition=(key//12)%3;gain_db=(0,-12,-24)[(key//36)%3]
        pcm=np.frombuffer(subprocess.check_output(['ffmpeg','-v','error','-i',str(source),'-ac','1','-ar','48000','-f','f32le','pipe:1']),dtype='<f4').copy()
        if condition:
            noise=np.random.default_rng(key).normal(size=len(pcm));power=np.mean(pcm.astype(np.float64)**2)
            noise*=np.sqrt(power/(np.mean(noise**2)*10**((20 if condition==1 else 10)/10)));pcm+=noise.astype(np.float32)
        pcm=np.clip(pcm*np.float32(10**(gain_db/20)),-1,1)
        with tempfile.TemporaryDirectory(prefix='snacka-phase-teacher-') as temporary:
            root=Path(temporary);(root/'input.f32').write_bytes(pcm.astype('<f4').tobytes())
            subprocess.run([str(a.binary.resolve()),str(root/'input.f32'),str(root/'labels.bin'),str(root/'audit.bin'),str(rate),str(duration),str(harmonic),'24',str(key%24)],check=True,capture_output=True)
            raw=(root/'labels.bin').read_bytes()
            if raw[:8]!=b'SPLT\x01\0\0\0' or (len(raw)-8)%392:raise ValueError('teacher framing')
            values=np.frombuffer(raw[8:],dtype='<f4').reshape(-1,98).copy()
            if not np.isfinite(values).all() or not np.isin(values[:,97],[0,1]).all():raise ValueError('nonfinite teacher')
            selected=values[values[:,97]==1]
            if (selected[:,80:96]<.78749).any() or (selected[:,80:96]>1.21251).any() or (np.abs(selected[:,96])>8).any():raise ValueError('teacher bound')
            language=0 if row['language']=='en' else 1
            groups.setdefault((row['split'],language),[]).append(selected)
            receipts.append({**row,'records':len(values),'eligible':len(selected),'input_sha256':hashlib.sha256(pcm.astype('<f4').tobytes()).hexdigest(),
                'teacher_sha256':hashlib.sha256(raw).hexdigest(),'classical_audit_sha256':hashlib.sha256((root/'audit.bin').read_bytes()).hexdigest(),
                'bitrate':rate,'duration_ms':duration,'harmonic':harmonic,'condition':condition,'gain_db':gain_db})
        print(f'completed {index+1} eligible={len(selected)}',flush=True)
    files=[]
    for (split,language),parts in sorted(groups.items()):
        values=np.concatenate(parts)
        if not len(values):raise ValueError('no eligible teacher rows')
        name=f'{split}-{language}.npz';path=a.output/name
        np.savez_compressed(path,x=values[:,:80],gain=values[:,80:96],phase=values[:,96])
        files.append(dict(name=name,rows=len(values),sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
    report=dict(schema=1,selection_sha256=hashlib.sha256(a.selection.read_bytes()).hexdigest(),binary_sha256=hashlib.sha256(a.binary.read_bytes()).hexdigest(),files=files,utterances=receipts,
        teacher='PhaseWide labels only; decoder synthesizes classical candidate0; no target-derived causal history')
    (a.output/'teacher-provenance.json').write_text(json.dumps(report,indent=2)+'\n')

if __name__=='__main__':main()
