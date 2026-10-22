#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Decode-validation admission precedes candidate ranking; incomplete runs fail closed."""
import argparse
import csv
import hashlib
from itertools import product
import json
from pathlib import Path
import numpy as np
from summarize_plc_representation import admission

KEYS=('source','group_id','clip','condition','bitrate','frame_ms','harmonic','pattern')


def summarize(path, selection, settings):
    run=json.loads(path.with_suffix('.json').read_text())
    if run.get('status')!='COMPLETE' or run['selection_sha256']!=hashlib.sha256(selection.read_bytes()).hexdigest():
        raise ValueError('incomplete or mismatched evaluation selection')
    with path.open() as f: rows=list(csv.DictReader(f))
    planned=json.loads(selection.read_text())
    expected=set(product(settings['conditions'],map(str,settings['bitrates']),map(str,settings['durations']),
                         map(str,settings['modes']),settings['patterns']))
    indexed={}
    for row in rows:
        key=tuple(row[k] for k in KEYS)
        if key in indexed:raise ValueError('duplicate decoded case')
        indexed[key]=row
        for label in ('baseline','current'):
            for metric in ('lost_error_energy','log_envelope_squared_sum','log_envelope_count','mrstft_magnitude_error','estoi','stoi'):
                if not np.isfinite(float(row[label+'_'+metric])):raise ValueError('nonfinite decoded metric')
    if len(rows)!=len(planned)*len(expected) or run['rows']!=len(rows):raise ValueError('incomplete factorial')
    for source in planned:
        selected=[r for r in rows if (r['source'],r['group_id'],r['clip'])==(source['source'],source['group_id'],Path(source['path']).stem)]
        actual={(r['condition'],r['bitrate'],r['frame_ms'],r['harmonic'],r['pattern']) for r in selected}
        if actual!=expected:raise ValueError('factorial coverage differs for source')
        if any(r['language']!=source['language'] or int(r['native_rate'])!=source['native_rate'] for r in selected):raise ValueError('source metadata drift')
    metrics={}
    for language in ('en','sv'):
        selected=[r for r in rows if r['language']==language]
        def ratio(metric):
            a=sum(float(r['current_'+metric]) for r in selected);b=sum(float(r['baseline_'+metric]) for r in selected)
            if b<=0:raise ValueError('zero baseline denominator')
            return a/b
        def env(label):
            return sum(float(r[label+'_log_envelope_squared_sum']) for r in selected)/sum(int(r[label+'_log_envelope_count']) for r in selected)
        groups=sorted({r['group_id'] for r in selected})
        deltas=np.array([np.mean([float(r['delta_estoi']) for r in selected if r['group_id']==g]) for g in groups])
        rng=np.random.default_rng(20261022);samples=deltas[rng.integers(0,len(groups),size=(2000,len(groups)))].mean(1)
        metrics[language]=dict(lost_error_ratio=ratio('lost_error_energy'),full_envelope_ratio=env('current')/env('baseline'),
            lowband_ratio=ratio('lowband_mdct_error_energy'),mrstft_ratio=ratio('mrstft_magnitude_error'),
            estoi_delta=float(np.mean([float(r['delta_estoi']) for r in selected])),
            stoi_delta=float(np.mean([float(r['delta_stoi']) for r in selected])),
            estoi_speaker_bootstrap95=np.quantile(samples,[.025,.975]).tolist(),speakers=len(groups),cases=len(selected))
    return dict(metrics=metrics,admission=admission(metrics['en'],metrics['sv']),csv_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),run=run)


def choose(results):
    admitted=[name for name,value in results.items() if value['admission']['passed'] and name in ('control','envelope')]
    if not admitted:return None
    return min(admitted,key=lambda name:(sum(results[name]['metrics'][lang]['full_envelope_ratio'] for lang in ('en','sv')),
        sum(results[name]['metrics'][lang]['mrstft_ratio'] for lang in ('en','sv')),0 if name=='envelope' else 1))


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('directory',type=Path);p.add_argument('selection',type=Path)
    p.add_argument('output',type=Path);p.add_argument('--protocol',type=Path,required=True);a=p.parse_args()
    settings=json.loads(a.protocol.read_text())['validation'];results={}
    for name in ('control','envelope','installed'):
        results[name]=summarize(a.directory/(name+'.csv'),a.selection,settings)
    # Same source PCM must be compared in every model run.
    maps=[]
    for name in results:
        with (a.directory/(name+'.csv')).open() as f:maps.append({tuple(r[k] for k in KEYS):r['input_sha256'] for r in csv.DictReader(f)})
    if any(x!=maps[0] for x in maps[1:]):raise ValueError('model comparisons used different PCM')
    report=dict(schema=1,selection='native guards before ranking',selected=choose(results),results=results,
        protocol_sha256=hashlib.sha256(a.protocol.read_bytes()).hexdigest())
    a.output.write_text(json.dumps(report,indent=2,allow_nan=False)+'\n');print(json.dumps(report,indent=2))

if __name__=='__main__':main()
