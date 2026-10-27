#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Exact full-support class-CDF budget preflight, independent of speech data."""
import argparse,hashlib,json,math
from pathlib import Path
from generate_pulse_tables import tables,DIMENSIONS,TOTAL
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path);a=p.parse_args();t=tables();rows=[]
 for ri,n in enumerate(DIMENSIONS):
  for k in range(1,81):
   counts=[math.comb(n,j)*math.comb(k-1,j-1)*(1<<j) for j in range(1,min(n,k)+1)]
   start=t['OFFSETS'][ri][k];cdf=t['BOUNDARIES'][start:start+len(counts)+1];widths=[b-a for a,b in zip(cdf,cdf[1:])]
   baseline=1+max(((v*TOTAL+w-1)//w-1).bit_length() for v,w in zip(counts,widths))
   b=(sum(counts)-1).bit_length()
   required=lambda bits:[(v*TOTAL+(1<<bits)-1)//(1<<bits) for v in counts]
   while sum(required(b))>TOTAL:b+=1
   witness=required(b)
   assert sum(witness)<=TOTAL and sum(required(b-1))>TOTAL
   assert all(v*TOTAL<=w*(1<<b) for v,w in zip(counts,witness))
   optimum=1+b;assert optimum<=baseline
   rows.append(dict(dimensions=n,pulses=k,classes=len(counts),baseline_H=baseline,optimal_H=optimum,required_width_sum=sum(witness),infeasible_previous_sum=sum(required(b-1)),uses_classes=bool(t['MASKS'][ri][k//64]&(1<<(k%64)))))
 root=Path(__file__).resolve().parents[1]
 d=dict(schema=1,scope='All 560 supported D,K class priors, full class support and Q=65535. Worst-case certified H only; no average bitrate or quality forecast.',definition='H=1+max ceil(log2(V*Q/w)); feasible B=H-1 iff sum ceil(V*Q/2^B)<=Q.',cases=len(rows),existing_minimax_count=sum(r['baseline_H']==r['optimal_H'] for r in rows),improved_H_count=sum(r['optimal_H']<r['baseline_H'] for r in rows),source_sha256={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in ('src/pulse.rs','src/pulse_classes.rs','docs/bitstream.md','tools/generate_pulse_tables.py')},production_changed=False,compute_units=0,results=rows)
 text=json.dumps(d,indent=2)+'\n'
 if a.output:
  if a.output.exists():p.error('fresh output required')
  a.output.write_text(text)
 else:print(text,end='')
 print(json.dumps({k:d[k] for k in ('cases','existing_minimax_count','improved_H_count','compute_units')}))
if __name__=='__main__':main()
