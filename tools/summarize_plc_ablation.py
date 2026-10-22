#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Apply the prospective Step2 gate against freshly decoded installed-main PCM."""
import argparse
import csv
import hashlib
import itertools
import json
import math
from pathlib import Path
import numpy as np

KEY=('clip','language','condition','bitrate','frame_ms','harmonic','pattern')


def read(path):
    if json.loads(path.with_suffix('.json').read_text())['status']!='COMPLETE':raise ValueError('incomplete evaluation')
    with path.open() as stream:rows=list(csv.DictReader(stream))
    result={tuple(row[k] for k in KEY):row for row in rows}
    if len(result)!=len(rows):raise ValueError('duplicate cases')
    return result


def average(rows,key):
    values=[float(r[key]) for r in rows if r.get(key) not in ('',None)]
    return float(np.mean(values)) if values else None


def validate_matrix(rows, protocol):
    evaluation=protocol['evaluation']
    clips={language:{r['clip'] for r in rows.values() if r['language']==language} for language in ('en','sv')}
    if set(r['language'] for r in rows.values())!={'en','sv'}:raise ValueError('language matrix differs')
    if any(len(values)!=evaluation['acceptance_clips_per_language'] for values in clips.values()):raise ValueError('clip matrix differs')
    for language in clips:
        groups={(r['source'],r['group_id']) for r in rows.values() if r['language']==language}
        if len(groups)!=evaluation['acceptance_clips_per_language']:raise ValueError('group matrix differs')
    actual=set()
    for r in rows.values():
        mode={'False':0,'True':1,'0':0,'1':1}.get(r['harmonic'])
        if mode is None:raise ValueError('invalid harmonic mode')
        actual.add((r['language'],r['clip'],int(r['bitrate']),int(r['frame_ms']),mode,r['condition'],r['pattern']))
    expected=set()
    for language in clips:
        expected.update((language,*case) for case in itertools.product(clips[language],evaluation['bitrates'],evaluation['durations_ms'],evaluation['modes'],evaluation['conditions'],evaluation['loss_patterns']))
    if actual!=expected or len(actual)!=len(rows):raise ValueError('registered case matrix differs')


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('installed',type=Path);p.add_argument('candidate',type=Path)
    p.add_argument('--protocol',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
    p.add_argument('--installed-bands',type=Path,help='optional separately replayed native high-band supplement')
    p.add_argument('--candidate-bands',type=Path)
    a=p.parse_args();protocol=json.loads(a.protocol.read_text());gates=protocol['acceptance']
    installed,candidate=read(a.installed),read(a.candidate)
    if installed.keys()!=candidate.keys():raise ValueError('case sets differ')
    validate_matrix(installed,protocol)
    validate_matrix(candidate,protocol)
    report={'protocol_sha256':hashlib.sha256(a.protocol.read_bytes()).hexdigest(),
            'installed_csv_sha256':hashlib.sha256(a.installed.read_bytes()).hexdigest(),
            'candidate_csv_sha256':hashlib.sha256(a.candidate.read_bytes()).hexdigest(),
            'cases':len(candidate),'failures':[],'by_language':[],'by_language_mode_duration':[]}
    deltas=[]
    for key in sorted(installed):
        before,after=installed[key],candidate[key]
        if any(before[field]!=after[field] for field in ('source','group_id')):raise ValueError('group metadata differs')
        for field in ['input_sha256','baseline_lost_error_energy','baseline_stoi','baseline_estoi',
                      'baseline_lowband_mdct_error_energy','baseline_log_envelope_squared_sum','baseline_log_envelope_count','baseline_mrstft_magnitude_error']:
            if before[field]!=after[field]:raise ValueError('independent baseline differs: '+field)
        delta={k:after[k] for k in KEY+('source','group_id')}
        for metric in ['stoi','estoi','onset_snr_db','recovery_snr_db','lost_snr_db']:
            b,c=before['current_'+metric],after['current_'+metric]
            delta[metric]=float(c)-float(b) if b not in ('',None) and c not in ('',None) else None
        deltas.append(delta)
    def fail(label,actual,minimum=None,maximum=None):
        if actual is None or not math.isfinite(actual) or (minimum is not None and actual<minimum) or (maximum is not None and actual>maximum):
            report['failures'].append({'gate':label,'actual':actual,'minimum':minimum,'maximum':maximum})
    grouped=[]
    for language in ('en','sv'):
        before=[r for r in installed.values() if r['language']==language]
        after=[r for r in candidate.values() if r['language']==language]
        rows=[r for r in deltas if r['language']==language]
        def total(items,key):return sum(float(r['current_'+key]) for r in items)
        lo=total(after,'lowband_mdct_error_energy')/total(before,'lowband_mdct_error_energy')
        def env(items):return total(items,'log_envelope_squared_sum')/total(items,'log_envelope_count')
        envelope=env(after)/env(before)
        magnitude=average(after,'current_mrstft_magnitude_error')/average(before,'current_mrstft_magnitude_error')
        entry={'language':language,'cases':len(rows),'lowband_error_ratio':lo,'log_envelope_ratio':envelope,
               'mrstft_ratio':magnitude,'stoi_delta':average(rows,'stoi'),'estoi_delta':average(rows,'estoi'),
               'whole_lost_error_reduction_vs_installed_pct':100*(1-total(after,'lost_error_energy')/total(before,'lost_error_energy')),
               'whole_lost_error_reduction_vs_classical_pct':100*(1-total(after,'lost_error_energy')/sum(float(r['baseline_lost_error_energy']) for r in after))}
        report['by_language'].append(entry)
        fail(language+' lowband',lo,maximum=gates['lowband_error_ratio_max'])
        fail(language+' envelope',envelope,maximum=gates['log_envelope_ratio_max'])
        fail(language+' MRSTFT',magnitude,maximum=gates['mrstft_magnitude_ratio_max'])
        fail(language+' STOI',entry['stoi_delta'],minimum=gates['aggregate_language_stoi_delta_min'])
        fail(language+' ESTOI',entry['estoi_delta'],minimum=gates['aggregate_language_estoi_delta_min'])
        groups={}
        for row in rows:groups.setdefault((row['source'],row['group_id']),[]).append(row['estoi'])
        grouped.append(np.array([np.mean(v) for v in groups.values()]))
        entry['bootstrap_unit']='speaker' if language=='en' else 'sentence group; speaker unknown'
        entry['groups']=len(groups)
    rng=np.random.default_rng(protocol['evaluation']['bootstrap_seed']);repeats=protocol['evaluation']['bootstrap_repeats']
    bootstrap=np.mean([g[rng.integers(0,len(g),(repeats,len(g)))].mean(axis=1) for g in grouped],axis=0)
    ci=np.quantile(bootstrap,[0.025,0.975]).tolist();report['balanced_group_bootstrap_estoi_95pct']=ci
    fail('balanced group ESTOI CI lower',ci[0],minimum=gates['pooled_group_bootstrap_estoi_ci_lower_min'])
    for language,mode,duration in sorted({(r['language'],r['harmonic'],r['frame_ms']) for r in deltas}):
        selected=[r for r in deltas if (r['language'],r['harmonic'],r['frame_ms'])==(language,mode,duration)]
        value=average(selected,'estoi');report['by_language_mode_duration'].append({'language':language,'harmonic':mode,'frame_ms':duration,'estoi_delta':value})
        fail(f'{language}/{mode}/{duration} ESTOI',value,minimum=gates['per_language_mode_duration_estoi_delta_min'])
    for metric in ('onset','recovery'):
        value=average(deltas,metric+'_snr_db');report[metric+'_snr_delta_db']=value
        fail(metric+' SNR',value,minimum=gates['onset_recovery_mean_snr_delta_min_db'])
    values=sorted(r['lost_snr_db'] for r in deltas if r['lost_snr_db'] is not None)
    worst=float(np.mean(values[:max(1,math.ceil(len(values)*.1))]));report['worst_decile_lost_snr_delta_db']=worst
    fail('worst decile lost SNR',worst,minimum=gates['worst_decile_lost_snr_delta_min_db'])
    expected=2*protocol['evaluation']['acceptance_clips_per_language']
    for key in ('bitrates','durations_ms','modes','conditions','loss_patterns'):expected*=len(protocol['evaluation'][key])
    if len(candidate)!=expected:raise ValueError(f'expected {expected} registered cases, got {len(candidate)}')
    report['quality_status']='PASS' if not report['failures'] else 'FAIL'
    report['adoption_status']='PENDING_RUNTIME_RESOURCE_GATES' if not report['failures'] else 'RETAIN_INSTALLED_MODEL'
    if bool(a.installed_bands)!=bool(a.candidate_bands):p.error('both native band supplements are required')
    band_inputs=(a.installed_bands,a.candidate_bands)
    if all(band_inputs):
        bands=[]
        for original,original_path,supplement_path in zip((installed,candidate),(a.installed,a.candidate),band_inputs):
            supplement=read(supplement_path)
            if original.keys()!=supplement.keys():raise ValueError('band supplement case set differs')
            old_meta=json.loads(original_path.with_suffix('.json').read_text())
            new_meta=json.loads(supplement_path.with_suffix('.json').read_text())
            if old_meta['binary_sha256']!=new_meta['binary_sha256']:raise ValueError('band supplement binary differs')
            for key,row in original.items():
                extra=supplement[key]
                columns=['input_sha256','source','group_id','native_rate']
                for label in ('baseline','current'):
                    columns.extend(label+'_'+metric for metric in ('lost_error_energy','lowband_mdct_error_energy','log_envelope_squared_sum','log_envelope_count'))
                if any(row[c]!=extra[c] for c in columns):raise ValueError('fresh native band replay differs from original case')
            bands.append(supplement)
        report['highband_supplement_audit']={'status':'PASS','cases_per_variant':len(installed),
            'installed_sha256':hashlib.sha256(a.installed_bands.read_bytes()).hexdigest(),
            'candidate_sha256':hashlib.sha256(a.candidate_bands.read_bytes()).hexdigest(),
            'note':'descriptive additional frequency breakdown; registered acceptance thresholds unchanged'}
    elif all('current_highband_log_envelope_squared_sum' in r for r in list(installed.values())+list(candidate.values())):
        bands=[installed,candidate]
    else:bands=[]
    if bands:
        report['highband_envelope']=[]
        for language in ('en','sv'):
            values=[]
            for variant in bands:
                selected=[r for r in variant.values() if r['language']==language]
                numerator=sum(float(r['current_highband_log_envelope_squared_sum']) for r in selected)
                count=sum(float(r['current_highband_log_envelope_count']) for r in selected)
                values.append(numerator/count)
            report['highband_envelope'].append({'language':language,'lower_hz':1600,
                'upper_hz':6800 if language=='sv' else 24000,'installed_mse':values[0],
                'candidate_mse':values[1],'candidate_over_installed_ratio':values[1]/values[0]})
    def json_safe(value):
        if isinstance(value,float) and not math.isfinite(value):return None
        if isinstance(value,dict):return {k:json_safe(v) for k,v in value.items()}
        if isinstance(value,list):return [json_safe(v) for v in value]
        return value
    rendered=json.dumps(json_safe(report),indent=2,allow_nan=False)
    a.output.write_text(rendered+'\n');print(rendered)
    raise SystemExit(bool(report['failures']))


if __name__=='__main__':main()
