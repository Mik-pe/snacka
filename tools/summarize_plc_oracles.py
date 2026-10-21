#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Separate native waveform outcomes, fixed-prediction ceilings and rho diagnostics."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import numpy as np

VARIANTS=('baseline','current','oracle_mdct','oracle_env','oracle_combined','oracle_wide')
HZ=[0,200,400,600,800,1000,1300,1600,2000,2500,3200,4000,5200,6800,9000,13000,24000]


def avg(rows,key):
    x=[float(r[key]) for r in rows if r.get(key) not in ('',None)]
    return float(np.mean(x)) if x else None


def quantiles(values):
    return np.quantile(values,[.25,.5,.75]).tolist() if len(values) else None


def functional(pp,pt,tt,gain):
    total=np.maximum((pp+tt).sum(axis=1,keepdims=True),1e-10)
    a,b,c=pp/total,pt/total,tt/total
    error=np.maximum((a*gain**2-2*b*gain+c).sum(axis=1),1e-10)
    regularization=.01*((gain-1)**2).mean(axis=1)
    envelope=.1*((np.log(a*gain**2+.001)-np.log(c+.001))**2).mean(axis=1)
    return error+regularization+envelope


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('results',type=Path);p.add_argument('output',type=Path)
    a=p.parse_args();status=json.loads((a.results/'status.json').read_text());assert status['status']=='COMPLETE'
    with (a.results/'waveforms.csv').open() as stream:rows=list(csv.DictReader(stream))
    assert len(rows)==status['rows']
    languages=sorted({r['language'] for r in rows})
    report=dict(status='COMPLETE',cases_per_variant=status['cases_per_variant'],
                protocol=json.loads((a.results/'protocol.json').read_text()),
                waveform_csv_sha256=hashlib.sha256((a.results/'waveforms.csv').read_bytes()).hexdigest(),
                interpretation=['Oracles see future coefficients and do not measure what any causal MLP can learn.',
                                'MDCT oracles are per-frame optima for fixed predictions, not a global waveform optimum through overlap-add or decoder-state feedback.',
                                'Combined applied-gain functional has the training coefficients but differs from the original post-training strength path. Strength-aware raw-gain comparison is separate.',
                                'Functional diagnostics use f64 statistics. Current raw gain is inferred from deployed f32 gain, so this is not a bitwise recovery of the PyTorch checkpoint score.',
                                'Only previously scored diagnostic material was used; no new acceptance set or training.'],
                waveforms=[],waveform_conditions=[],waveform_strata=[],native_objectives=[],fixed_prediction=[],fixed_prediction_by_horizon=[],correlations=[])
    key=lambda r:tuple(r[k] for k in ('source','group_id','clip','condition','bitrate','frame_ms','harmonic','pattern'))
    baseline={key(r):r for r in rows if r['implementation']=='baseline'}
    for language in languages:
        for variant in VARIANTS:
            selected=[r for r in rows if r['language']==language and r['implementation']==variant]
            base=sum(float(baseline[key(r)]['lost_error_energy']) for r in selected);current=sum(float(r['lost_error_energy']) for r in selected)
            groups={}
            for r in selected:
                group=(r['source'],r['group_id']);v=groups.setdefault(group,[0.,0.]);v[0]+=float(baseline[key(r)]['lost_error_energy']);v[1]+=float(r['lost_error_energy'])
            matrix=np.asarray(list(groups.values()));rng=np.random.default_rng(6716);sample=matrix[rng.integers(0,len(matrix),(2000,len(matrix)))].sum(axis=1)
            entry=dict(language=language,implementation=variant,cases=len(selected),groups=len(matrix),
                       lost_snr_gain_db=10*math.log10(base/current),error_energy_reduction_fraction=1-current/base,
                       group_bootstrap_95pct_db=np.quantile(10*np.log10(sample[:,0]/sample[:,1]),[.025,.975]).tolist())
            deltas=sorted(float(r['lost_snr_db'])-float(baseline[key(r)]['lost_snr_db']) for r in selected if r.get('lost_snr_db') not in ('',None) and baseline[key(r)].get('lost_snr_db') not in ('',None))
            entry['worst_decile_mean_lost_snr_delta_db']=float(np.mean(deltas[:max(1,math.ceil(len(deltas)*.1))])) if deltas else None
            entry['cases_with_greater_error_fraction']=float(np.mean([float(r['lost_error_energy'])>float(baseline[key(r)]['lost_error_energy']) for r in selected]))
            for metric in ('stoi','estoi','lsd_db','onset_snr_db','recovery_snr_db'):
                deltas=[float(r[metric])-float(baseline[key(r)][metric]) for r in selected if r.get(metric) not in ('',None) and baseline[key(r)].get(metric) not in ('',None)]
                entry['mean_delta_'+metric]=float(np.mean(deltas)) if deltas else None
            if variant in ('baseline','current'):
                archived=sum(float(r['archived_lost_error_energy']) for r in selected)
                entry['aggregate_relative_error_drift_vs_archive']=current/archived-1
            report['waveforms'].append(entry)
            for condition in ('clean','noise20','noise10'):
                subset=[r for r in selected if r['condition']==condition]
                before=sum(float(baseline[key(r)]['lost_error_energy']) for r in subset);after=sum(float(r['lost_error_energy']) for r in subset)
                detail=dict(language=language,implementation=variant,condition=condition,cases=len(subset),error_energy_reduction_fraction=1-after/before,lost_snr_gain_db=10*math.log10(before/after))
                for metric in ('stoi','estoi','lsd_db'):detail['mean_delta_'+metric]=float(np.mean([float(r[metric])-float(baseline[key(r)][metric]) for r in subset]))
                report['waveform_conditions'].append(detail)
    for language,mode,ms in sorted({(r['language'],r['harmonic'],r['frame_ms']) for r in rows}):
        for variant in VARIANTS:
            selected=[r for r in rows if (r['language'],r['harmonic'],r['frame_ms'],r['implementation'])==(language,mode,ms,variant)]
            entry=dict(language=language,harmonic=mode,frame_ms=int(ms),implementation=variant)
            for metric in ('stoi','estoi','lsd_db'):
                entry['mean_delta_'+metric]=float(np.mean([float(r[metric])-float(baseline[key(r)][metric]) for r in selected]))
            report['waveform_strata'].append(entry)
    with (a.results/'native-objectives.csv').open() as stream:native=list(csv.DictReader(stream))
    for language in languages:
        base=sum(float(r['mdct_error']) for r in native if r['language']==language and r['implementation']=='baseline')
        for variant in VARIANTS:
            chosen=[r for r in native if r['language']==language and r['implementation']==variant]
            eligible=sum(int(r['eligible_frames']) for r in chosen)
            entry=dict(language=language,implementation=variant,lost_frames=sum(int(r['lost_frames']) for r in chosen),eligible_frames=eligible,
                       raw_mdct_error_energy_reduction_fraction=1-sum(float(r['mdct_error']) for r in chosen)/base)
            for metric in ('normalized_mdct_error_eligible','combined_applied_loss_eligible','training_scale_loss_eligible'):
                entry['mean_'+metric]=sum(float(r[metric]) for r in chosen)/eligible if eligible else None
            report['native_objectives'].append(entry)
    by_language={}
    for part in sorted((a.results/'parts').glob('*.json')):
        meta=json.loads(part.read_text());path=part.with_name(meta['stats_file'])
        assert hashlib.sha256(path.read_bytes()).hexdigest()==meta['stats_sha256']
        with np.load(path,allow_pickle=False) as z:
            group=by_language.setdefault(meta['source']['language'],{})
            for name in z.files:group.setdefault(name,[]).append(z[name])
    for language,chunks in by_language.items():
        data={k:np.concatenate(v) for k,v in chunks.items()};pp,pt,tt=data['pp'],data['pt'],data['tt'];eligible=data['eligible'].astype(bool)
        gains=dict(baseline=np.ones_like(pp),current=data['mlp'],**{name:data['oracles'][:,i,:].astype(np.float64) for i,name in enumerate(VARIANTS[2:])})
        # The wide functional optimum on q=.5..1.5 is also the exact original
        # training-gain optimum. Its deployed gain is 1+.425*(q-1), not q.
        gains['oracle_training_strength']=(np.float32(1)+np.float32(.425)*(gains['oracle_wide'].astype(np.float32)-np.float32(1))).astype(np.float64)
        domains={'all_loss':np.ones(len(pp),dtype=bool),'runtime_eligible':eligible,
                 'extractor_domain':eligible&(data['horizon']<=6)&(pp.sum(axis=1)>=1e-10)&(tt.sum(axis=1)>=1e-10)}
        base=np.maximum((pp-2*pt+tt).sum(axis=1),0.)
        raw_current=1+(data['mlp']-1)/.425
        for scope,mask in domains.items():
            for variant,g in gains.items():
                errors=np.maximum((pp*g*g-2*pt*g+tt).sum(axis=1),0.)
                entry=dict(language=language,scope=scope,implementation=variant,frames=int(mask.sum()),
                           raw_mdct_error_energy_reduction_fraction=1-float(errors[mask].sum()/base[mask].sum()),
                           mean_combined_applied_functional=float(functional(pp,pt,tt,g)[mask].mean()))
                if variant in ('current','oracle_training_strength'):
                    q=raw_current if variant=='current' else gains['oracle_wide']
                    entry['mean_original_training_functional']=float(functional(pp,pt,tt,q)[mask].mean())
                report['fixed_prediction'].append(entry)
        for horizon in sorted(set(data['horizon'].tolist())):
            mask=eligible&(data['horizon']==horizon)
            if not mask.any() or base[mask].sum()==0:continue
            for variant,g in gains.items():
                errors=np.maximum((pp*g*g-2*pt*g+tt).sum(axis=1),0.)
                report['fixed_prediction_by_horizon'].append(dict(language=language,horizon=int(horizon),implementation=variant,eligible_frames=int(mask.sum()),raw_mdct_error_energy_reduction_fraction=1-float(errors[mask].sum()/base[mask].sum())))
        # Numerical cross-checks of the mathematical dominance relationships.
        mdct=np.maximum((pp*gains['oracle_mdct']**2-2*pt*gains['oracle_mdct']+tt).sum(axis=1),0.)
        assert np.all(mdct<=base+1e-7*np.maximum(pp.sum(axis=1)+tt.sum(axis=1),1e-20))
        tight=functional(pp,pt,tt,gains['oracle_combined']);wide=functional(pp,pt,tt,gains['oracle_wide'])
        assert np.all(wide<=tight+2e-7),'wide combined oracle violated certified dominance after f32 gain application'
        denominator=np.sqrt(pp*tt);valid=denominator>0;rho=np.divide(pt,denominator,out=np.zeros_like(pt),where=valid).clip(-1,1)
        support=valid&(pp/np.maximum(pp.sum(axis=1,keepdims=True),1e-30)>1e-6)&(tt/np.maximum(tt.sum(axis=1,keepdims=True),1e-30)>1e-6)
        mse_gain=np.divide(pt,pp,out=np.ones_like(pt),where=pp>0)
        env_gain=np.sqrt(np.divide(tt,pp,out=np.ones_like(tt),where=pp>0))
        total=np.maximum((pp+tt).sum(axis=1,keepdims=True),1e-10);an,bn,cn=pp/total,pt/total,tt/total
        opposing=(an-bn)*np.log((an+.001)/(cn+.001))<0
        for scope,mask in [('all_loss',domains['all_loss']),('runtime_eligible',eligible)]:
            for condition_index,condition in enumerate(('clean','noise20','noise10')):
                chosen=mask&(data['condition']==condition_index)
                for b in range(16):
                    good=chosen&valid[:,b];supported=chosen&support[:,b]
                    report['correlations'].append(dict(language=language,scope=scope,condition=condition,band=b,hz=[HZ[b],HZ[b+1]],
                        frames=int(chosen.sum()),valid_correlation_frames=int(good.sum()),rho_q25_q50_q75=quantiles(rho[good,b]),
                        energy_supported_frames=int(supported.sum()),supported_rho_q25_q50_q75=quantiles(rho[supported,b]),
                        opposing_gradients_fraction=float(opposing[good,b].mean()) if good.any() else None,
                        mdct_lower_env_above_one_fraction=float(((mse_gain[good,b]<=.7875)&(env_gain[good,b]>1)).mean()) if good.any() else None,
                        combined_gain_q25_q50_q75=quantiles(gains['oracle_combined'][chosen,b]),
                        current_gain_q25_q50_q75=quantiles(gains['current'][chosen,b]),
                        mdct_gain_q25_q50_q75=quantiles(gains['oracle_mdct'][chosen,b]),
                        envelope_gain_q25_q50_q75=quantiles(gains['oracle_env'][chosen,b]),
                        wide_gain_q25_q50_q75=quantiles(gains['oracle_wide'][chosen,b]),
                        combined_at_lower_fraction=float((gains['oracle_combined'][chosen,b]<=.787501).mean()) if chosen.any() else None,
                        combined_at_upper_fraction=float((gains['oracle_combined'][chosen,b]>=1.212499).mean()) if chosen.any() else None))
    a.output.write_text(json.dumps(report,indent=2,allow_nan=False)+'\n')
    print('Summarized',status['cases_per_variant'],'cases per variant; waveform, fixed-prediction and rho results separated')


if __name__=='__main__':main()
