#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Fresh matched phase oracles and causal f0 trajectory on validation only.

All seven variants are reported. Resume verifies the frozen run identity and
part digest; it never imports cached quality metrics from earlier studies.
"""
import argparse
from concurrent.futures import ProcessPoolExecutor
import csv
from functools import partial
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import time
import numpy as np
from evaluate_loss import loss_frames
from evaluate_plc_ablation import magnitude_error, snr
from evaluate_quality import intelligibility

VARIANTS = ('baseline','current','gain_env','phase_narrow','phase_wide','phase_noise','trajectory')
PATTERNS = ('periodic1','periodic3','periodic6','independent5','burst40','burst80')
RECORD = np.dtype([('frame','<u4'),('eligible','u1'),('horizon','u1'),('samples','<u2'),
    ('phase','<f4'),('noise','<f4'), *[(k,'<f8',(16,)) for k in ('before','after','pp','pt','tt','gains')]])
assert RECORD.itemsize == 784


def execute(task, binary, output, identity):
    row, duration, mode, canonical = task
    key = hashlib.sha256(json.dumps([row, duration, mode, identity],sort_keys=True).encode()).hexdigest()
    part = output/'parts'/key
    report, stats_file = part.with_suffix('.json'), part.with_suffix('.npz')
    if report.exists():
        saved = json.loads(report.read_text())
        if saved['identity'] != identity or saved['key'] != key or saved['status'] != 'COMPLETE':
            raise ValueError('stale or incomplete part')
        if hashlib.sha256(stats_file.read_bytes()).hexdigest() != saved['stats_sha256']:
            raise ValueError('statistics digest differs')
        return saved
    started = time.monotonic()
    manifest = json.loads((canonical/'manifest.json').read_text())
    meta = manifest['sources'][row['flac_sha256']]
    if meta['group_id'] != row['group_id'] or meta['path'] != row['path']:
        raise ValueError('canonical source differs')
    payload = {}
    for condition, spec in meta['conditions'].items():
        path = (canonical/spec['file']).resolve()
        if not path.is_relative_to(canonical.resolve()):
            raise ValueError('canonical path escapes root')
        data = path.read_bytes()
        if len(data) != spec['bytes'] or hashlib.sha256(data).hexdigest() != spec['sha256']:
            raise ValueError('canonical bytes differ')
        payload[condition] = data
    clean = np.frombuffer(payload['clean'],dtype='<f4').astype(np.float64)
    n = 48*duration
    frame = (np.arange(len(clean))+120)//n
    offset = (np.arange(len(clean))+120)%n
    rows, all_records, implementations, condition_ids, pattern_ids = [], [], [], [], []
    with tempfile.TemporaryDirectory(prefix='snacka-representation-') as tmp:
        root = Path(tmp)
        masks = {}
        for pattern in PATTERNS:
            lost = loss_frames(Path(row['path']).stem,duration,(len(clean)+n-1)//n,pattern)
            path = root/(pattern+'.txt')
            path.write_text(''.join(f'{i}\n' for i in sorted(lost)))
            masks[pattern] = lost,path
        for ci, (condition,data) in enumerate(payload.items()):
            path = root/'input.f32'
            path.write_bytes(data)
            native = root/condition
            command = [str(binary),str(path),str(native),'24000',str(duration),str(mode),str(row['native_rate']),
                       *[str(p) for _,p in masks.values()]]
            replay = subprocess.run(command,capture_output=True,text=True)
            if replay.returncode:
                raise RuntimeError(f'native replay failed: {row["path"]} {duration}/{mode}/{condition}: {replay.stderr[-1000:]}')
            uninterrupted = np.fromfile(native/'uninterrupted.baseline.f32',dtype='<f4')
            for variant in VARIANTS:
                if (native/f'uninterrupted.{variant}.f32').read_bytes() != uninterrupted.tobytes():
                    raise ValueError('uninterrupted PCM differs')
            for pi,(pattern,(dropped,_)) in enumerate(masks.items()):
                lost = np.isin(frame,list(dropped)); previous = np.isin(frame-1,list(dropped))
                onset = lost & ~previous & (offset<120)
                recovery = ~lost & previous & (offset<120)
                received = ~lost & ~recovery
                for vi, variant in enumerate(VARIANTS):
                    actual = np.fromfile(native/f'{pattern}.{variant}.f32',dtype='<f4')
                    if len(actual)!=len(clean) or not np.isfinite(actual).all() or (np.abs(actual)>1).any():
                        raise ValueError('decoded PCM invalid')
                    if not np.array_equal(actual.view('<u4')[received],uninterrupted.view('<u4')[received]):
                        raise ValueError('changed received PCM beyond recovery')
                    records = np.fromfile(native/f'{pattern}.{variant}.bin',dtype=RECORD)
                    if not np.array_equal(records['frame'],sorted(dropped)):
                        raise ValueError('lost frame coverage differs')
                    for name in ('pp','pt','tt','gains','phase','noise'):
                        if not np.isfinite(records[name]).all():
                            raise ValueError('nonfinite native statistics')
                    if vi:
                        if not np.array_equal(records['frame'],baseline_records['frame']):
                            raise ValueError('unpaired lost-frame keys')
                    else:
                        baseline_records = records
                    pp,pt,tt,g = (records[k] for k in ('pp','pt','tt','gains'))
                    applied_pp, applied_pt = pp*g*g, pt*g
                    errors = np.maximum(applied_pp-2*applied_pt+tt,0)
                    total = np.maximum(np.sum(applied_pp+tt,axis=1),1e-10)[:,None]
                    env = (np.log(applied_pp/total+0.001)-np.log(tt/total+0.001))**2
                    active = applied_pp+tt>1e-10
                    last = 13 if row['native_rate']==16000 else 16
                    result = dict(source=row['source'],group_id=row['group_id'],clip=Path(row['path']).stem,
                        language=row['language'],native_rate=row['native_rate'],condition=condition,bitrate=24000,
                        frame_ms=duration,harmonic=mode,pattern=pattern,implementation=variant,
                        input_sha256=hashlib.sha256(data).hexdigest(),lost_samples=int(lost.sum()),
                        lost_error_energy=float(np.sum((uninterrupted[lost].astype(np.float64)-actual[lost])**2)),
                        lowband_mdct_error_energy=float(errors[:,:7].sum()),
                        mrstft_magnitude_error=magnitude_error(uninterrupted,actual.astype(np.float64),row['native_rate']))
                    for name,sl in [('full',slice(0,last)),('low',slice(0,7)),('high',slice(7,last))]:
                        result[name+'_envelope_sum'] = float(np.sum(env[:,sl]*active[:,sl]))
                        result[name+'_envelope_count'] = int(active[:,sl].sum())
                    for name,mask in [('lost',lost),('onset',onset),('recovery',recovery)]:
                        result[name+'_snr_db'] = snr(uninterrupted,actual,mask)
                    result.update({k:float(v) for k,v in intelligibility(clean,actual.astype(np.float64),Path(row['path']).stem+condition).items()})
                    rows.append(result)
                    all_records.append(records)
                    implementations.append(np.full(len(records),vi,dtype='u1'))
                    condition_ids.append(np.full(len(records),ci,dtype='u1'))
                    pattern_ids.append(np.full(len(records),pi,dtype='u1'))
    np.savez_compressed(stats_file,records=np.concatenate(all_records),implementation=np.concatenate(implementations),
                        condition=np.concatenate(condition_ids),pattern=np.concatenate(pattern_ids))
    saved = dict(status='COMPLETE',key=key,identity=identity,seconds=time.monotonic()-started,
        source=row,duration=duration,mode=mode,rows=rows,stats_sha256=hashlib.sha256(stats_file.read_bytes()).hexdigest())
    report.write_text(json.dumps(saved)+'\n')
    return saved


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('selection',type=Path)
    p.add_argument('canonical',type=Path,help='parent containing en/canonical-en and sv/canonical-sv')
    p.add_argument('output',type=Path)
    p.add_argument('--binary',type=Path,required=True)
    p.add_argument('--protocol',type=Path,required=True)
    p.add_argument('--workers',type=int,default=2)
    a=p.parse_args()
    rows=json.loads(a.selection.read_text())
    if len(rows)!=24 or {r['language'] for r in rows}!={'en','sv'}:
        raise ValueError('requires complete frozen24-utterance selection')
    a.output.mkdir(parents=True,exist_ok=True);(a.output/'parts').mkdir(exist_ok=True)
    identity=dict(binary_sha256=hashlib.sha256(a.binary.read_bytes()).hexdigest(),protocol_sha256=hashlib.sha256(a.protocol.read_bytes()).hexdigest(),
        selection_sha256=hashlib.sha256(a.selection.read_bytes()).hexdigest(),
        manifests={lang:hashlib.sha256((a.canonical/lang/('canonical-'+lang)/'manifest.json').read_bytes()).hexdigest() for lang in ('en','sv')})
    run=a.output/'run.json'
    if run.exists() and json.loads(run.read_text())['identity']!=identity:
        raise ValueError('run identity changed; fresh output required')
    run.write_text(json.dumps(dict(status='INCOMPLETE',identity=identity),indent=2)+'\n')
    tasks=[(row,duration,mode,a.canonical/row['language']/('canonical-'+row['language'])) for row in rows for duration in (10,20) for mode in (0,1)]
    execute_task=partial(execute,binary=a.binary.resolve(),output=a.output.resolve(),identity=identity)
    count=0
    with (a.output/'quality.csv').open('w') as stream, ProcessPoolExecutor(max_workers=a.workers) as pool:
        writer=None
        for index,part in enumerate(pool.map(execute_task,tasks)):
            if writer is None:
                writer=csv.DictWriter(stream,fieldnames=list(part['rows'][0]));writer.writeheader()
            writer.writerows(part['rows']);stream.flush();count+=len(part['rows'])
            print(f'completed {index+1}/{len(tasks)}, rows={count}',flush=True)
    if count!=24*4*3*6*len(VARIANTS):
        raise ValueError('incomplete factorial coverage')
    run.write_text(json.dumps(dict(status='COMPLETE',identity=identity,rows=count,
        quality_sha256=hashlib.sha256((a.output/'quality.csv').read_bytes()).hexdigest()),indent=2)+'\n')


if __name__=='__main__':
    main()
