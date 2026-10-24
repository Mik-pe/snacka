#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Versioned raw-power teacher export, exact within-speaker bandwidth factor."""
import argparse, hashlib, json, subprocess, tempfile
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import numpy as np
SALT='snacka-target-energy-20261023:bandwidth:'
FILTER='aresample=16000:resampler=soxr:precision=28,aresample=48000:resampler=soxr:precision=28'
def sha(b):return hashlib.sha256(b).hexdigest()
def job(item,corpus,binary):
 row,bw=item;source=corpus/row['path'];assert sha(source.read_bytes())==row['flac_sha256']
 cmd=['ffmpeg','-v','error','-i',str(source),'-ac','1']
 if bw==16000 and row['native_rate']==48000:cmd+=['-af',FILTER]
 cmd+=['-ar','48000','-f','f32le','pipe:1'];source_pcm=subprocess.check_output(cmd)
 key=int(sha(('snacka-plc-v1:'+row['path']).encode())[:16],16)
 rate=(12000,24000,32000)[key%3];duration=(10,20)[key//3%2];harmonic=key//6%2;condition=key//12%3;gain_db=(0,-12,-24)[key//36%3]
 pcm=np.frombuffer(source_pcm,dtype='<f4').copy()
 if condition:
  noise=np.random.default_rng(key).normal(size=len(pcm));power=np.mean(pcm.astype(np.float64)**2)
  noise*=np.sqrt(power/(np.mean(noise**2)*10**((20 if condition==1 else 10)/10)));pcm+=noise.astype(np.float32)
 pcm=np.clip(pcm*np.float32(10**(gain_db/20)),-1,1).astype('<f4').tobytes()
 with tempfile.TemporaryDirectory(prefix='snacka-energy-teacher-') as tmp:
  root=Path(tmp);(root/'in.f32').write_bytes(pcm)
  subprocess.run([str(binary),str(root/'in.f32'),str(root/'labels'),str(root/'audit'),str(rate),str(duration),str(harmonic),'24',str(key%24)],check=True,capture_output=True)
  raw=(root/'labels').read_bytes();assert raw[:8]==b'SPLE\x02\0\0\0' and (len(raw)-8)%520==0
  values=np.frombuffer(raw[8:],dtype='<f4').reshape(-1,130).copy();assert np.isfinite(values).all() and np.isin(values[:,129],[0,1]).all()
  selected=values[values[:,129]==1]
  audit_sha=sha((root/'audit').read_bytes())
 receipt={**{k:v for k,v in row.items() if k!='corpus_root'},'effective_rate':bw,'records':len(values),'eligible':len(selected),'source_pcm_sha256':sha(source_pcm),'input_sha256':sha(pcm),'teacher_sha256':sha(raw),'classical_audit_sha256':audit_sha,'bitrate':rate,'duration_ms':duration,'harmonic':harmonic,'condition':condition,'gain_db':gain_db}
 return selected,receipt

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('corpus',type=Path);p.add_argument('output',type=Path);p.add_argument('--binary',type=Path,required=True);p.add_argument('--workers',type=int,default=4);a=p.parse_args()
 if a.output.exists():p.error('fresh output required')
 a.output.mkdir(parents=True);manifest=json.loads((a.corpus/'training-selection.json').read_text());assert len(manifest)==344
 groups=defaultdict(list)
 for r in manifest:
  assert r['split'] in ('train','validation')
  if r['language']=='en' and r['split']=='train':groups[r['group_id']].append(r)
 low=set()
 for speaker,rows in groups.items():
  assert len(rows)==8
  low.update(r['path'] for r in sorted(rows,key=lambda r:sha((SALT+r['path']).encode()))[:4])
 assert len(groups)==24 and len(low)==96
 items=[]
 for row in manifest:
  items.append((row,row['native_rate']))
  if row['language']=='en' and (row['path'] in low or row['split']=='validation'):items.append((row,16000))
 shards=defaultdict(list);receipts=[];val=[]
 with ThreadPoolExecutor(max_workers=a.workers) as pool:
  for index,(values,row) in enumerate(pool.map(lambda i:job(i,a.corpus/'corpus-registered',a.binary.resolve()),items)):
   rid=len(receipts);receipts.append(row);mask=np.zeros((len(values),16),np.float32);mask[:,:13 if row['effective_rate']==16000 else 16]=1
   parts=dict(x=values[:,:80],last=values[:,80:96],target=values[:,96:112],predicted=values[:,112:128],phase=values[:,128],mask=mask,record=np.full(len(values),rid,np.int32))
   lang=0 if row['language']=='en' else 1
   if row['split']=='train':
    if row['effective_rate']==row['native_rate']:shards[('native',lang)].append(parts)
    if row['language']=='sv' or (row['path'] in low)==(row['effective_rate']==16000):shards[('balanced',lang)].append(parts)
   else:
    val.append({**row,'corpus_root':str(a.corpus/'corpus-registered')})
    shards[('validation',lang)].append(parts)
   if index%20==0:print(json.dumps(dict(completed=index+1,total=len(items))),flush=True)
 files=[]
 for (factor,lang),parts in sorted(shards.items()):
  data={k:np.concatenate([v[k] for v in parts]) for k in parts[0]};path=a.output/f'{factor}-{lang}.npz';np.savez_compressed(path,**data);files.append(dict(name=path.name,rows=len(data['x']),sha256=sha(path.read_bytes())))
 report=dict(schema=2,format='SPLE2-raw-log-mean-power',eps=1e-12,source_manifest_sha256=sha((a.corpus/'training-selection.json').read_bytes()),binary_sha256=sha(a.binary.read_bytes()),ffmpeg_version=subprocess.check_output(['ffmpeg','-version'],text=True),ffmpeg_sha256=sha(Path(subprocess.check_output(['which','ffmpeg'],text=True).strip()).resolve().read_bytes()),filter=FILTER,lowpass_paths=sorted(low),files=files,utterances=receipts,teacher='unchanged PhaseWide; classical candidate0 history only')
 (a.output/'teacher-provenance.json').write_text(json.dumps(report,indent=2)+'\n');(a.output/'validation-selection.json').write_text(json.dumps(val,indent=2)+'\n')
 print(json.dumps(files),flush=True)
if __name__=='__main__':main()
