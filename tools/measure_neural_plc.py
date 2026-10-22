#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Measure received/first-loss/continued-loss/recovery calls on fixed speech.

The disabled variant still observes history. Use measure_cpu.py and an external
frozen-main executable to measure total added received-frame observation cost.
Per-call wall timings include scheduler noise and are not deadline guarantees.
"""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tempfile


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('corpus',type=Path)
    p.add_argument('output',type=Path)
    p.add_argument('--binary',type=Path,required=True)
    p.add_argument('--repetitions',type=int,default=5)
    p.add_argument('--modes',type=int,nargs='+',choices=[0,1],default=[0])
    a=p.parse_args()
    rows=[];sources=[]
    with tempfile.TemporaryDirectory(prefix='snacka-plc-timing-') as temporary:
        root=Path(temporary)
        for clip in sorted(a.corpus.glob('*.flac')):
            raw=subprocess.check_output(['ffmpeg','-v','error','-i',str(clip),'-ar','48000','-ac','1','-f','f32le','pipe:1'])
            pcm=root/'input.f32';pcm.write_bytes(raw)
            sources.append(dict(name=clip.name,sha256=hashlib.sha256(clip.read_bytes()).hexdigest()))
            for duration in [10,20]:
                count=(len(raw)//4+48*duration-1)//(48*duration)
                dropped={i for i in range(8,count-1) if i%40 in [0,1,2,3,4,5] or i%40==20}
                mask=root/'mixed.txt';mask.write_text(''.join(f'{i}\n' for i in sorted(dropped)))
                for mode in a.modes:
                    for repetition in range(a.repetitions+1):
                        out=root/'output'
                        subprocess.run([str(a.binary.resolve()),str(pcm),str(out),'24000',str(duration),str(mode),str(mask)],check=True,capture_output=True)
                        if repetition==0:continue
                        with (out/'timing.csv').open() as stream:
                            for row in csv.DictReader(stream):
                                if row['pattern']!='mixed' or row['operation']=='drain':continue
                                index=int(row['frame'])
                                kind=('continued_loss' if index-1 in dropped else 'first_loss') if index in dropped else ('recovery' if index-1 in dropped else 'received')
                                rows.append(dict(clip=clip.stem,harmonic=mode,frame_ms=duration,repetition=repetition-1,implementation=row['implementation'],operation=kind,elapsed_ns=row['elapsed_ns']))
    with a.output.open('w') as stream:
        writer=csv.DictWriter(stream,fieldnames=list(rows[0]));writer.writeheader();writer.writerows(rows)
    a.output.with_suffix('.json').write_text(json.dumps(dict(machine=platform.machine(),platform=platform.platform(),binary_sha256=hashlib.sha256(a.binary.read_bytes()).hexdigest(),sources=sources,rows=len(rows),bitrate=24000,modes=a.modes,repetitions=a.repetitions,note='Wall time per call; baseline is correction-disabled with history observation. No WCET claim.'),indent=2)+'\n')
    print('Measured',len(rows),'calls')


if __name__=='__main__':main()
