#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Four fixed-final-checkpoint QAT fits; requires sealed data/code binding."""
import argparse,hashlib,json,time,random,subprocess
from pathlib import Path
import numpy as np
import torch
from torch import nn
import torch.nn.functional as F
from train_neural_plc import QATLinear,export
from plc_target_energy_loss import objective
class Student(nn.Module):
 def __init__(self):
  super().__init__();self.layers=nn.ModuleList([QATLinear(80,64),QATLinear(64,64),QATLinear(64,17)])
  nn.init.zeros_(self.layers[-1].weight);nn.init.zeros_(self.layers[-1].bias)
 def forward(self,x,qat=True):
  for layer in self.layers[:2]:x=F.relu(layer(x,qat))
  raw=self.layers[2](x,qat)
  return raw[:,:16],8*raw[:,16].tanh()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('data',type=Path);p.add_argument('output',type=Path);p.add_argument('--binding',type=Path,required=True);a=p.parse_args()
 if a.output.exists():p.error('fresh output required')
 binding=json.loads(a.binding.read_text());assert binding['training_allowed'] is True
 repo=Path(__file__).resolve().parents[1]
 subprocess.run(['git','merge-base','--is-ancestor',binding['source_commit'],'HEAD'],cwd=repo,check=True)
 for name,digest in binding['source_sha256'].items():assert sha(repo/name)==digest,name
 for name,digest in binding['data_sha256'].items():assert sha(a.data/name)==digest,name
 assert not list(a.data.glob('test*'))
 a.output.mkdir(parents=True);torch.set_num_threads(2)
 protocol=json.loads((repo/'docs/neural-plc-target-energy-protocol.json').read_text());spec=protocol['training'];reports=[]
 for fit in protocol['fits']:
  seed=spec['seed'];random.seed(seed);np.random.seed(seed);torch.manual_seed(seed)
  representation='target_energy' if fit.startswith('energy') else 'gain_ratio';factor=fit.split('_')[1]
  kind='target-log-power-phase-v1' if representation=='target_energy' else 'gain-ratio-phase-v1'
  data={}
  for lang in (0,1):
   with np.load(a.data/f'{factor}-{lang}.npz',allow_pickle=False) as f:data[lang]={k:torch.from_numpy(f[k].copy()) for k in f.files}
  model=Student();optimizer=torch.optim.AdamW(model.parameters(),lr=spec['optimizer']['lr'],weight_decay=spec['optimizer']['weight_decay']);history=[];started=time.monotonic()
  initial_sha=hashlib.sha256(b''.join(v.detach().numpy().tobytes() for v in model.parameters())).hexdigest()
  for epoch in range(spec['rounds']):
   totals=[]
   for step in range(spec['updates_per_round']):
    loss=0.
    for lang in (0,1):
     d=data[lang];idx=torch.randint(len(d['x']),(spec['frames_per_language_per_update'],));raw,phase=model(d['x'][idx],epoch>=spec['float_warmup_rounds'])
     loss=loss+.5*objective(raw,representation,d['last'][idx],d['predicted'][idx],d['target'][idx],d['mask'][idx],phase,d['phase'][idx])
    if not torch.isfinite(loss):raise ValueError('nonfinite objective')
    optimizer.zero_grad(set_to_none=True);loss.backward();torch.nn.utils.clip_grad_norm_(model.parameters(),spec['optimizer']['gradient_clip']);optimizer.step();totals.append(float(loss.detach()))
   history.append(dict(round=epoch+1,loss=sum(totals)/len(totals),seconds=time.monotonic()-started))
  out=a.output/fit;out.mkdir();export(model,out/'neural_plc_weights.rs',1.)
  path=out/'neural_plc_weights.rs';path.write_text(path.read_text()+f'pub(crate) const MODEL_KIND: &str = "{kind}";\npub(crate) const FORMAT_TAG: u32 = {2 if representation=="target_energy" else 1};\n')
  with np.load(a.data/'validation-0.npz',allow_pickle=False) as f:x=torch.from_numpy(f['x'][:32].copy())
  raw,phase=model(x);fixture=np.concatenate([x.numpy(),raw.detach().numpy(),phase[:,None].detach().numpy()],axis=1).astype('<f4');fixture.tofile(out/'raw-parity.bin')
  torch.save(dict(model=model.state_dict(),spec=spec,model_kind=kind,round=spec['rounds']),out/'checkpoint.pt')
  diagnostics={}
  provenance=json.loads((a.data/'teacher-provenance.json').read_text())['utterances']
  for lang in (0,1):
   d=data[lang]
   with torch.no_grad():
    raw,phase=model(d['x']);z=torch.log(1+.2125*raw.tanh()) if representation=='gain_ratio' else .5*(raw+d['last']-d['predicted'])
   diagnostics[str(lang)]=dict(rows=len(raw),phase_rmse=float((phase-d['phase']).square().mean().sqrt()),zero_phase_rmse=float(d['phase'].square().mean().sqrt()),phase_mean_abs=float(phase.abs().mean()),log_gain_quantiles=np.quantile(z.numpy(),[0,.25,.5,.75,1]).tolist(),delta_quantiles=np.quantile((d['target']-d['last']).numpy(),[0,.25,.5,.75,1]).tolist(),teacher_saturation_fraction=float(((.5*(d['target']-d['predicted'])<np.log(.7875))|(.5*(d['target']-d['predicted'])>np.log(1.2125))).float().mean()))
   strata=[]
   rates=np.array([provenance[int(i)]['bitrate'] for i in d['record'].numpy()]);bandwidth=np.array([provenance[int(i)]['effective_rate'] for i in d['record'].numpy()]);horizons=np.rint(d['x'][:,74].numpy()*6).astype(int)+1
   for rate in (12000,24000,32000):
    for bw in (16000,48000):
     for horizon in range(1,7):
      selected=(rates==rate)&(bandwidth==bw)&(horizons==horizon)
      if selected.any():
       z_part=z.numpy()[selected];t=.5*(d['target'].numpy()[selected]-d['predicted'].numpy()[selected]);strata.append(dict(bitrate=rate,effective_rate=bw,horizon=horizon,rows=int(selected.sum()),predicted_log_gain_quantiles=np.quantile(z_part,[0,.25,.5,.75,1]).tolist(),target_delta_quantiles=np.quantile((d['target']-d['last']).numpy()[selected],[0,.25,.5,.75,1]).tolist(),teacher_saturation_fraction=float(((t<np.log(.7875))|(t>np.log(1.2125))).mean()),phase_rmse=float(np.sqrt(np.mean((phase.numpy()[selected]-d['phase'].numpy()[selected])**2))),zero_phase_rmse=float(np.sqrt(np.mean(d['phase'].numpy()[selected]**2)))))
   diagnostics[str(lang)]['strata']=strata
  report=dict(schema=1,fit=fit,model_kind=kind,representation=representation,architecture=[80,64,64,17],spec=spec,initial_sha256=initial_sha,updates=spec['rounds']*spec['updates_per_round'],history=history,seconds=time.monotonic()-started,binding_sha256=sha(a.binding),weights_sha256=sha(path),torch=torch.__version__,device='cpu',diagnostics=diagnostics)
  (out/'training-report.json').write_text(json.dumps(report,indent=2)+'\n');reports.append(report);print(json.dumps(dict(fit=fit,seconds=report['seconds'],loss=history[-1]['loss'])),flush=True)
 assert len({r['initial_sha256'] for r in reports})==1
 (a.output/'summary.json').write_text(json.dumps(reports,indent=2)+'\n')
if __name__=='__main__':main()
