#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Preregistered, fixed-topology applied-gain QAT loss ablations; no test input."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import random
import time
import numpy as np
import torch
from train_neural_plc import load, export
from plc_ablation_loss import AppliedPredictor, band_terms, objective, sequence_prediction, mrstft


def load_sequences(directory, split, device):
    groups=[]
    for path in sorted(directory.glob(split+'-*.npz')):
        _,language,n,h=path.stem.split('-')
        with np.load(path,allow_pickle=False) as data:
            arrays={k:torch.as_tensor(data[k],device=device,dtype=torch.float32) for k in data.files}
        groups.append((language,int(n),int(h),arrays))
    if not groups:raise ValueError('missing sequence split')
    return groups


def sequence_loss(model, group, basis, indices=None, qat=True):
    language,n,_,data=group
    batch={k:v[indices] for k,v in data.items()} if indices is not None else data
    actual=sequence_prediction(model,batch,basis[n],qat)
    return mrstft(actual,batch['target'],16000 if language=='sv' else 48000)


@torch.no_grad()
def evaluate(model,data,spec,groups,basis):
    gain=model(data['x']);per_example=objective(gain,data,spec)
    coherent,envelope=band_terms(gain,data,7,1.0)
    seq_by_language={language:[] for language in ('en','sv')}
    for group in groups:
        values=[]
        for start in range(0,len(group[3]['x']),32):
            values.append(sequence_loss(model,group,basis,slice(start,start+32)))
        seq_by_language[group[0]].append(torch.cat(values))
    sequence={language:torch.cat(values).mean().item() for language,values in seq_by_language.items()}
    seq_mean=sum(sequence.values())/2
    result={'balanced_objective':(per_example*data['weight']).mean().item()+spec['sequence_weight']*seq_mean,
            'common_validation_score':((coherent+0.1*envelope)*data['weight']).mean().item()+0.1*seq_mean,
            'mrstft':sequence,'gain_min':gain.min().item(),'gain_max':gain.max().item()}
    for language,name in ((0,'en'),(1,'sv')):
        mask=data['language']==language
        identity=torch.ones_like(gain)
        original,_=band_terms(identity,data,7,1.0)
        result[name]={'records':int(mask.sum()),'lowband_error':coherent[mask].mean().item(),
                      'lowband_error_ratio_vs_classical':(coherent[mask].mean()/original[mask].mean()).item(),
                      'log_band_energy_mse':envelope[mask].mean().item()}
    return result


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('data',type=Path);p.add_argument('sequences',type=Path)
    p.add_argument('output',type=Path);p.add_argument('--protocol',type=Path,required=True)
    p.add_argument('--experiment',choices=['control','2A','2B','2C','2D'],required=True)
    p.add_argument('--best-loss',choices=['2A','2B','2C'],help='validation-selected parent for 2D')
    p.add_argument('--epochs',type=int,help='diagnostic override; cannot be accepted as registered run')
    a=p.parse_args();protocol=json.loads(a.protocol.read_text());spec=next(e.copy() for e in protocol['experiments'] if e['name']==a.experiment)
    if a.experiment=='2D':
        if not a.best_loss:p.error('2D requires validation-selected best loss')
        parent=next(e for e in protocol['experiments'] if e['name']==a.best_loss)
        spec={**parent,**spec,'parent_loss':a.best_loss}
    if a.output.exists():p.error('fresh result directory required')
    a.output.mkdir(parents=True)
    if list(a.data.glob('test*.npz')) or list(a.sequences.glob('test*.npz')):raise ValueError('test staged in training')
    seed=protocol['seed'];random.seed(seed);np.random.seed(seed);torch.manual_seed(seed)
    device=torch.device('cuda' if torch.cuda.is_available() else 'cpu')
    if device.type=='cuda':torch.cuda.manual_seed_all(seed);torch.backends.cuda.matmul.allow_tf32=False
    torch.set_num_threads(2)
    train=load(a.data,'train',device);validation=load(a.data,'validation',device)
    sequences={split:load_sequences(a.sequences,split,device) for split in ('train','validation')}
    basis={n:torch.as_tensor(np.fromfile(a.sequences/f'basis-{n}.f32',dtype='<f4').reshape(n,n+120),device=device) for n in (480,960)}
    model=AppliedPredictor(spec['gain_min'],spec['gain_max']).to(device)
    optimizer=torch.optim.AdamW(model.parameters(),lr=protocol['optimizer']['lr'],weight_decay=protocol['optimizer']['weight_decay'])
    best=float('inf');best_state=None;best_epoch=None;history=[];started=time.monotonic()
    epochs=a.epochs or protocol['epochs'];warmup=min(protocol['float_warmup_epochs'],epochs-1)
    for epoch in range(epochs):
        order=torch.randperm(len(train['x']),device=device);model.train()
        for step,indices in enumerate(order.split(protocol['optimizer']['batch_size'])):
            batch={k:v[indices] for k,v in train.items()};gain=model(batch['x'],epoch>=warmup)
            loss=(objective(gain,batch,spec)*batch['weight']).mean()
            if spec['sequence_weight']:
                # Private RNG: adding this term must not change the feature shuffle.
                rng=random.Random(seed+epoch*100000+step)
                for language in ('en','sv'):
                    candidates=[g for g in sequences['train'] if g[0]==language];group=candidates[rng.randrange(len(candidates))]
                    selection=torch.tensor([rng.randrange(len(group[3]['x'])) for _ in range(8)],device=device)
                    loss=loss+spec['sequence_weight']*0.5*sequence_loss(model,group,basis,selection,epoch>=warmup).mean()
            optimizer.zero_grad(set_to_none=True);loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(),protocol['optimizer']['gradient_clip']);optimizer.step()
        model.eval();metrics=evaluate(model,validation,spec,sequences['validation'],basis)
        entry={'epoch':epoch+1,'seconds':time.monotonic()-started,**metrics};history.append(entry)
        if epoch>=warmup and metrics['balanced_objective']<best:
            best=metrics['balanced_objective'];best_state=copy.deepcopy(model.state_dict());best_epoch=epoch+1
        print(json.dumps(entry),flush=True)
    model.load_state_dict(best_state)
    export(model,a.output/'neural_plc_weights.rs',1.0)
    inputs=validation['x'][:32];gains=model(inputs).detach().cpu().numpy()
    np.concatenate((inputs.cpu().numpy(),gains),axis=1).astype('<f4').tofile(a.output/'parity.bin')
    torch.save({'model':model.state_dict(),'spec':spec,'epoch':best_epoch},a.output/'checkpoint.pt')
    hashes={str(f.relative_to(a.data)):hashlib.sha256(f.read_bytes()).hexdigest() for f in sorted(a.data.glob('*.npz'))}
    report={'experiment':a.experiment,'spec':spec,'seed':seed,'epochs':epochs,'registered_run':a.epochs is None,
            'training_records':len(train['x']),'validation_records':len(validation['x']),'selected_epoch':best_epoch,
            'validation':evaluate(model,validation,spec,sequences['validation'],basis),'history':history,
            'protocol_sha256':hashlib.sha256(a.protocol.read_bytes()).hexdigest(),
            'dataset_sha256':hashes,'sequence_provenance_sha256':hashlib.sha256((a.sequences/'sequence-provenance.json').read_bytes()).hexdigest(),
            'weights_sha256':hashlib.sha256((a.output/'neural_plc_weights.rs').read_bytes()).hexdigest(),
            'seconds':time.monotonic()-started,'torch':torch.__version__,'device':str(device),
            'accelerator':torch.cuda.get_device_name() if device.type=='cuda' else 'CPU'}
    (a.output/'training-report.json').write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
