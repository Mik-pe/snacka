#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Fixed causal17-head QAT distillation; phase-off/on are the same fitted weights.

These arrays require build_plc_phase_student.py and their adjacent training
report. The17-row output includes a fractional phase head and is not a legacy
16-gain decoder artifact.
"""
import argparse
import hashlib
import json
from pathlib import Path
import random
import time
import numpy as np
import torch
from torch import nn
import torch.nn.functional as F
from train_neural_plc import QATLinear,export

class Student(nn.Module):
    def __init__(self):
        super().__init__();self.layers=nn.ModuleList([QATLinear(80,64),QATLinear(64,64),QATLinear(64,17)])
        nn.init.zeros_(self.layers[-1].weight);nn.init.zeros_(self.layers[-1].bias)
    def forward(self,x,qat=True):
        for layer in self.layers[:2]:x=F.relu(layer(x,qat))
        raw=self.layers[-1](x,qat).tanh()
        return torch.cat((1+.2125*raw[:,:16],8*raw[:,16:]),1)


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('data',type=Path);p.add_argument('output',type=Path);p.add_argument('--protocol',type=Path,required=True);a=p.parse_args()
    if a.output.exists():p.error('fresh output required')
    if list(a.data.glob('test*')):raise ValueError('test present in student training')
    protocol=json.loads(a.protocol.read_text());spec=protocol['phase_student'];seed=spec['seed']
    random.seed(seed);np.random.seed(seed);torch.manual_seed(seed);torch.set_num_threads(2)
    device=torch.device('cpu');data={}
    for split in ('train','validation'):
        for language in (0,1):
            with np.load(a.data/f'{split}-{language}.npz',allow_pickle=False) as f:
                data[(split,language)]={k:torch.from_numpy(f[k].copy()) for k in f.files}
    model=Student();optimizer=torch.optim.AdamW(model.parameters(),lr=spec['optimizer']['lr'],weight_decay=spec['optimizer']['weight_decay'])
    started=time.monotonic();history=[]
    for epoch in range(spec['epochs']):
        totals=[]
        for step in range(spec['batches_per_epoch']):
            loss=0.
            for language in (0,1):
                d=data[('train',language)];idx=torch.randint(len(d['x']),(spec['frames_per_language'],));actual=model(d['x'][idx],epoch>=spec['float_warmup_epochs'])
                bands=13 if language==1 else 16
                gain=(torch.log(actual[:,:bands])-torch.log(d['gain'][idx,:bands])).square().mean()
                phase=F.smooth_l1_loss(actual[:,16]/8,d['phase'][idx]/8)
                loss=loss+.5*(gain+.1*phase+.01*(actual[:,:16]-1).square().mean()+.001*(actual[:,16]/8).square().mean())
            if not torch.isfinite(loss):raise ValueError('nonfinite student objective')
            optimizer.zero_grad(set_to_none=True);loss.backward();torch.nn.utils.clip_grad_norm_(model.parameters(),spec['optimizer']['gradient_clip']);optimizer.step();totals.append(float(loss.detach()))
        entry=dict(epoch=epoch+1,loss=sum(totals)/len(totals),seconds=time.monotonic()-started);history.append(entry);print(json.dumps(entry),flush=True)
    a.output.mkdir(parents=True);export(model,a.output/'neural_plc_weights.rs',1.)
    x=data[('validation',0)]['x'][:32]
    if len(x)!=32:raise ValueError('32 native parity fixtures required')
    np.concatenate((x.numpy(),model(x).detach().numpy()),1).astype('<f4').tofile(a.output/'parity.bin')
    torch.save({'model':model.state_dict(),'spec':spec,'epoch':spec['epochs']},a.output/'checkpoint.pt')
    report=dict(schema=1,spec=dict(gain_min=.7875,gain_max=1.2125,phase_max=8.,outputs=17),seed=seed,epochs=spec['epochs'],selected_epoch=spec['epochs'],history=history,
        teacher_provenance_sha256=hashlib.sha256((a.data/'teacher-provenance.json').read_bytes()).hexdigest(),
        protocol_sha256=hashlib.sha256(a.protocol.read_bytes()).hexdigest(),weights_sha256=hashlib.sha256((a.output/'neural_plc_weights.rs').read_bytes()).hexdigest(),
        torch=torch.__version__,device=str(device),seconds=time.monotonic()-started,selection='fixed epoch60; native validation required before selection')
    (a.output/'training-report.json').write_text(json.dumps(report,indent=2)+'\n')

if __name__=='__main__':main()
