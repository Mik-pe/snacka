#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Audit complete oracle coverage and report every prespecified representation.

The admission guard is for descriptive representation headroom only. Passing
does not approve a causal model or permit choosing on this known cohort.
"""
import argparse
from collections import defaultdict
import csv
import hashlib
import json
from itertools import product
from pathlib import Path
import numpy as np
VARIANTS=tuple(json.loads((Path(__file__).resolve().parents[1]/'docs/neural-plc-phase-protocol.json').read_text())['variants'])

KEYS=('source','group_id','clip','condition','bitrate','frame_ms','harmonic','pattern')


def admission(english, swedish):
    """Per-language phase-insensitive guards precede any error ranking."""
    checks={}
    for language,metrics in [('en',english),('sv',swedish)]:
        for key in ('lost_error_ratio','full_envelope_ratio','estoi_delta'):
            value=metrics.get(key)
            checks[f'{language}_{key}_finite']=value is not None and bool(np.isfinite(value))
        checks[f'{language}_error_lower']=checks[f'{language}_lost_error_ratio_finite'] and metrics['lost_error_ratio']<1
        checks[f'{language}_envelope_noninferior']=checks[f'{language}_full_envelope_ratio_finite'] and metrics['full_envelope_ratio']<=1
        checks[f'{language}_estoi_noninferior']=checks[f'{language}_estoi_delta_finite'] and metrics['estoi_delta']>=0
    return dict(passed=all(checks.values()),checks=checks)


def compare(rows, reference):
    def ratio(field):
        denominator=sum(float(x[field]) for x in reference)
        if denominator<=0:
            raise ValueError('reference denominator must be positive')
        return sum(float(x[field]) for x in rows)/denominator
    result={name+'_ratio':ratio(field) for name,field in [('lost_error','lost_error_energy'),('lowband_mdct','lowband_mdct_error_energy'),('mrstft','mrstft_magnitude_error')]}
    for name in ('full','low','high'):
        def env(part):
            return sum(float(x[name+'_envelope_sum']) for x in part)/sum(int(x[name+'_envelope_count']) for x in part)
        result[name+'_envelope_ratio']=env(rows)/env(reference)
    for name in ('stoi','estoi'):
        result[name+'_delta']=float(np.mean([float(x[name])-float(y[name]) for x,y in zip(rows,reference)]))
    for name in ('onset_snr_db','recovery_snr_db'):
        values=[float(x[name])-float(y[name]) for x,y in zip(rows,reference) if x[name] and y[name]]
        result[name+'_delta']=float(np.mean(values)) if values else None
    return result


def audit(rows):
    indexed={}
    for row in rows:
        key=(row['implementation'],*(row[k] for k in KEYS))
        if key in indexed:
            raise ValueError('duplicate quality case')
        indexed[key]=row
        for metric in ('lost_error_energy','lowband_mdct_error_energy','mrstft_magnitude_error','stoi','estoi','full_envelope_sum','low_envelope_sum','high_envelope_sum'):
            if not np.isfinite(float(row[metric])):
                raise ValueError('nonfinite quality metric')
    base={tuple(row[k] for k in KEYS):row for row in rows if row['implementation']=='baseline'}
    if len(base)!=1728 or len(indexed)!=1728*len(VARIANTS):
        raise ValueError('requires complete1728-case by7-variant matrix')
    clips=defaultdict(list)
    for row in base.values():
        clips[(row['source'],row['group_id'],row['clip'],row['language'])].append(row)
        if len(row['input_sha256'])!=64 or any(c not in '0123456789abcdef' for c in row['input_sha256']):
            raise ValueError('invalid input digest')
    if len(clips)!=24 or any(sum(k[3]==lang for k in clips)!=12 for lang in ('en','sv')):
        raise ValueError('requires twelve source clips per language')
    expected=set(product(('clean','noise20','noise10'),('10','20'),('0','1'),
        ('periodic1','periodic3','periodic6','independent5','burst40','burst80')))
    for selected in clips.values():
        observed={(x['condition'],x['frame_ms'],x['harmonic'],x['pattern']) for x in selected}
        if observed!=expected or any(x['bitrate']!='24000' for x in selected):
            raise ValueError('factorial coverage differs')
    for variant in VARIANTS:
        current={tuple(row[k] for k in KEYS):row for row in rows if row['implementation']==variant}
        if current.keys()!=base.keys():
            raise ValueError('missing paired cases')
        for key,row in current.items():
            if row['input_sha256']!=base[key]['input_sha256'] or row['language']!=base[key]['language'] or row['native_rate']!=base[key]['native_rate']:
                raise ValueError('input or group metadata differ')
    return indexed


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('evaluation',type=Path)
    p.add_argument('output',type=Path)
    a=p.parse_args()
    run=json.loads((a.evaluation/'run.json').read_text())
    quality=a.evaluation/'quality.csv'
    if run['status']!='COMPLETE' or hashlib.sha256(quality.read_bytes()).hexdigest()!=run['quality_sha256']:
        raise ValueError('incomplete or changed run')
    with quality.open() as stream:
        rows=list(csv.DictReader(stream))
    audit(rows)
    report=dict(schema=1,purpose='Matched phase representation diagnostic on speaker-disjoint validation; trajectory is causal; no independent acceptance',run=run,comparisons={},correlations={},admission={})
    for variant in VARIANTS[1:]:
        report['comparisons'][variant]={}
        for reference in ('baseline','current','gain_env','phase_narrow'):
            comparisons={}
            for language in ('en','sv'):
                selected=sorted([x for x in rows if x['implementation']==variant and x['language']==language],key=lambda x:tuple(x[k] for k in KEYS))
                paired=sorted([x for x in rows if x['implementation']==reference and x['language']==language],key=lambda x:tuple(x[k] for k in KEYS))
                values=compare(selected,paired)
                groups=sorted({x['group_id'] for x in selected})
                group_deltas=np.array([np.mean([float(x['estoi'])-float(y['estoi']) for x,y in zip(selected,paired) if x['group_id']==g]) for g in groups])
                rng=np.random.default_rng(20261022)
                bootstrap=group_deltas[rng.integers(0,len(groups),size=(2000,len(groups)))].mean(axis=1)
                values['estoi_group_bootstrap95']=np.quantile(bootstrap,[.025,.975]).tolist()
                values['groups']=len(groups);values['cases']=len(selected)
                comparisons[language]=values
            report['comparisons'][variant][reference]=comparisons
        b=report['comparisons'][variant]['baseline']
        report['admission'][variant]=admission(b['en'],b['sv'])
    collected=defaultdict(list)
    seen_parts=0
    for part in sorted((a.evaluation/'parts').glob('*.json')):
        saved=json.loads(part.read_text())
        seen_parts+=1
        if saved['identity']!=run['identity'] or saved['status']!='COMPLETE':
            raise ValueError('part provenance differs')
        path=part.with_suffix('.npz')
        if hashlib.sha256(path.read_bytes()).hexdigest()!=saved['stats_sha256']:
            raise ValueError('part statistics digest differs')
        with np.load(path,allow_pickle=False) as data:
            records=data['records'];impl=data['implementation']
            for index,variant in enumerate(VARIANTS):
                eligible=records[(impl==index)&(records['eligible']==1)]
                collected[(saved['source']['language'],variant)].append(eligible)
    if seen_parts!=96:
        raise ValueError('requires all96 native block records')
    for (language,variant),parts in sorted(collected.items()):
        records=np.concatenate(parts)
        result=dict(eligible_frames=len(records),phase_histogram={str(float(x)):int(np.sum(records['phase']==x)) for x in np.unique(records['phase'])},
                    noise_histogram={str(float(x)):int(np.sum(records['noise']==x)) for x in np.unique(records['noise'])},bands=[])
        for b in range(16):
            valid=np.isfinite(records['before'][:,b])&np.isfinite(records['after'][:,b])
            before,after=records['before'][valid,b],records['after'][valid,b]
            result['bands'].append(dict(band=b,frames=len(before),before_q=np.quantile(before,[.25,.5,.75]).tolist() if len(before) else None,
                after_q=np.quantile(after,[.25,.5,.75]).tolist() if len(after) else None,
                paired_delta_q=np.quantile(after-before,[.25,.5,.75]).tolist() if len(before) else None))
        report['correlations'].setdefault(language,{})[variant]=result
    report['limits']=['Finite grid, not continuous global optimum','No architecture or causal learnability proof','Validation cohort, not independent acceptance','Both languages bootstrap published speaker IDs; six validation speakers each; metadata proof only','Frame-local selection with actual native overlap/recovery; no sequence or clean-source lookahead objective','Each variant retains actual recovery-history feedback; correlation changes paired within that variant, not forced identical states']
    a.output.write_text(json.dumps(report,indent=2,allow_nan=False)+'\n')
    print(json.dumps(report['admission'],indent=2))


if __name__=='__main__':
    main()
