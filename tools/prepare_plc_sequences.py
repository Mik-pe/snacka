#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""CPU-only native sequence export with complete source hashes and OLA parity."""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import numpy as np
from extract_plc_corpus import extract as extract_statistics
from plc_ablation_loss import HZ


def parse(path):
    raw=path.read_bytes()
    if raw[:8]!=b'SPLS\x01\0\0\0':raise ValueError('sequence framing')
    offset=8
    while offset<len(raw):
        n=int.from_bytes(raw[offset:offset+2],'little');h=raw[offset+2];offset+=4
        if n not in (480,960) or h not in (1,2,4,6):raise ValueError('sequence dimensions')
        def take(shape):
            nonlocal offset
            count=int(np.prod(shape));value=np.frombuffer(raw,dtype='<f4',count=count,offset=offset).copy().reshape(shape)
            offset+=count*4
            if not np.isfinite(value).all():raise ValueError('nonfinite sequence')
            return value
        event={'overlap':take((120,)), 'x':take((h,80)), 'prediction':take((h,n)),
               'recovery':take((n,)), 'target_coefficients':take((h,n)),
               'target':take(((h+1)*n,)), 'baseline':take(((h+1)*n,))}
        yield n,h,event
    if offset!=len(raw):raise ValueError('trailing sequence bytes')


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('selection',type=Path,help='explicit train/validation manifest rows with corpus_root')
    p.add_argument('output',type=Path)
    p.add_argument('--binary',type=Path,required=True)
    p.add_argument('--stride',type=int,default=24)
    a=p.parse_args()
    if a.output.exists():p.error('fresh output required')
    a.output.mkdir(parents=True)
    subprocess.run([str(a.binary.resolve()),'basis',str(a.output.resolve())],check=True)
    basis={n:np.fromfile(a.output/f'basis-{n}.f32',dtype='<f4').reshape(n,n+120) for n in (480,960)}
    rows=json.loads(a.selection.read_text());groups=defaultdict(list);receipt=[];maximum=0
    for index,row in enumerate(rows):
        if row['split'] not in ('train','validation'):raise ValueError('test may not enter sequence training')
        source=Path(row['corpus_root'])/row['path']
        if hashlib.sha256(source.read_bytes()).hexdigest()!=row['flac_sha256']:raise ValueError('source hash')
        key=int(hashlib.sha256(('snacka-plc-v1:'+row['path']).encode()).hexdigest()[:16],16)
        bitrate=(12000,24000,32000)[key%3];duration=(10,20)[(key//3)%2];harmonic=(key//6)%2
        condition=(key//12)%3;gain_db=(0,-12,-24)[(key//36)%3]
        pcm=np.frombuffer(subprocess.check_output(['ffmpeg','-v','error','-i',str(source),'-ac','1','-ar','48000','-f','f32le','pipe:1']),dtype='<f4').copy()
        if condition:
            noise=np.random.default_rng(key).normal(size=len(pcm));power=np.mean(pcm.astype(np.float64)**2)
            noise*=np.sqrt(power/(np.mean(noise**2)*10**((20 if condition==1 else 10)/10)));pcm+=noise.astype(np.float32)
        pcm=np.clip(pcm*np.float32(10**(gain_db/20)),-1,1)
        count=0
        with tempfile.TemporaryDirectory(prefix='snacka-sequence-') as temporary:
            root=Path(temporary);(root/'input.f32').write_bytes(pcm.astype('<f4').tobytes())
            subprocess.run([str(a.binary.resolve()),str(root/'input.f32'),str(root/'sequences.bin'),str(bitrate),str(duration),str(harmonic),str(a.stride),str(key%a.stride)],check=True,capture_output=True)
            for n,h,event in parse(root/'sequences.bin'):
                coefficients=np.concatenate((event['prediction'],event['recovery'][None,:]))
                blocks=coefficients@basis[n];values=blocks[:,:n].copy()
                values[0,:120]+=event['overlap'];values[1:,:120]+=blocks[:-1,n:]
                predicted=np.clip(values,-1,1).flatten()
                gap=float(np.max(np.abs(predicted-event['baseline'])));maximum=max(maximum,gap)
                if gap>2e-5:raise ValueError(f'native inverse/OLA parity failed: {gap}')
                # Preserve enough statistics to independently audit low-band and envelope loss.
                groups[(row['split'],row['language'],n,h)].append(event);count+=1
        receipt.append({**row,'events':count,'bitrate':bitrate,'duration_ms':duration,'harmonic':harmonic,'condition':condition,'gain_db':gain_db})
        print(f'completed {index+1}/{len(rows)} events={count}',flush=True)
    files=[]
    for (split,language,n,h),events in sorted(groups.items()):
        name=f'{split}-{language}-{n}-{h}.npz';path=a.output/name
        np.savez_compressed(path,**{k:np.stack([e[k] for e in events]) for k in events[0]})
        files.append({'name':name,'events':len(events),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
    report={'schema':1,'selection_sha256':hashlib.sha256(a.selection.read_bytes()).hexdigest(),
            'binary_sha256':hashlib.sha256(a.binary.read_bytes()).hexdigest(),'stride':a.stride,
            'native_inverse_ola_identity_max_abs':maximum,'files':files,'utterances':receipt}
    (a.output/'sequence-provenance.json').write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
