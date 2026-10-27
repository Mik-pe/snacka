#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Multi-profile, bilingual native labels; future targets never enter decoder state."""
import argparse,hashlib,json,itertools,subprocess,tempfile,time
from concurrent.futures import ProcessPoolExecutor
from functools import partial
from pathlib import Path
import numpy as np
from prepare_plc_energy_study import FILTER
SALT='snacka-conditioned-20261027:'
def digest(b):return hashlib.sha256(b).hexdigest()
def task(item,corpus,binary):
 row,bw,rate,dur,mode=item;path=corpus/row['path'];assert digest(path.read_bytes())==row['flac_sha256']
 key=int(digest((SALT+row['path']).encode())[:16],16);profile=(12000,24000,32000).index(rate)*4+(0 if dur==10 else 2)+mode
 condition=(key+profile)%3;gain_db=(0,-12,-24)[(key//3+profile)%3]
 command=['ffmpeg','-v','error','-i',str(path),'-ac','1']
 if row['native_rate']==48000 and bw==16000:command+=['-af',FILTER]
 pcm=np.frombuffer(subprocess.check_output(command+['-ar','48000','-f','f32le','pipe:1']),dtype='<f4').copy();source_sha=digest(pcm.tobytes())
 if condition:
  noise=np.random.default_rng((key+profile)&((1<<64)-1)).normal(size=len(pcm));power=np.mean(pcm.astype(np.float64)**2);noise*=np.sqrt(power/(np.mean(noise**2)*10**((20 if condition==1 else 10)/10)));pcm+=noise.astype(np.float32)
 pcm=np.clip(pcm*np.float32(10**(gain_db/20)),-1,1).astype('<f4').tobytes()
 with tempfile.TemporaryDirectory(prefix='snacka-cond-export-') as temporary:
  root=Path(temporary);(root/'in.f32').write_bytes(pcm)
  run=subprocess.run([str(binary),str(root/'in.f32'),str(root/'labels.bin'),str(root/'audit.bin'),str(rate),str(dur),str(mode),'12',str(key%12)],capture_output=True)
  if run.returncode:raise RuntimeError('native teacher failed: '+run.stderr.decode()[:400])
  raw=(root/'labels.bin').read_bytes();assert raw[:8]==b'SPCC\x01\0\0\0' and (len(raw)-8)%908==0
  values=np.frombuffer(raw[8:],dtype='<f4').reshape(-1,227).copy();assert np.isfinite(values).all();assert np.isin(values[:,225],[0.,1.]).all();selected=values[values[:,225]==1]
  audit=digest((root/'audit.bin').read_bytes())
 receipt={k:v for k,v in row.items() if k!='corpus_root'};receipt.update(effective_rate=bw,bitrate=rate,frame_ms=dur,harmonic=mode,condition=['clean','noise20','noise10'][condition],gain_db=gain_db,source_pcm_sha256=source_sha,input_sha256=digest(pcm),labels_sha256=digest(raw),baseline_audit_sha256=audit,records=len(values),eligible=len(selected))
 return selected,receipt

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('corpus',type=Path);p.add_argument('output',type=Path);p.add_argument('--binary',type=Path,required=True);p.add_argument('--workers',type=int,default=2);a=p.parse_args()
 if a.output.exists():p.error('fresh output required')
 a.output.mkdir(parents=True);manifest=json.loads((a.corpus/'training-selection.json').read_text());assert len(manifest)==344 and {r['split'] for r in manifest}=={'train','validation'}
 train={r['group_id'] for r in manifest if r['split']=='train'};val={r['group_id'] for r in manifest if r['split']=='validation'};assert not train&val
 items=[(r,bw,rate,dur,mode) for r in manifest for bw in ([16000,48000] if r['language']=='en' else [16000]) for rate,dur,mode in itertools.product([12000,24000,32000],[10,20],[0,1])]
 groups={};receipts=[];started=time.monotonic()
 with ProcessPoolExecutor(max_workers=a.workers) as pool:
  for i,(values,row) in enumerate(pool.map(partial(task,corpus=a.corpus/'corpus-registered',binary=a.binary.resolve()),items)):
   rid=len(receipts);receipts.append(row);parts=dict(h=values[:,80:144],condition=(.5*(values[:,160:176]-values[:,144:160])).clip(-4,4),target=values[:,176:192],current=values[:,160:176],wide=values[:,192:208],base=values[:,208:224],phase=values[:,224],horizon=values[:,226],record=np.full(len(values),rid,np.int32),mask=np.tile(np.array([1.]*(13 if row['effective_rate']==16000 else 16)+[0.]*(3 if row['effective_rate']==16000 else 0),np.float32),(len(values),1)))
   groups.setdefault((row['split'],row['language']),[]).append(parts)
   if i%100==0:print(json.dumps(dict(completed=i+1,total=len(items),seconds=time.monotonic()-started)),flush=True)
 files=[]
 for (split,language),parts in sorted(groups.items()):
  data={k:np.concatenate([p[k] for p in parts]) for k in parts[0]};n=len(data['h'])
  for start in range(0,n,100000):
   name=f'{split}-{language}-{start//100000:03}.npz';path=a.output/name;np.savez_compressed(path,**{k:v[start:start+100000] for k,v in data.items()});files.append(dict(name=name,rows=min(100000,n-start),sha256=digest(path.read_bytes())))
 valrows=[]
 for row in manifest:
  if row['split']=='validation':
   for bw in ([16000,48000] if row['language']=='en' else [16000]):valrows.append(dict(row,effective_rate=bw,corpus_root=str(a.corpus/'corpus-registered')))
 (a.output/'validation-selection.json').write_text(json.dumps(valrows,indent=2)+'\n')
 report=dict(python_source_sha256=digest(Path(__file__).read_bytes()),schema=1,format='SPCC1-baseline-state-phase-aligned',source_manifest_sha256=digest((a.corpus/'training-selection.json').read_bytes()),binary_sha256=digest(a.binary.read_bytes()),phase_and_trunk='frozen installed PR31; exact native latent and actual phase',teacher_state='installed PR31 only; future phase/energy choices never synthesized',profiles=12,english_views_per_utterance=[16000,48000],source_minutes=sum(r['seconds'] for r in manifest if r['split']=='train')/60,augmented_training_minutes=sum(r['seconds'] for r in manifest if r['split']=='train')/60*12+sum(r['seconds'] for r in manifest if r['split']=='train' and r['language']=='en')/60*12,files=files,utterances=receipts,seconds=time.monotonic()-started,ffmpeg=subprocess.check_output(['ffmpeg','-version'],text=True))
 (a.output/'provenance.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(dict(status='COMPLETE',files=files,seconds=report['seconds'])),flush=True)
if __name__=='__main__':main()
