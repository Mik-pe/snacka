#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Fixed-checkpoint causal gain experiment. Selection requires native validation."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import time
import numpy as np
import torch
from train_neural_plc import export
from train_plc_ablation import load_sequences
from plc_ablation_loss import AppliedPredictor, HZ, sequence_prediction, mrstft


def terms(model, data, basis, language, spec, qat=True):
    x,prediction,target=data['x'],data['prediction'],data['target_coefficients']
    gains=model(x.flatten(0,1),qat).reshape(*x.shape[:2],16)
    eligible=(x[:,:,78]>=1)&(x[:,:,76]<.985)&(x[:,:,73]<x[:,:,77])&(prediction.abs().sum(-1)>0)
    gains=torch.where(eligible[:,:,None],gains,1.)
    n=prediction.shape[-1];edges=HZ*n//24000
    pp=torch.stack([prediction[:,:,lo:hi].square().sum(-1) for lo,hi in zip(edges[:-1],edges[1:])],-1)
    pt=torch.stack([(prediction[:,:,lo:hi]*target[:,:,lo:hi]).sum(-1) for lo,hi in zip(edges[:-1],edges[1:])],-1)
    tt=torch.stack([target[:,:,lo:hi].square().sum(-1) for lo,hi in zip(edges[:-1],edges[1:])],-1)
    active=pp*gains.square()+tt>1e-10
    scale=(pp+tt).sum(-1,keepdim=True).clamp_min(1e-10);pp,pt,tt=pp/scale,pt/scale,tt/scale
    coherent=(pp*gains.square()-2*pt*gains+tt).clamp_min(0)[:,:,:7].sum(-1).mean()
    difference=torch.log(pp*gains.square()+.001)-torch.log(tt+.001)
    if spec.get('envelope_normalization','').startswith('applied'):
        normalizer=(pp*gains.square()+tt).sum(-1,keepdim=True).clamp_min(1e-10)
        difference=torch.log(pp*gains.square()/normalizer+.001)-torch.log(tt/normalizer+.001)
        mask=active.clone()
        if language=='sv':mask[:,:,13:]=False
        envelope=(difference.square()*mask).sum()/mask.sum().clamp_min(1)
    else:
        envelope=difference[:,:,:13 if language=='sv' else 16].square().mean()
    actual=torch.cat((data['preceding'],sequence_prediction(model,data,basis,qat)),1)
    reference=torch.cat((data['target_preceding'],data['target']),1)
    spectral=mrstft(actual,reference,16000 if language=='sv' else 48000).mean()
    identity=(gains-1).square().mean()
    total=spec['coherent_weight']*coherent+spec['envelope_weight']*envelope+spec['mrstft_weight']*spectral+.01*identity
    return total,dict(coherent=coherent,envelope=envelope,mrstft=spectral)


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('sequences',type=Path);p.add_argument('output',type=Path)
    p.add_argument('--protocol',type=Path,required=True);p.add_argument('--experiment',choices=['control','envelope'],required=True)
    a=p.parse_args();protocol=json.loads(a.protocol.read_text());spec=protocol['models'][a.experiment]
    if a.output.exists():p.error('fresh output required')
    if list(a.sequences.glob('test*')):raise ValueError('test staged in training')
    provenance=json.loads((a.sequences/'sequence-provenance.json').read_text())
    if provenance['schema']!=2:raise ValueError('a full preceding received frame is required')
    a.output.mkdir(parents=True);seed=protocol['seed'];random.seed(seed);np.random.seed(seed);torch.manual_seed(seed)
    device=torch.device('cuda' if torch.cuda.is_available() else 'cpu');torch.set_num_threads(2)
    if device.type=='cuda':torch.cuda.manual_seed_all(seed);torch.backends.cuda.matmul.allow_tf32=False
    groups={s:load_sequences(a.sequences,s,device) for s in ('train','validation')}
    basis={n:torch.as_tensor(np.fromfile(a.sequences/f'basis-{n}.f32',dtype='<f4').reshape(n,n+120),device=device) for n in (480,960)}
    model=AppliedPredictor(spec['gain_min'],spec['gain_max']).to(device)
    optimizer=torch.optim.AdamW(model.parameters(),lr=.002,weight_decay=.0001)
    rng=random.Random(seed);history=[];started=time.monotonic()
    for epoch in range(protocol['epochs']):
        model.train();totals=[]
        for step in range(protocol['batches_per_epoch']):
            loss=0
            for language in ('en','sv'):
                candidates=[g for g in groups['train'] if g[0]==language]
                # Sample events uniformly, not duration/horizon groups uniformly.
                group=rng.choices(candidates,weights=[len(g[3]['x']) for g in candidates],k=1)[0]
                indices=torch.tensor([rng.randrange(len(group[3]['x'])) for _ in range(protocol['sequences_per_language'])],device=device)
                data={k:v[indices] for k,v in group[3].items()}
                component,_=terms(model,data,basis[group[1]],language,spec,epoch>=protocol['float_warmup_epochs'])
                loss=loss+.5*component
            optimizer.zero_grad(set_to_none=True);loss.backward();torch.nn.utils.clip_grad_norm_(model.parameters(),1.);optimizer.step()
            totals.append(float(loss.detach()))
        entry={'epoch':epoch+1,'loss':sum(totals)/len(totals),'seconds':time.monotonic()-started};history.append(entry)
        print(json.dumps(entry),flush=True)
    # One predeclared checkpoint: no loss-based epoch selection or test feedback.
    model.eval();export(model,a.output/'neural_plc_weights.rs',1.)
    x=torch.cat([g[3]['x'].flatten(0,1) for g in groups['validation']])[:32]
    np.concatenate((x.cpu().numpy(),model(x).detach().cpu().numpy()),1).astype('<f4').tofile(a.output/'parity.bin')
    torch.save({'model':model.state_dict(),'spec':spec,'epoch':protocol['epochs']},a.output/'checkpoint.pt')
    report=dict(experiment=a.experiment,spec=spec,seed=seed,epochs=protocol['epochs'],selected_epoch=protocol['epochs'],
        selection='fixed checkpoint; not eligible for adoption before native decoded validation guards',history=history,
        protocol_sha256=hashlib.sha256(a.protocol.read_bytes()).hexdigest(),
        sequence_provenance_sha256=hashlib.sha256((a.sequences/'sequence-provenance.json').read_bytes()).hexdigest(),
        weights_sha256=hashlib.sha256((a.output/'neural_plc_weights.rs').read_bytes()).hexdigest(),
        seconds=time.monotonic()-started,torch=torch.__version__,device=str(device),
        accelerator=torch.cuda.get_device_name() if device.type=='cuda' else 'CPU')
    (a.output/'training-report.json').write_text(json.dumps(report,indent=2)+'\n')

if __name__=='__main__':main()
