#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Resume only this immutable native run, retaining complete paired case blocks.

No old-experiment metric cache is accepted. The input selection, executables and
frozen scorer must match. Every retained case needs all10 implementations,
3conditions and6patterns; a torn final block is discarded and replayed.
"""
import argparse,csv,hashlib,json,itertools,os,sys,tempfile
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor
from functools import partial
from pathlib import Path
import numpy as np
from evaluate_plc_energy_study import job
from select_plc_energy_study import ORDER

def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def case(r):return (r['clip'],r['cell'],str(r['bitrate']),str(r['frame_ms']),str(r['harmonic']))
def complete_blocks(rows,binaries):
 expected=set(itertools.product(['classical',*binaries],('clean','noise20','noise10'),('periodic1','periodic3','periodic6','independent5','burst40','burst80')))
 blocks=defaultdict(list)
 for r in rows:blocks[case(r)].append(r)
 good={};discarded=0
 for k,values in blocks.items():
  observed={(r.get('implementation'),r.get('condition'),r.get('pattern')) for r in values}
  if len(values)!=len(expected) or observed!=expected:discarded+=len(values);continue
  for r in values:assert all(np.isfinite(float(r[m])) for m in ('wave','env','mr','estoi')) and len(r['input_sha256'])==64
  for condition in ('clean','noise20','noise10'):assert len({r['input_sha256'] for r in values if r['condition']==condition})==1
  good[k]=values
 return good,discarded

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('selection',type=Path);p.add_argument('binaries',type=Path);p.add_argument('csv',type=Path);p.add_argument('--binding',type=Path,required=True);p.add_argument('--workers',type=int,default=10);a=p.parse_args()
 root=Path(__file__).resolve().parents[1];binding=json.loads(a.binding.read_text());assert sha(root/'tools/evaluate_plc_energy_study.py')==binding['source_sha256']['tools/evaluate_plc_energy_study.py']
 proof=json.loads(a.csv.with_suffix('.json').read_text());assert proof['status']=='INCOMPLETE' and proof['selection_sha256']==sha(a.selection)
 binaries={k:Path(v).resolve() for k,v in json.loads(a.binaries.read_text()).items()};assert list(binaries)==['pr31',*ORDER]
 assert proof['binary_sha256']=={k:sha(v) for k,v in binaries.items()}
 oldsha=sha(a.csv)
 with a.csv.open() as f:reader=csv.DictReader(f);rows=list(reader);fields=reader.fieldnames
 good,discarded=complete_blocks(rows,binaries);selection=json.loads(a.selection.read_text());assert len(selection)==36
 tasks=[(r,rate,dur,mode) for r in selection for rate in (12000,24000,32000) for dur in (10,20) for mode in (0,1)]
 taskkey=lambda t:(Path(t[0]['path']).stem,f"{t[0]['language']}{t[0]['effective_rate']//1000}k",str(t[1]),str(t[2]),str(t[3]))
 assert set(good)<={taskkey(t) for t in tasks};pending=[t for t in tasks if taskkey(t) not in good]
 checkpoint=a.csv.with_name(a.csv.stem+'-first-checkpoint.csv');assert not checkpoint.exists();a.csv.rename(checkpoint)
 proof['resume']=dict(checkpoint_sha256=oldsha,complete_retained_tasks=len(good),discarded_torn_rows=discarded,new_workers=a.workers,scorer_sha256=sha(root/'tools/evaluate_plc_energy_study.py'),runner_sha256=sha(Path(__file__)),semantics='same run and frozen scorer/binaries; no old-experiment metric reuse')
 a.csv.with_suffix('.json').write_text(json.dumps(proof,indent=2)+'\n');count=0;completed=len(good)
 with a.csv.open('w') as f,ProcessPoolExecutor(max_workers=a.workers) as pool:
  w=csv.DictWriter(f,fieldnames=fields);w.writeheader()
  for t in tasks:
   if taskkey(t) in good:w.writerows(good[taskkey(t)]);count+=len(good[taskkey(t)])
  f.flush()
  for result in pool.map(partial(job,binaries=binaries),pending):
   w.writerows(result);f.flush();count+=len(result);completed+=1;print(json.dumps(dict(completed=completed,total=len(tasks),rows=count)),flush=True)
 assert count==77760 and completed==432
 proof.update(status='COMPLETE',rows=count,csv_sha256=sha(a.csv));a.csv.with_suffix('.json').write_text(json.dumps(proof,indent=2)+'\n')
if __name__=='__main__':main()
