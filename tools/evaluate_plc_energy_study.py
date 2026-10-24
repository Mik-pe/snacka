#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""One paired factorial replay across all eight frozen heads and both controls."""
import argparse,csv,hashlib,json,subprocess,tempfile,warnings
from concurrent.futures import ProcessPoolExecutor
from functools import partial
from pathlib import Path
import numpy as np
from pystoi import stoi
from evaluate_loss import loss_frames
from evaluate_speech import noisy
from evaluate_plc_ablation import PATTERNS
from prepare_plc_energy_study import FILTER

def sha(b):return hashlib.sha256(b).hexdigest()
def magnitudes(values,n,rate):
 padded=np.pad(values,(n//2,n//2),mode='reflect');frames=np.lib.stride_tricks.sliding_window_view(padded,n)[::n//4]
 window=.5-.5*np.cos(2*np.pi*np.arange(n)/n);m=np.abs(np.fft.rfft(frames*window,axis=1))
 return m[:,:int(8000*n/48000)+1] if rate==16000 else m

def job(item,binaries):
 row,bitrate,duration,harmonic=item;source=Path(row['corpus_root'])/row['path'];assert sha(source.read_bytes())==row['flac_sha256']
 cmd=['ffmpeg','-v','error','-i',str(source),'-ac','1']
 if row['effective_rate']==16000 and row['native_rate']==48000:cmd+=['-af',FILTER]
 cmd+=['-ar','48000','-f','f32le','pipe:1'];pcm=np.clip(np.frombuffer(subprocess.check_output(cmd),dtype='<f4'),-1,1).astype('<f4');clean=pcm.astype(np.float64)
 samples=duration*48;frames=(np.arange(len(pcm))+120)//samples;offset=(np.arange(len(pcm))+120)%samples;outrows=[]
 with tempfile.TemporaryDirectory(prefix='snacka-energy-validation-') as tmp:
  root=Path(tmp);masks={}
  for pattern in PATTERNS:
   dropped=loss_frames(source.stem,duration,(len(pcm)+samples-1)//samples,pattern);p=root/(pattern+'.txt');p.write_text(''.join(f'{i}\n' for i in sorted(dropped)));masks[pattern]=(dropped,p)
  for condition in ('clean','noise20','noise10'):
   data=pcm.tobytes() if condition=='clean' else noisy(clean,source.stem,int(condition[5:]))[0];inputpath=root/'input.f32';inputpath.write_bytes(data);baseline={};target=None;statistics={};streams={}
   for name,binary in binaries.items():
    output=root/name;subprocess.run([str(binary),str(inputpath),str(output),str(bitrate),str(duration),str(int(harmonic)),*[str(v[1]) for v in masks.values()]],check=True,capture_output=True)
    ref=(output/'uninterrupted.baseline.f32').read_bytes();assert ref==(output/'uninterrupted.current.f32').read_bytes()
    if target is None:target=ref
    assert ref==target,'packet/received PCM differs'
    with (output/'band-metrics.csv').open() as f:stats=list(csv.DictReader(f))
    statistics[name]=stats
    for pattern in PATTERNS:
     raw=(output/(pattern+'.baseline.f32')).read_bytes()
     if name=='pr31':baseline[pattern]=raw
     assert raw==baseline[pattern],'classical PLC replay differs'
     streams[(name,pattern)]=np.fromfile(output/(pattern+'.current.f32'),dtype='<f4')
   reference=np.frombuffer(target,dtype='<f4').astype(np.float64)
   refmag={n:magnitudes(reference,n,row['effective_rate']) for n in (128,256,512)}
   for pattern,(dropped,_) in masks.items():
    lost=np.isin(frames,list(dropped));recovery=~lost & np.isin(frames-1,list(dropped)) & (offset<120);received=~lost & ~recovery
    implementations={'classical':np.frombuffer(baseline[pattern],dtype='<f4'),**{name:streams[(name,pattern)] for name in binaries}}
    for name,values in implementations.items():
     assert len(values)==len(reference) and np.isfinite(values).all() and (np.abs(values)<=1).all()
     assert np.array_equal(values[received],reference[received]),'unexpected error outside loss/recovery'
     stats=[r for r in statistics['pr31' if name=='classical' else name] if r['pattern']==pattern and r['implementation']==('baseline' if name=='classical' else 'current')];assert len(stats)==16
     supported=[r for r in stats if int(r['band'])<(13 if row['effective_rate']==16000 else 16)]
     errors=[]
     for n,tm in refmag.items():
      am=magnitudes(values.astype(np.float64),n,row['effective_rate']);scale=max(float(tm.mean()),1e-5);t=tm/scale;q=am/scale;errors.append(float(np.mean(np.abs(q-t))+np.mean(np.abs(np.log(q+.01)-np.log(t+.01)))))
     # Preserve the historical scorer's STOI call/RNG progression for ESTOI.
     np.random.seed(int.from_bytes(hashlib.sha256(f'snacka-quality-6716:{source.stem+condition}'.encode()).digest()[:4],'little'))
     with warnings.catch_warnings():
      warnings.simplefilter('error',UserWarning);st=float(stoi(clean,values.astype(np.float64),48000));est=float(stoi(clean,values.astype(np.float64),48000,extended=True))
     result=dict(clip=source.stem,source=row['source'],group_id=row['group_id'],language=row['language'],effective_rate=row['effective_rate'],native_rate=row['native_rate'],cell=row.get('cell',f"{row['language']}{row['effective_rate']//1000}k"),condition=condition,bitrate=bitrate,frame_ms=duration,harmonic=int(harmonic),pattern=pattern,implementation=name,lost_samples=int(lost.sum()),input_sha256=sha(data),pcm_sha256=sha(values.tobytes()),wave=float(np.sum((reference[lost]-values[lost].astype(np.float64))**2)),env=sum(float(r['log_energy_squared']) for r in supported),env_count=sum(float(r['count']) for r in supported),mr=float(np.mean(errors)),stoi=st,estoi=est)
     assert all(np.isfinite(result[k]) for k in ('wave','env','mr','stoi','estoi'));outrows.append(result)
 return outrows

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('selection',type=Path);p.add_argument('binaries',type=Path);p.add_argument('output',type=Path);p.add_argument('--workers',type=int,default=4);p.add_argument('--acceptance',action='store_true');a=p.parse_args()
 if a.output.exists():p.error('fresh output required')
 binaries={k:Path(v).resolve() for k,v in json.loads(a.binaries.read_text()).items()};assert next(iter(binaries))=='pr31'
 selection=json.loads(a.selection.read_text());assert len(selection)==(72 if a.acceptance else 36)
 tasks=[(r,rate,dur,mode) for r in selection for rate in (12000,24000,32000) for dur in (10,20) for mode in (0,1)]
 report=dict(status='INCOMPLETE',selection_sha256=sha(a.selection.read_bytes()),binary_sha256={k:sha(v.read_bytes()) for k,v in binaries.items()},tasks=len(tasks));a.output.with_suffix('.json').write_text(json.dumps(report)+'\n');count=0
 with a.output.open('w') as f,ProcessPoolExecutor(max_workers=a.workers) as pool:
  writer=None
  for i,rows in enumerate(pool.map(partial(job,binaries=binaries),tasks)):
   if writer is None:writer=csv.DictWriter(f,fieldnames=list(rows[0]));writer.writeheader()
   writer.writerows(rows);f.flush();count+=len(rows);print(json.dumps(dict(completed=i+1,total=len(tasks),rows=count)),flush=True)
 report.update(status='COMPLETE',rows=count,csv_sha256=sha(a.output.read_bytes()));a.output.with_suffix('.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
