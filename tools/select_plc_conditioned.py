#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Guard-first selection of the frozen six-fit/three-strength pool."""
import argparse,csv,hashlib,itertools,json
from collections import defaultdict
from pathlib import Path
import numpy as np
from select_plc_energy_study import key,metrics,summarize
FITS=('history_wide','history_aligned','conditioned_wide','conditioned_aligned','history_damping','conditioned_damping')
ORDER=[f'{n}_s{s}' for n in FITS for s in ('025','050','100')]
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('csv',type=Path);p.add_argument('selection',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
 if a.output.exists():p.error('fresh immutable output required')
 proof=json.loads(a.csv.with_suffix('.json').read_text());assert proof['status']=='COMPLETE' and proof['csv_sha256']==hashlib.sha256(a.csv.read_bytes()).hexdigest()
 selection=json.loads(a.selection.read_text());assert len(selection)==36
 expected={(Path(r['path']).stem,f"{r['language']}{r['effective_rate']//1000}k",condition,rate,dur,mode,pattern) for r,condition,rate,dur,mode,pattern in itertools.product(selection,('clean','noise20','noise10'),('12000','24000','32000'),('10','20'),('0','1'),('periodic1','periodic3','periodic6','independent5','burst40','burst80'))}
 assert len(expected)==7776
 groups=defaultdict(list)
 with a.csv.open() as f:
  for r in csv.DictReader(f):
   assert all(np.isfinite(float(r[k])) for k in ('wave','env','mr','estoi'));groups[r['implementation']].append(r)
 assert set(groups)==set(ORDER+['pr31','classical'])
 for name,rows in groups.items():
  assert len(rows)==len(expected) and {key(r) for r in rows}==expected,name
  rows.sort(key=key)
 results={}
 for name in ORDER:
  result=summarize(groups,name)
  minimum=min(result['metrics'][cell]['pr31']['estoi_delta'] for cell in ('en16k','en48k','sv16k'))
  if minimum<=0:result['failed_guards'].append(dict(slice='minimum aggregate cell',reference='pr31',metric='strict_positive_estoi',value=minimum))
  result['admitted']=not result['failed_guards'];results[name]=result
 def rank(n):
  m=results[n]['metrics'];cells=('en16k','en48k','sv16k')
  return(-min(m[c]['pr31']['estoi_delta'] for c in cells),sum(m[c]['pr31']['env_ratio'] for c in cells),sum(m[c]['pr31']['mr_ratio'] for c in cells),ORDER.index(n))
 admitted=[n for n in ORDER if results[n]['admitted']];winner=min(admitted,key=rank) if admitted else None
 out=dict(schema=1,selected=winner,results=results,rows_per_implementation=len(expected),input_csv_sha256=proof['csv_sha256'],test_audio_seen=False,no_fallback=True)
 a.output.write_text(json.dumps(out,indent=2,allow_nan=False)+'\n')
 print(json.dumps(dict(selected=winner,failed_guards={n:len(v['failed_guards']) for n,v in results.items()})))
if __name__=='__main__':main()
