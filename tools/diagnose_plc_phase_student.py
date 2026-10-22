#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Fixed-checkpoint scalar distillation diagnostics; never used for model ranking."""
import argparse
import hashlib
import json
from pathlib import Path
import tempfile
import numpy as np
import torch
from train_plc_phase_student import Student
from train_neural_plc import export


def summary(actual,gain,phase,language):
    bands=13 if language==1 else 16
    error=(np.log(actual[:,:bands])-np.log(gain[:,:bands]))**2
    neutral=np.log(gain[:,:bands])**2
    phase_error=np.mean((actual[:,16]-phase)**2);zero_error=np.mean(phase**2)
    return dict(records=len(actual),gain_log_mse=float(error.mean()),identity_gain_log_mse=float(neutral.mean()),
        phase_mse_samples=float(phase_error),zero_phase_mse_samples=float(zero_error),
        phase_mse_ratio_vs_zero=float(phase_error/zero_error) if zero_error>0 else None,
        predicted_phase_abs_mean=float(np.abs(actual[:,16]).mean()),target_phase_abs_mean=float(np.abs(phase).mean()),
        predicted_phase_quartiles=np.quantile(actual[:,16],[.25,.5,.75]).tolist(),
        target_phase_quartiles=np.quantile(phase,[.25,.5,.75]).tolist(),
        target_phase_bound_fraction=float(np.mean(np.abs(phase)==8)),predicted_phase_bound_fraction=float(np.mean(np.abs(actual[:,16])>=7.95)),
        predicted_gain_mean=float(actual[:,:bands].mean()),target_gain_mean=float(gain[:,:bands].mean()))


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('data',type=Path);p.add_argument('model',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    if a.output.exists():p.error('fresh diagnostics output required')
    torch.set_num_threads(2);model=Student();saved=torch.load(a.model/'checkpoint.pt',map_location='cpu',weights_only=True);model.load_state_dict(saved['model']);model.eval()
    report=json.loads((a.model/'training-report.json').read_text())
    with tempfile.TemporaryDirectory() as temporary:
        path=Path(temporary)/'weights.rs';export(model,path,1.)
        if hashlib.sha256(path.read_bytes()).hexdigest()!=report['weights_sha256']:raise ValueError('checkpoint does not reproduce exported quantized weights')
    result=dict(purpose='Diagnostic only; neither candidate ranking nor a causal/perceptual ceiling',weights_sha256=report['weights_sha256'],splits={})
    with torch.no_grad():
        for split in ('train','validation'):
            result['splits'][split]={}
            for language in (0,1):
                with np.load(a.data/f'{split}-{language}.npz',allow_pickle=False) as d:
                    actual=model(torch.from_numpy(d['x'])).numpy();groups={'all':np.ones(len(actual),dtype=bool)}
                    horizon=np.rint(d['x'][:,74]*6).astype(int)+1
                    for h in range(1,7):groups[f'horizon{h}']=horizon==h
                    groups['last_received_voiced']=d['x'][:,66]>=1;groups['last_received_unvoiced']=d['x'][:,66]<1
                    result['splits'][split]['sv' if language else 'en']={name:summary(actual[mask],d['gain'][mask],d['phase'][mask],language) for name,mask in groups.items() if mask.any()}
    a.output.write_text(json.dumps(result,indent=2,allow_nan=False)+'\n')
    print(json.dumps({split:{language:values['all'] for language,values in languages.items()} for split,languages in result['splits'].items()},indent=2))

if __name__=='__main__':main()
