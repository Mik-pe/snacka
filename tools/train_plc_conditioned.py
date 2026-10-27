#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Frozen six-fit, block-int8 residual head on native PR31 latent activations."""
import argparse,hashlib,json,math,os,time
os.environ.setdefault('CUBLAS_WORKSPACE_CONFIG',':4096:8')
from pathlib import Path
import numpy as np
import torch
from torch import nn
import torch.nn.functional as F
RADIUS=.10
LOW=math.log(.7875);HIGH=math.log(1.2125)
FITS=('history_wide','history_aligned','conditioned_wide','conditioned_aligned','history_damping','conditioned_damping')
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def quant(w):
 scale=w.detach().abs().amax(1,keepdim=True).clamp_min(1e-8)/127
 q=(w/scale).round().clamp(-127,127)*scale
 return w+(q-w).detach()
class Adapter(nn.Module):
 def __init__(self,conditioned,damp_only=False):
  super().__init__();self.wh=nn.Parameter(torch.zeros(16,64));self.wc=nn.Parameter(torch.zeros(16,16));self.bias=nn.Parameter(torch.zeros(16));self.conditioned=conditioned;self.damp_only=damp_only
 def delta(self,h,c,qat=True):
  wh=quant(self.wh) if qat else self.wh;wc=quant(self.wc) if qat else self.wc
  raw=F.linear(h,wh,self.bias)
  if self.conditioned:raw=raw+F.linear(c,wc)
  delta=RADIUS*torch.tanh(raw/RADIUS)
  return delta.clamp_max(0.) if self.damp_only else delta
 def gains(self,h,c,base,strength=1.):
  d=self.delta(h,c)*strength;z=(base.log()+d).clamp(LOW,HIGH);regular=z.exp()
  # Value parity at zero, while the training loss below retains its derivative.
  return torch.where(d==0,base,regular)
def export(model,path,strength=1.):
 lines=['// SPDX-License-Identifier: MIT','// Frozen candidate-conditioned PLC residual; see adjacent training report.','pub const FORMAT_TAG: u32 = 3;','pub const ENABLED: bool = true;',f'pub const USE_CANDIDATE: bool = {str(model.conditioned).lower()};',f'pub const DAMP_ONLY: bool = {str(model.damp_only).lower()};',f'pub const STRENGTH: f32 = {strength};']
 for name,weight,scale_name in [('WH',model.wh,'SH'),('WC',model.wc,'SC')]:
  w=weight.detach().cpu().numpy();scale=np.maximum(np.abs(w).max(1),1e-8)/np.float32(127);q=np.rint(w/scale[:,None]).clip(-127,127).astype(np.int8)
  lines.append(f'pub const {name}: [[i8; {w.shape[1]}]; 16] = [');lines.extend('['+', '.join(map(str,row))+'],' for row in q.tolist());lines.append('];')
  lines.append(f'pub const {scale_name}: [f32; 16] = ['+', '.join(f'f32::from_bits(0x{int(v):08x})' for v in scale.astype(np.float32).view(np.uint32))+'];')
 lines.append('pub const BIAS: [f32; 16] = ['+', '.join(f'f32::from_bits(0x{int(v):08x})' for v in model.bias.detach().cpu().numpy().astype(np.float32).view(np.uint32))+'];')
 path.write_text('\n'.join(lines)+'\n')
