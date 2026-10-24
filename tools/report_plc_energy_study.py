#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Descriptive full-factorial tables after locked selection, never choose a model."""
import argparse,csv,hashlib,json
from collections import defaultdict
from pathlib import Path
from select_plc_energy_study import ORDER,key,metrics
CELLS=('en16k','en48k','sv16k')
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('study',type=Path);p.add_argument('output',type=Path);a=p.parse_args();root=a.study
 decision=json.loads((root/'validation-decision.json').read_text());proof=json.loads((root/'validation.json').read_text());assert proof['status']=='COMPLETE' and proof['csv_sha256']==sha(root/'validation.csv')==decision['input_csv_sha256']
 groups=defaultdict(list)
 with (root/'validation.csv').open() as f:
  for r in csv.DictReader(f):groups[r['implementation']].append(r)
 for rows in groups.values():rows.sort(key=key)
 baseline={cell:metrics([r for r in groups['pr31'] if r['cell']==cell],[r for r in groups['classical'] if r['cell']==cell]) for cell in CELLS}
 contrasts=[]
 for phase in ('off','on'):
  for cell in CELLS:
   m=lambda name:decision['results'][name+'_'+phase]['metrics'][cell]['classical']
   r0,r1,e0,e1=map(m,('ratio_native','ratio_balanced','energy_native','energy_balanced'))
   for metric in ('wave_ratio','env_ratio','mr_ratio','estoi_delta'):
    contrasts.append(dict(phase=phase,cell=cell,metric=metric,bandwidth_on_ratio=r1[metric]-r0[metric],target_energy_on_native=e0[metric]-r0[metric],target_energy_on_balanced=e1[metric]-r1[metric],interaction=(e1[metric]-e0[metric])-(r1[metric]-r0[metric])))
 slices=[]
 for name in ORDER:
  for cell in CELLS:
   for condition in ('clean','noise20','noise10'):
    for pattern in ('periodic1','periodic3','periodic6','independent5','burst40','burst80'):
     subset=lambda rows:[r for r in rows if (r['cell'],r['condition'],r['pattern'])==(cell,condition,pattern)]
     slices.append(dict(candidate=name,cell=cell,condition=condition,pattern=pattern,vs_classical=metrics(subset(groups[name]),subset(groups['classical'])),vs_pr31=metrics(subset(groups[name]),subset(groups['pr31']))))
 fits=[]
 for fit in ('ratio_native','energy_native','ratio_balanced','energy_balanced'):
  q=json.loads((root/'models'/fit/'training-report.json').read_text());fits.append({k:q[k] for k in ('fit','model_kind','architecture','initial_sha256','updates','seconds','weights_sha256','binding_sha256','torch','device')})
 output=dict(schema=1,decision=decision,installed_pr31_vs_classical=baseline,contrasts=contrasts,noise_pattern_slices=slices,training=fits,training_seconds=sum(q['seconds'] for q in fits),validation_csv_sha256=proof['csv_sha256'],validation_cases_per_implementation=decision['rows_per_implementation'],validation_rows=proof['rows'],validation_binary_sha256=proof['binary_sha256'],independent_acceptance_opened=False,activation_status='PENDING native/independent gates' if decision['selected'] else 'RETAIN PR31: no validation admission',compute_units_consumed=0)
 a.output.write_text(json.dumps(output,indent=2,allow_nan=False)+'\n')
 lines=['## Locked validation results','',f"Fixed fits: four, each1920updates; total measured training time{output['training_seconds']:.3f}s on CPU. Every candidate has7776 native cases. Fresh controls and identical input/uninterrupted/classical PCM were checked in every paired replay.",'','| Application | Failed guards | Admitted |','|---|---:|---|']
 for name in ORDER:
  q=decision['results'][name];lines.append(f"| {name} | {len(q['failed_guards'])} | {'yes' if q['admitted'] else 'no'} |")
 lines+=['','The guard count includes aggregate cells, every cell×bitrate and the finer duration/mode ESTOI floor. A failed guard is a failed application; no score-based rescue.','',f"Validation-selected candidate: **{decision['selected'] or 'none'}**.",'','Error columns below show percent change versus classical PLC; negative is lower error. ESTOI is an absolute difference, higher is better. This is measured development-set behavior, not independent acceptance or a listening result.','', '| Application | Cell | Waveform error % | Envelope error % | MR-STFT error % | ESTOI delta |','|---|---|---:|---:|---:|---:|']
 for name in ['PR31',*ORDER]:
  for cell in CELLS:
   m=baseline[cell] if name=='PR31' else decision['results'][name]['metrics'][cell]['classical'];lines.append(f"| {name} | {cell} | {(m['wave_ratio']-1)*100:+.3f} | {(m['env_ratio']-1)*100:+.3f} | {(m['mr_ratio']-1)*100:+.3f} | {m['estoi_delta']:+.7f} |")
 lines+=['','Full per-bitrate guards, dual-reference results, phase-off/on differences, noise/pattern slices and2×2 contrasts are in the machine-readable results. The contrasts describe this fixed seed, update budget and teacher; they do not estimate variability across retrainings.','']
 a.output.with_suffix('.md').write_text('\n'.join(lines))
if __name__=='__main__':main()
