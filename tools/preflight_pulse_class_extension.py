#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Shadow class-limit 127 for small dimensions; no production table changes."""
import argparse,json,math
from fractions import Fraction
from pathlib import Path
from generate_pulse_tables import tables,DIMENSIONS,TOTAL
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
 if a.output.exists():p.error('fresh output required')
 existing=tables();rows=[];summaries=[]
 # ln(2) > 1000/1443 by four positive terms of its atanh(1/3) series.
 assert 2*sum(Fraction(1,(2*j+1)*3**(2*j+1)) for j in range(4))>Fraction(1000,1443)
 for n in (20,40,80):
  old=existing['PULSE_COST'][DIMENSIONS.index(n)];previous=old[80];section=[]
  for k in range(81,128):
   v=[math.comb(n,j)*math.comb(k-1,j-1)*2**j for j in range(1,n+1)];total=sum(v);units=TOTAL-n;width=[1+x*units//total for x in v]
   order=sorted(range(n),key=lambda j:(v[j]*units%total,-j),reverse=True)
   for j in order[:TOTAL-sum(width)]:width[j]+=1
   h=1+max(((x*TOTAL+w-1)//w-1).bit_length() for x,w in zip(v,width));u=(math.comb(n+k-1,k)-1).bit_length()+n;previous=max(previous,min(u,h))
   rounding=Fraction(1443,1000)*(Fraction(TOTAL,2**23-TOTAL)+n*Fraction(n,2**23-n)+(k-1)*Fraction(k-1,2**23-(k-1)))
   assert sum(width)==TOTAL and min(width)>0 and rounding<1 and n<=80
   row=dict(dimensions=n,pulses=k,legacy_C=old[k],candidate_C=previous,certified_budget_reduction=old[k]-previous,H=h,interval_rounding_upper_bits=float(rounding));rows.append(row);section.append(row)
  summaries.append(dict(dimensions=n,improved_entries=sum(r['certified_budget_reduction']>0 for r in section),max_budget_reduction=max(r['certified_budget_reduction'] for r in section),K81=section[0],K127=section[-1]))
 data=dict(schema=1,scope='Exact table-only proposal: enable the existing class representation through K=127 only for D=20,40,80. No encoder, decoder, profile, table or model is changed.',production_changed=False,profile_negotiation_required=True,heap_or_position_array_growth_required=False,logical_added_table_bytes=47*((21+41+81)*2+7*4),cases=len(rows),summary=summaries,interpretation='Certified allocation-cost potential, not measured byte savings, occurrence frequency or audio improvement.',results=rows)
 header=dict(data);records=header.pop('results');text=json.dumps(header,indent=2)[:-2]+',\n  "results": [\n'+',\n'.join('    '+json.dumps(r,separators=(',',':')) for r in records)+'\n  ]\n}\n';a.output.write_text(text);print(json.dumps(dict(cases=len(rows),improved=sum(r['certified_budget_reduction']>0 for r in rows),max_reduction=max(r['certified_budget_reduction'] for r in rows),logical_added_table_bytes=data['logical_added_table_bytes'])))
if __name__=='__main__':main()
