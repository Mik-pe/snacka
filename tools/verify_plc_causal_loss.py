#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Check native context, identity reconstruction, finite QAT gradients and no speaker leakage."""
import argparse
import json
from pathlib import Path
import numpy as np
import torch
from train_plc_ablation import load_sequences
from train_plc_causal import terms
from plc_ablation_loss import AppliedPredictor, sequence_prediction


def main():
    p=argparse.ArgumentParser();p.add_argument('sequences',type=Path);p.add_argument('manifest',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    groups=load_sequences(a.sequences,'train',torch.device('cpu'))
    model=AppliedPredictor(.7875,1.2125);worst=0.;checked=0
    for language,n,h,arrays in groups:
        data={k:v[:2] for k,v in arrays.items()}
        basis=torch.from_numpy(np.fromfile(a.sequences/f'basis-{n}.f32',dtype='<f4').reshape(n,n+120))
        actual=sequence_prediction(model,data,basis)
        gap=float((actual-data['baseline']).abs().max().detach());worst=max(worst,gap)
        if gap>=2e-5:raise ValueError('Torch/native identity/OLA mismatch')
        if not torch.equal(data['preceding'],data['target_preceding']):raise ValueError('previous received context mismatch')
        loss,_=terms(model,data,basis,language,dict(coherent_weight=.01,envelope_weight=1.,mrstft_weight=.1))
        loss.backward()
        if not torch.isfinite(loss) or any(p.grad is not None and not torch.isfinite(p.grad).all() for p in model.parameters()):raise ValueError('nonfinite gradients')
        model.zero_grad();checked+=len(data['x'])
    rows=[json.loads(line) for line in a.manifest.read_text().splitlines()];splits={}
    for row in rows:
        key=(row['source'],row['group_id'])
        if key in splits and splits[key]!=row['split']:raise ValueError('speaker leakage')
        splits[key]=row['split']
        if row['split']=='test':raise ValueError('test staged in train/validation corpus')
    report=dict(status='PASS',identity_max_abs=worst,sequences_checked=checked,groups_checked=len(groups),
                finite_qat_gradients=True,full_preceding_frame=True,speaker_split_disjoint=True)
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))

if __name__=='__main__':main()
