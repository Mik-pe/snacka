#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Replay noncausal native PLC oracles on a previously scored diagnostic set.

Raw audio stays on the evaluation host. Every variant, including baseline,
gets new perceptual scoring in the same build. The archive is historical
comparison only; native hook PCM parity stays bit-exact. Parts permit resuming
the identical frozen experiment after interruption.
"""
import argparse
from concurrent.futures import ProcessPoolExecutor
import csv
from functools import lru_cache
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import time
import numpy as np
from evaluate_neural_plc import PATTERNS, noisy, spectrum, snr, intelligibility, spectral_distance

VARIANTS=('baseline','current','oracle_mdct','oracle_env','oracle_combined','oracle_wide')
KEYS=('source','group_id','clip','condition','bitrate','frame_ms','harmonic','pattern')
RECORD=np.dtype([('frame','<u4'),('samples','<u2'),('eligible','u1'),('horizon','u1'),
                 ('pp','<f8',(16,)),('pt','<f8',(16,)),('tt','<f8',(16,)),('mlp','<f8',(16,)),('oracles','<f4',(4,16))])
assert RECORD.itemsize==776


@lru_cache(maxsize=1)
def frozen(path):
    with Path(path).open() as stream:
        return {tuple(x[k] for k in KEYS):x for x in csv.DictReader(stream)}


@lru_cache(maxsize=1)
def prepared(path):
    return json.loads(Path(path).read_text()) if path else None


def execute(task):
    row,rate,duration,harmonic,binary,reference,parts,quality,prepared_path=task
    identity=dict(source=row['source'],group=row['group_id'],path=row['path'],rate=rate,duration=duration,harmonic=harmonic)
    part=Path(parts)/hashlib.sha256(json.dumps(identity,sort_keys=True).encode()).hexdigest()[:20]
    report=part.with_suffix('.json');stats=part.with_suffix('.npz')
    if report.exists():
        saved=json.loads(report.read_text());assert saved['status']=='COMPLETE'
        assert hashlib.sha256(stats.read_bytes()).hexdigest()==saved['stats_sha256']
        return saved
    started=time.monotonic();source=Path(row['corpus_root'])/row['path']
    assert hashlib.sha256(source.read_bytes()).hexdigest()==row['flac_sha256']
    manifest=prepared(prepared_path);cached=manifest['sources'].get(row['flac_sha256']) if manifest else None
    cached_data={}
    if cached:
        root=Path(prepared_path).parent.resolve()
        for condition,meta in cached['conditions'].items():
            path=(root/meta['file']).resolve();assert path.is_relative_to(root)
            data=path.read_bytes();assert len(data)==meta['bytes'] and hashlib.sha256(data).hexdigest()==meta['sha256'];cached_data[condition]=data
        pcm=cached_data['clean'];clipped=cached['resampling_clipped_samples']
    else:
        raw=subprocess.check_output(['ffmpeg','-v','error','-i',str(source),'-ar','48000','-ac','1','-f','f32le','pipe:1'])
        resampled=np.frombuffer(raw,dtype='<f4');clipped=int((np.abs(resampled)>1).sum())
        pcm=np.clip(resampled,-1,1).astype('<f4').tobytes()
    clean=np.frombuffer(pcm,dtype='<f4').astype(np.float64)
    samples=48*duration;frame=(np.arange(len(clean))+120)//samples;offset=(np.arange(len(clean))+120)%samples
    outputs=[];native=[];records=[];conditions=[];patterns=[]
    gold=frozen(reference)
    with tempfile.TemporaryDirectory(prefix='snacka-oracle-') as temp:
        root=Path(temp);input_path=root/'input.f32'
        masks={}
        from evaluate_loss import loss_frames
        for pattern in PATTERNS:
            dropped=loss_frames(source.stem,duration,(len(clean)+samples-1)//samples,pattern)
            p=root/(pattern+'.txt');p.write_text(''.join(f'{i}\n' for i in sorted(dropped)));masks[pattern]=(dropped,p)
        for condition_index,condition in enumerate(('clean','noise20','noise10')):
            data=cached_data[condition] if cached else (pcm if condition=='clean' else noisy(clean,source.stem,int(condition[5:]))[0])
            input_path.write_bytes(data);out=root/condition
            subprocess.run([binary,str(input_path),str(out),str(rate),str(duration),str(int(harmonic)),
                            *[str(p) for _,p in masks.values()],'--verify-native'],check=True,capture_output=True)
            original=np.frombuffer(data,dtype='<f4').astype(np.float64)
            spec=spectrum(original) if quality else None
            uninterrupted=np.fromfile(out/'uninterrupted.baseline.f32',dtype='<f4')
            for variant in VARIANTS:
                assert (out/f'uninterrupted.{variant}.f32').read_bytes()==uninterrupted.tobytes()
            with (out/'frame-errors.csv').open() as stream:
                native.extend(dict(x,condition=condition) for x in csv.DictReader(stream) if x['pattern']!='uninterrupted')
            for pattern_index,(pattern,(dropped,_)) in enumerate(masks.items()):
                fixed=np.fromfile(out/(pattern+'.stats.bin'),dtype=RECORD)
                records.append(fixed);conditions.append(np.full(len(fixed),condition_index,dtype=np.uint8));patterns.append(np.full(len(fixed),pattern_index,dtype=np.uint8))
                lost=np.isin(frame,list(dropped));previous=np.isin(frame-1,list(dropped))
                onset=lost&~previous&(offset<120);recovery=~lost&previous&(offset<120);received=~lost&~recovery
                base=dict(clip=source.stem,source=row['source'],group_id=row['group_id'],language=row['language'],native_rate=row['native_rate'],
                          condition=condition,bitrate=rate,frame_ms=duration,harmonic=harmonic,pattern=pattern,
                          lost_samples=int(lost.sum()),resampling_clipped_samples=clipped,input_sha256=hashlib.sha256(data).hexdigest())
                old=gold[tuple(str(base[k]) for k in KEYS)]
                assert old['input_sha256']==base['input_sha256'],'frozen input differs'
                for variant in VARIANTS:
                    values=np.fromfile(out/f'{pattern}.{variant}.f32',dtype='<f4')
                    assert len(values)==len(uninterrupted) and np.isfinite(values).all() and (np.abs(values)<=1).all()
                    assert np.array_equal(values.view('<u4')[received],uninterrupted.view('<u4')[received]),'error propagated beyond recovery overlap'
                    x=dict(base,implementation=variant)
                    x['lost_error_energy']=float(np.sum((uninterrupted[lost].astype(np.float64)-values[lost])**2))
                    for name,selected in [('lost',lost),('onset',onset),('recovery',recovery),('whole',np.ones_like(lost))]:x[name+'_snr_db']=snr(uninterrupted,values,selected)
                    if quality:
                        x.update({k:float(v) for k,v in intelligibility(clean,values.astype(np.float64),source.stem+condition).items()})
                        x['lsd_db']=float(spectral_distance(spec,values.astype(np.float64)));x['quality_origin']='new same-build scoring'
                    if variant in ('baseline','current'):
                        x['archived_lost_error_energy']=float(old[variant+'_lost_error_energy'])
                    outputs.append(x)
    fixed=np.concatenate(records)
    np.savez_compressed(stats,**{name:fixed[name] for name in fixed.dtype.names},condition=np.concatenate(conditions),pattern=np.concatenate(patterns))
    saved=dict(status='COMPLETE',identity=identity,seconds=time.monotonic()-started,rows=outputs,native=native,
               source=row,stats_file=stats.name,stats_sha256=hashlib.sha256(stats.read_bytes()).hexdigest())
    report.write_text(json.dumps(saved)+'\n');return saved


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('selection',type=Path);p.add_argument('reference',type=Path);p.add_argument('output',type=Path)
    p.add_argument('--binary',type=Path,required=True);p.add_argument('--workers',type=int,default=2)
    p.add_argument('--limit',type=int);p.add_argument('--quick',action='store_true');p.add_argument('--resume',action='store_true')
    p.add_argument('--task-shard',nargs=2,type=int,default=[0,1],metavar=('INDEX','COUNT'))
    p.add_argument('--prepared-inputs',type=Path,help='optional exact archived PCM manifest from the original resampling host')
    p.add_argument('--describe-protocol',action='store_true',help='print the immutable run protocol without starting work')
    a=p.parse_args();rows=json.loads(a.selection.read_text());a.output=a.output.resolve();a.binary=a.binary.resolve()
    if not 0<=a.task_shard[0]<a.task_shard[1]:p.error('invalid task shard')
    if a.prepared_inputs:assert prepared(str(a.prepared_inputs))['reference_sha256']==hashlib.sha256(a.reference.read_bytes()).hexdigest()
    protocol=dict(selection_sha256=hashlib.sha256(a.selection.read_bytes()).hexdigest(),reference_sha256=hashlib.sha256(a.reference.read_bytes()).hexdigest(),
                  binary_sha256=hashlib.sha256(a.binary.read_bytes()).hexdigest(),quality=not a.quick,limit=a.limit,task_shard=a.task_shard,
                  prepared_inputs_sha256=hashlib.sha256(a.prepared_inputs.read_bytes()).hexdigest() if a.prepared_inputs else None,
                  scoring_source_sha256={name:hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest() for name in ('evaluate_plc_oracles.py','evaluate_neural_plc.py','evaluate_quality.py','evaluate_speech.py','evaluate_loss.py')},
                  variants=VARIANTS,clamps={'tight':[.7875,1.2125],'wide':[.5,1.5]},combined_objective='training functional evaluated at applied gain; strength-aware objective reported separately',
                  solver_absolute_objective_gap_per_band=1e-9,baseline_quality_policy='all six variants freshly scored in the same build; archive historical only; input hashes and native hook PCM equality stay exact',
                  role='Previously scored test set is diagnostic. Oracles use future coefficients, not deployable causal models.')
    if a.describe_protocol:print(json.dumps(protocol,indent=2));return
    if a.resume:assert json.loads((a.output/'protocol.json').read_text())==json.loads(json.dumps(protocol))
    else:
        if a.output.exists():p.error('output exists; resume only the identical protocol')
        a.output.mkdir(parents=True);(a.output/'protocol.json').write_text(json.dumps(protocol,indent=2)+'\n');(a.output/'parts').mkdir()
    tasks=[(r,rate,ms,bool(mode),str(a.binary),str(a.reference.resolve()),str(a.output/'parts'),not a.quick,str(a.prepared_inputs.resolve()) if a.prepared_inputs else None)
           for r in rows for rate in (12000,24000,32000) for ms in (10,20) for mode in (0,1)]
    tasks=tasks[a.task_shard[0]::a.task_shard[1]]
    if a.limit:tasks=tasks[:a.limit]
    (a.output/'status.json').write_text(json.dumps(dict(status='INCOMPLETE',total=len(tasks))))
    with ProcessPoolExecutor(max_workers=a.workers) as pool:
        for index,result in enumerate(pool.map(execute,tasks)):
            (a.output/'status.json').write_text(json.dumps(dict(status='INCOMPLETE',completed=index+1,total=len(tasks))))
            print(f'completed {index+1}/{len(tasks)}; native PCM parity and frozen input equality pass; job {result["seconds"]:.2f}s',flush=True)
    result=[];native=[]
    for part in sorted((a.output/'parts').glob('*.json')):
        saved=json.loads(part.read_text());result.extend(saved['rows']);native.extend(dict(x,source=saved['source']['source'],group_id=saved['source']['group_id'],language=saved['source']['language'],bitrate=saved['identity']['rate'],frame_ms=saved['identity']['duration'],harmonic=saved['identity']['harmonic']) for x in saved['native'])
    for name,data in [('waveforms.csv',result),('native-objectives.csv',native)]:
        with (a.output/name).open('w') as stream:
            fields=list(dict.fromkeys(k for row in data for k in row));writer=csv.DictWriter(stream,fieldnames=fields);writer.writeheader();writer.writerows(data)
    expected=len(tasks)*3*len(PATTERNS)*len(VARIANTS);assert len(result)==expected
    (a.output/'status.json').write_text(json.dumps(dict(status='COMPLETE',tasks=len(tasks),rows=len(result),cases_per_variant=len(result)//len(VARIANTS))))
    print('COMPLETE',len(result),'rows',flush=True)


if __name__=='__main__':main()
