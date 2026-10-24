#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Frozen guard-first selection; no acceptance loading and no fallback winner."""
import argparse,csv,hashlib,json,itertools
from collections import defaultdict
from pathlib import Path
import numpy as np
ORDER=['ratio_native_off','ratio_native_on','energy_native_off','energy_native_on','ratio_balanced_off','ratio_balanced_on','energy_balanced_off','energy_balanced_on']
KEYS=('clip','cell','condition','bitrate','frame_ms','harmonic','pattern')
def key(r):return tuple(r[k] for k in KEYS)
def metrics(rows,ref):
 assert len(rows)==len(ref) and all(key(a)==key(b) and a['input_sha256']==b['input_sha256'] for a,b in zip(rows,ref))
 result={k+'_ratio':float(sum(float(r[k]) for r in rows)/sum(float(r[k]) for r in ref)) for k in ('wave','env','mr')}
 result['env_ratio']*=sum(float(r['env_count']) for r in ref)/sum(float(r['env_count']) for r in rows)
 result['estoi_delta']=float(np.mean([float(a['estoi'])-float(b['estoi']) for a,b in zip(rows,ref)]));return result

def summarize(groups,name):
 out={};fail=[]
 cells=('en16k','en48k','sv16k')
 for cell,rate in itertools.product(cells,(None,'12000','24000','32000')):
  label=cell+(':'+rate if rate else '');subset=lambda rows:[r for r in rows if r['cell']==cell and (rate is None or r['bitrate']==rate)]
  out[label]={}
  for ref in ('classical','pr31'):
   m=metrics(subset(groups[name]),subset(groups[ref]));out[label][ref]=m
   for metric,value in m.items():
    passed=value>=0 if metric=='estoi_delta' else value<1 if metric=='wave_ratio' and ref=='classical' else value<=1
    if not passed:fail.append(dict(slice=label,reference=ref,metric=metric,value=value))
 for cell,rate,dur,mode in itertools.product(cells,('12000','24000','32000'),('10','20'),('0','1')):
  subset=lambda rows:[r for r in rows if (r['cell'],r['bitrate'],r['frame_ms'],r['harmonic'])==(cell,rate,dur,mode)]
  for ref in ('classical','pr31'):
   m=metrics(subset(groups[name]),subset(groups[ref]))
   if m['estoi_delta']<-.001:fail.append(dict(slice=f'{cell}:{rate}:{dur}:{mode}',reference=ref,metric='fine_estoi_delta',value=m['estoi_delta']))
 return dict(admitted=not fail,failed_guards=fail,metrics=out)
def choose(results):
 admitted=[n for n in ORDER if results[n]['admitted']]
 def ranking(n):
  m=results[n]['metrics'];cells=('en16k','en48k','sv16k')
  return (-min(m[c]['pr31']['estoi_delta'] for c in cells),sum(m[c]['pr31']['env_ratio'] for c in cells),sum(m[c]['pr31']['mr_ratio'] for c in cells),ORDER.index(n))
 return min(admitted,key=ranking) if admitted else None

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('csv',type=Path);p.add_argument('selection',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
 if a.output.exists():p.error('immutable selection; fresh output required')
 proof=json.loads(a.csv.with_suffix('.json').read_text());assert proof['status']=='COMPLETE' and proof['csv_sha256']==hashlib.sha256(a.csv.read_bytes()).hexdigest()
 selection=json.loads(a.selection.read_text());expected=set()
 for r,condition,rate,dur,mode,pattern in itertools.product(selection,('clean','noise20','noise10'),('12000','24000','32000'),('10','20'),('0','1'),('periodic1','periodic3','periodic6','independent5','burst40','burst80')):
  expected.add((Path(r['path']).stem,f"{r['language']}{r['effective_rate']//1000}k",condition,rate,dur,mode,pattern))
 groups=defaultdict(list)
 with a.csv.open() as f:
  for r in csv.DictReader(f):
   assert all(np.isfinite(float(r[k])) for k in ('wave','env','mr','estoi'));groups[r['implementation']].append(r)
 assert set(groups)==set(ORDER+['classical','pr31'])
 for n,rows in groups.items():
  assert len(rows)==len(expected) and {key(r) for r in rows}==expected,n
  rows.sort(key=key)
 results={n:summarize(groups,n) for n in ORDER};winner=choose(results)
 result=dict(schema=1,selected=winner,results=results,rows_per_implementation=len(expected),cells=['en16k','en48k','sv16k'],references=['classical','pr31'],phase_off_on={n:{cell:{k:results[n+'_on']['metrics'][cell]['classical'][k]-results[n+'_off']['metrics'][cell]['classical'][k] for k in ('wave_ratio','env_ratio','mr_ratio','estoi_delta')} for cell in ('en16k','en48k','sv16k')} for n in ('ratio_native','energy_native','ratio_balanced','energy_balanced')},test_audio_seen=False,rule='core guards plus per-cell/bitrate amendment; guards before ranking; no fallback',input_csv_sha256=proof['csv_sha256'])
 a.output.write_text(json.dumps(result,indent=2,allow_nan=False)+'\n');print(json.dumps(dict(selected=winner,failed_guards={n:len(v['failed_guards']) for n,v in results.items()})))
if __name__=='__main__':main()
