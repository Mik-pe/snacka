#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Apply the prospective Step2 gate against freshly decoded installed-main PCM."""
import argparse
import csv
import hashlib
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


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('installed',type=Path);p.add_argument('candidate',type=Path)
    p.add_argument('--protocol',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
    a=p.parse_args();protocol=json.loads(a.protocol.read_text());gates=protocol['acceptance']
    installed,candidate=read(a.installed),read(a.candidate)
    if installed.keys()!=candidate.keys():raise ValueError('case sets differ')
    report={'protocol_sha256':hashlib.sha256(a.protocol.read_bytes()).hexdigest(),
            'installed_csv_sha256':hashlib.sha256(a.installed.read_bytes()).hexdigest(),
            'candidate_csv_sha256':hashlib.sha256(a.candidate.read_bytes()).hexdigest(),
            'cases':len(candidate),'failures':[],'by_language':[],'by_language_mode_duration':[]}
    deltas=[]
    for key in sorted(installed):
        before,after=installed[key],candidate[key]
        for field in ['input_sha256','baseline_lost_error_energy','baseline_stoi','baseline_estoi',
                      'baseline_lowband_mdct_error_energy','baseline_log_envelope_squared_sum','baseline_log_envelope_count','baseline_mrstft_magnitude_error']:
            if before[field]!=after[field]:raise ValueError('independent baseline differs: '+field)
        delta={k:after[k] for k in KEY+('source','group_id')}
        for metric in ['stoi','estoi','onset_snr_db','recovery_snr_db','lost_snr_db']:
            b,c=before['current_'+metric],after['current_'+metric]
            delta[metric]=float(c)-float(b) if b not in ('',None) and c not in ('',None) else None
        deltas.append(delta)
    def fail(label,actual,minimum=None,maximum=None):
        if actual is None or (minimum is not None and actual<minimum) or (maximum is not None and actual>maximum):
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
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
    raise SystemExit(bool(report['failures']))


if __name__=='__main__':main()
