#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Schedule complete paired cases; reuse the sealed native scorer unchanged."""
import argparse,csv,hashlib,json,itertools,os,time
from pathlib import Path
from concurrent.futures import ProcessPoolExecutor
from functools import partial
from evaluate_plc_energy_study import job
ROOT=Path(__file__).resolve().parents[1]
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def tasks(selection):
 return [(r,b,d,m) for r in selection for b,d,m in itertools.product((12000,24000,32000),(10,20),(0,1))]
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('run',type=Path);p.add_argument('--shard',type=int,required=True);p.add_argument('--workers',type=int,default=8);a=p.parse_args();r=a.run;manifest=json.loads((r/'distributed-manifest.json').read_text());selection=json.loads((r/'validation-selection.json').read_text());binding=json.loads((r/'binding.json').read_text())
 assert sha(r/'validation-selection.json')==manifest['selection_sha256']
 for name,digest in binding['code_sha256'].items():assert sha(ROOT/'tools'/name)==digest,name
 for name,digest in binding['source_sha256'].items():assert sha(ROOT/name)==digest,name
 binaries={k:Path(v) for k,v in json.loads((r/'binaries.json').read_text()).items()};reference=json.loads((r/'gpu-validation-partial.json').read_text())
 assert {n:sha(p) for n,p in binaries.items()}==reference['binary_sha256']
 indices=manifest['shards'][str(a.shard)];alltasks=tasks(selection);out=r/f'shard-{a.shard}.csv';proof=r/f'shard-{a.shard}.json'
 if out.exists():p.error('fresh shard required')
 status=dict(status='INCOMPLETE',shard=a.shard,task_indices=indices,manifest_sha256=sha(r/'distributed-manifest.json'),selection_sha256=sha(r/'validation-selection.json'),binary_sha256=reference['binary_sha256'],scorer_sha256=sha(ROOT/'tools/evaluate_plc_energy_study.py'),pid=os.getpid(),started=time.time())
 proof.write_text(json.dumps(status,indent=2));count=0
 with out.open('w') as f,ProcessPoolExecutor(max_workers=a.workers) as pool:
  writer=None
  for i,rows in enumerate(pool.map(partial(job,binaries=binaries),[alltasks[j] for j in indices])):
   assert len(rows)==360
   if writer is None:writer=csv.DictWriter(f,fieldnames=list(rows[0]));writer.writeheader()
   writer.writerows(rows);f.flush();count+=len(rows);print(json.dumps(dict(shard=a.shard,completed=i+1,total=len(indices),rows=count)),flush=True)
 status.update(status='COMPLETE',rows=count,csv_sha256=sha(out),seconds=time.time()-status['started']);proof.write_text(json.dumps(status,indent=2));print(json.dumps(dict(shard=a.shard,status='COMPLETE',csv_sha256=status['csv_sha256'])),flush=True)
if __name__=='__main__':main()
