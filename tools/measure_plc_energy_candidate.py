#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Interleaved M5 online call gate vs PR31 on known validation speech."""
import argparse,csv,hashlib,json,platform,subprocess,tempfile,itertools
from pathlib import Path
from collections import defaultdict
import numpy as np
from prepare_plc_energy_study import FILTER

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('selection',type=Path);p.add_argument('candidate',type=Path);p.add_argument('pr31',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
 if a.output.exists():p.error('fresh timing output required')
 a.output.mkdir();rows=[]
 assert platform.machine()=='arm64'
 with tempfile.TemporaryDirectory(prefix='snacka-energy-cpu-') as tmp:
  root=Path(tmp)
  for index,row in enumerate(json.loads(a.selection.read_text())):
   source=Path(row['corpus_root'])/row['path'];assert hashlib.sha256(source.read_bytes()).hexdigest()==row['flac_sha256'];cmd=['ffmpeg','-v','error','-i',str(source),'-ac','1']
   if row['effective_rate']==16000 and row['native_rate']==48000:cmd+=['-af',FILTER]
   raw=subprocess.check_output(cmd+['-ar','48000','-f','f32le','pipe:1']);pcm=root/'input.f32';pcm.write_bytes(raw)
   for rate,dur,mode in itertools.product((12000,24000,32000),(10,20),(0,1)):
    for block in range(-1,8):
     order=[('pr31',a.pr31),('candidate',a.candidate)]
     if block%2:order.reverse()
     for label,binary in order:
      out=root/'time.csv';subprocess.run([str(binary.resolve()),str(pcm),str(out),str(rate),str(dur),str(mode)],check=True,capture_output=True)
      if block<0:continue
      with out.open() as f:
       for r in csv.DictReader(f):rows.append(dict(clip=source.stem,effective_rate=row['effective_rate'],bitrate=rate,frame_ms=dur,harmonic=mode,block=block,implementation=label,**r))
   print(json.dumps(dict(completed=index+1)),flush=True)
 raw=a.output/'calls.csv'
 with raw.open('w') as f:w=csv.DictWriter(f,fieldnames=list(rows[0]));w.writeheader();w.writerows(rows)
 results=[];passed=True
 for rate,dur,mode,op in itertools.product((12000,24000,32000),(10,20),(0,1),('received','first_loss','continued_loss','recovery')):
  subset=[r for r in rows if (r['bitrate'],r['frame_ms'],r['harmonic'],r['operation'])==(rate,dur,mode,op)]
  q={label:np.quantile([int(r['elapsed_ns'])/1000 for r in subset if r['implementation']==label],[.5,.95,.99,1]).tolist() for label in ('pr31','candidate')}
  deltas=[]
  for b in range(8):deltas.append(float(np.median([int(r['elapsed_ns'])/1000 for r in subset if r['implementation']=='candidate' and r['block']==b])-np.median([int(r['elapsed_ns'])/1000 for r in subset if r['implementation']=='pr31' and r['block']==b])))
  ok=q['candidate'][0]<=q['pr31'][0] and q['candidate'][0]<=25 if op=='first_loss' else True
  passed=passed and ok;results.append(dict(bitrate=rate,frame_ms=dur,harmonic=mode,operation=op,quantiles_us=q,paired_block_median_deltas_us=deltas,passed=ok))
 report=dict(status='COMPLETE',passed=passed,rule='each bitrate/duration/mode first-loss median <=PR31 and <=25us',platform=platform.platform(),machine=platform.machine(),rows=len(rows),results=results,csv_sha256=hashlib.sha256(raw.read_bytes()).hexdigest(),candidate_sha256=hashlib.sha256(a.candidate.read_bytes()).hexdigest(),pr31_sha256=hashlib.sha256(a.pr31.read_bytes()).hexdigest(),note='actual online calls; capture and metric loops excluded; wall timings include scheduling; no WCET guarantee')
 (a.output/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(dict(passed=passed,rows=len(rows))))
if __name__=='__main__':main()
