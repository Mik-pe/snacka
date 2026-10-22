#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Complete paired validation and preregistered causal selection, before test data."""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import numpy as np
from select_plc_causal import summarize,KEYS
from summarize_plc_phase import audit,compare,admission


def load(path):
    with path.open() as f:return list(csv.DictReader(f))


def choose(results):
    admitted=[name for name,result in results.items() if result['admission']['passed']]
    def key(name):
        m=results[name]['metrics']
        return (-min(m[lang]['estoi_delta'] for lang in ('en','sv')),
            sum(m[lang]['full_envelope_ratio'] for lang in ('en','sv')),
            sum(m[lang]['mrstft_ratio'] for lang in ('en','sv')),
            17 if name=='distill_phase' else 0 if name=='trajectory' else 16,name)
    return min(admitted,key=key) if admitted else None


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('a_directory',type=Path);p.add_argument('a_selection',type=Path)
    p.add_argument('b_directory',type=Path);p.add_argument('oracle',type=Path);p.add_argument('selection',type=Path);p.add_argument('output',type=Path)
    p.add_argument('--protocol',type=Path,required=True);p.add_argument('--a-protocol',type=Path,required=True);a=p.parse_args()
    if a.output.exists():p.error('selection is immutable; fresh output required')
    settings=json.loads(a.a_protocol.read_text())['validation'];a_report=json.loads(a.a_selection.read_text())
    results={};inputs=[];frames=[]
    # Audit all preregistered runs, even unadmitted models.
    a_verified={name:summarize(a.a_directory/(name+'.csv'),a.selection,settings) for name in ('control','envelope','installed')}
    from select_plc_causal import choose as stage_a_choose
    if stage_a_choose(a_verified)!=a_report['selected'] or a_verified!=a_report['results']:raise ValueError('stageA selection/report differs from complete data')
    for name in a_verified:
        rows=load(a.a_directory/(name+'.csv'));inputs.append({tuple(r[k] for k in KEYS):r['input_sha256'] for r in rows});frames.append(rows)
    if a_report['selected'] is not None:results[a_report['selected']]=a_verified[a_report['selected']]
    for name in ('distill_gain','distill_phase'):
        results[name]=summarize(a.b_directory/(name+'.csv'),a.selection,settings)
        rows=load(a.b_directory/(name+'.csv'));inputs.append({tuple(r[k] for k in KEYS):r['input_sha256'] for r in rows});frames.append(rows)
    quality=a.oracle/'quality.csv';run=json.loads((a.oracle/'run.json').read_text());rows=load(quality)
    if run['status']!='COMPLETE' or hashlib.sha256(quality.read_bytes()).hexdigest()!=run['quality_sha256']:raise ValueError('oracle incomplete or changed')
    audit(rows)
    if run['identity']['selection_sha256']!=hashlib.sha256(a.selection.read_bytes()).hexdigest():raise ValueError('oracle cohort differs')
    selected=sorted([r for r in rows if r['implementation']=='trajectory'],key=lambda r:tuple(r[k] for k in KEYS))
    reference=sorted([r for r in rows if r['implementation']=='baseline'],key=lambda r:tuple(r[k] for k in KEYS))
    inputs.append({tuple(r[k] for k in KEYS):r['input_sha256'] for r in selected})
    if any(x!=inputs[0] for x in inputs[1:]):raise ValueError('causal comparisons used different PCM')
    metrics={}
    for language in ('en','sv'):
        values=compare([r for r in selected if r['language']==language],[r for r in reference if r['language']==language])
        for key in ('lost_error_ratio','full_envelope_ratio','estoi_delta','mrstft_ratio'):
            if not np.isfinite(values[key]):raise ValueError('nonfinite trajectory metric')
        metrics[language]=values
    results['trajectory']=dict(metrics=metrics,admission=admission(metrics['en'],metrics['sv']),run=run)
    # Paired metrics are computed on each VM. Report baseline consistency, too.
    index={tuple(r[k] for k in KEYS):r for r in frames[0]};largest=0.
    for group in frames[1:]:
        for row in group:
            before=index[tuple(row[k] for k in KEYS)]
            gap=abs(float(row['baseline_estoi'])-float(before['baseline_estoi']));largest=max(largest,gap)
    report=dict(schema=1,selected=choose(results),results=results,stage_a_selected=a_report['selected'],
        protocol_sha256=hashlib.sha256(a.protocol.read_bytes()).hexdigest(),
        selection_sha256=hashlib.sha256(a.selection.read_bytes()).hexdigest(),
        validation_input_sha_paired=True,max_cross_run_baseline_estoi_abs=largest,
        test_audio_seen=False,rule='guards first, then max minimum-language ESTOI; then envelope,MR-STFT,smaller head')
    a.output.write_text(json.dumps(report,indent=2,allow_nan=False)+'\n');print(json.dumps(report,indent=2))

if __name__=='__main__':main()
