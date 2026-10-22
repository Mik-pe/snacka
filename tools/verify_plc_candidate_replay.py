#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Require all native validation PCM to match the selected experimental head.

This reruns native synthesis, without reusing metric caches or scoring test
audio. Equality covers uninterrupted, lost, overlap, and recovery samples.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from evaluate_loss import loss_frames

PATTERNS=('periodic1','periodic3','periodic6','independent5','burst40','burst80')


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('selection',type=Path);p.add_argument('canonical',type=Path)
    p.add_argument('student',type=Path);p.add_argument('production',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    if a.output.exists():p.error('fresh replay proof required')
    calls=0;streams=0;digest=hashlib.sha256()
    with tempfile.TemporaryDirectory() as tmp:
        root=Path(tmp)
        for row in json.loads(a.selection.read_text()):
            canonical=a.canonical/row['language']/('canonical-'+row['language'])
            manifest=json.loads((canonical/'manifest.json').read_text())['sources'][row['flac_sha256']]
            for condition,spec in manifest['conditions'].items():
                data=(canonical/spec['file']).read_bytes()
                if hashlib.sha256(data).hexdigest()!=spec['sha256']:raise ValueError('canonical validation PCM differs')
                input_file=root/'input.f32';input_file.write_bytes(data)
                for duration in (10,20):
                    masks=[]
                    for pattern in PATTERNS:
                        path=root/(pattern+'.txt');dropped=loss_frames(Path(row['path']).stem,duration,(len(data)//4+48*duration-1)//(48*duration),pattern)
                        path.write_text(''.join(f'{i}\n' for i in sorted(dropped)));masks.append(path)
                    for mode in (0,1):
                        for binary,label in [(a.student,'student'),(a.production,'production')]:
                            subprocess.run([str(binary.resolve()),str(input_file),str(root/label),'24000',str(duration),str(mode),*map(str,masks)],check=True,capture_output=True)
                        for path in sorted((root/'student').glob('*.f32')):
                            data=path.read_bytes()
                            if data!=(root/'production'/path.name).read_bytes():raise ValueError('production changed validated PCM: '+path.name)
                            digest.update(data);streams+=1
                        calls+=1
    report=dict(status='PASS',native_replay_calls=calls,paired_pcm_streams=streams,paired_pcm_sha256=digest.hexdigest(),
        selection_sha256=hashlib.sha256(a.selection.read_bytes()).hexdigest(),
        student_binary_sha256=hashlib.sha256(a.student.read_bytes()).hexdigest(),production_binary_sha256=hashlib.sha256(a.production.read_bytes()).hexdigest())
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))


if __name__=='__main__':main()