def load(root,split,language,device):
 parts=[]
 for path in sorted(root.glob(f'{split}-{language}-*.npz')):
  with np.load(path,allow_pickle=False) as f:parts.append({k:f[k].copy() for k in ('h','condition','target','current','wide','base','mask')})
 if not parts:raise ValueError('missing registered training shard')
 return {k:torch.from_numpy(np.concatenate([p[k] for p in parts])).to(device) for k in parts[0]}
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('data',type=Path);p.add_argument('output',type=Path);p.add_argument('--protocol',type=Path,required=True);p.add_argument('--binding',type=Path,required=True);a=p.parse_args()
 if a.output.exists():p.error('fresh output required')
 binding=json.loads(a.binding.read_text());protocol=json.loads(a.protocol.read_text());assert binding['training_allowed'] and binding['protocol_sha256']==sha(a.protocol)
 for name,digest in binding['code_sha256'].items():assert sha(Path(__file__).parent/name)==digest,name
 assert list(FITS)==protocol['fits']
 for name,digest in binding['data_sha256'].items():assert sha(a.data/name)==digest,name
 assert not list(a.data.glob('test*')) and torch.cuda.is_available(),'registered CUDA run required'
 torch.set_num_threads(2);torch.backends.cuda.matmul.allow_tf32=False;torch.backends.cudnn.allow_tf32=False;torch.use_deterministic_algorithms(True)
 device=torch.device('cuda');data={l:load(a.data,'train',l,device) for l in ('en','sv')};validation={l:load(a.data,'validation',l,device) for l in ('en','sv')};a.output.mkdir(parents=True);reports=[];spec=protocol['training']
 for fit in FITS:
  torch.manual_seed(spec['seed']);torch.cuda.manual_seed_all(spec['seed']);model=Adapter(fit.startswith('conditioned'),fit.endswith('damping')).to(device);optimizer=torch.optim.AdamW(model.parameters(),lr=spec['lr'],weight_decay=spec['weight_decay']);history=[];torch.cuda.synchronize();started=time.monotonic();accum=[]
  for step in range(spec['updates']):
   lr=spec['min_lr']+.5*(spec['lr']-spec['min_lr'])*(1+math.cos(math.pi*step/max(spec['updates']-1,1)))
   for group in optimizer.param_groups:group['lr']=lr
   loss=0.
   for language in ('en','sv'):
    d=data[language];idx=torch.randint(len(d['h']),(spec['frames_per_language'],),device=device);delta=model.delta(d['h'][idx],d['condition'][idx],step>=spec['float_warmup']);zbase=d['base'][idx].log();predicted=d['wide' if fit.endswith('wide') else 'current'][idx];target=(.5*(d['target'][idx]-predicted)).clamp(LOW,HIGH);mask=d['mask'][idx]
    if model.damp_only:target=torch.minimum(target,zbase)
    # Supervision before student clamp retains correction gradients at a wrong bound.
    per_band=(zbase+delta-target).square()+spec['baseline_anchor']*delta.square();loss=loss+.5*((per_band*mask).sum(-1)/mask.sum(-1)).mean()
   optimizer.zero_grad(set_to_none=True);loss.backward();torch.nn.utils.clip_grad_norm_(model.parameters(),spec['gradient_clip']);optimizer.step();accum.append(loss.detach())
   if (step+1)%1000==0:
    value=float(torch.stack(accum).mean());accum=[]
    if not math.isfinite(value):raise ValueError('nonfinite objective')
    entry=dict(step=step+1,loss=value,seconds=time.monotonic()-started,lr=lr);history.append(entry);print(json.dumps(dict(fit=fit,**entry)),flush=True)
  torch.cuda.synchronize();seconds=time.monotonic()-started;out=a.output/fit;out.mkdir();export(model,out/'plc_conditioned_weights.rs');torch.save(dict(model=model.state_dict(),fit=fit,spec=spec),out/'checkpoint.pt')
  fixtures={k:torch.cat([validation[l][k][:16] for l in ('en','sv')]) for k in ('h','condition','base')}
  if not model.conditioned:fixtures['condition']=torch.zeros_like(fixtures['condition'])
  with torch.no_grad():
   for strength in protocol['strengths']:
    expected=model.gains(fixtures['h'],fixtures['condition'],fixtures['base'],strength);np.concatenate([fixtures['h'].cpu().numpy(),fixtures['condition'].cpu().numpy(),fixtures['base'].cpu().numpy(),expected.cpu().numpy()],1).astype('<f4').tofile(out/f'parity-{strength}.bin')
  report=dict(schema=1,fit=fit,model_kind='pr31-conditional-residual-v1',format_tag=3,uses_candidate=model.conditioned,damp_only=model.damp_only,phase='unchanged frozen PR31',teacher='future-informed PhaseWide' if fit.endswith('wide') else 'actual PR31 phase',updates=spec['updates'],selected_step=spec['updates'],history=history,seconds=seconds,weights_sha256=sha(out/'plc_conditioned_weights.rs'),binding_sha256=sha(a.binding),protocol_sha256=sha(a.protocol),torch=torch.__version__,device=torch.cuda.get_device_name(),logical_adapter_bytes=1472,rows={l:len(data[l]['h']) for l in data})
  (out/'training-report.json').write_text(json.dumps(report,indent=2)+'\n');reports.append(report)
 (a.output/'training-summary.json').write_text(json.dumps(reports,indent=2)+'\n')
if __name__=='__main__':main()
