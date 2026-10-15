from pathlib import Path
import json,subprocess,hashlib,time
A=Path('/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015')
old=Path('/tmp/snacka-opus-goal-quantize-5fc1b43/equivalence/rows.jsonl')
rows=[json.loads(x) for x in old.read_text().splitlines()]
output=A/'trace-proof';output.mkdir(exist_ok=True)
summary={'source_commit':'8f723ba1f35c053e1cc840f38a4b20446a127dde','baseline_commit':'cca87dd757d96623b91bff81d34a021e5a3e2792','cases':len(rows),'traces':0,'whole_success_traces':0,'whole_success_packets':0,'prefix_traces_before_old_refusal':0,'prefix_packets':0,'baseline_refusal_traces':0,'candidate_refusal_traces':0,'candidate_full_packets':0,'exact_whole_success_traces':0,'exact_prefix_traces':0,'regressions':[],'scope':'924 historical geometries/signals; no attenuation/retry. Exact entire baseline-success traces, otherwise exact packets/metadata/ordinary-PLC-FEC output only before first baseline refusal. Candidate full traces are separately validated, not compared after old refusals.'}
def run(role,args):
 p=subprocess.run([str(A/f'prefix-probe-{role}'),*args],capture_output=True,text=True)
 if p.returncode: raise RuntimeError({'role':role,'args':args,'code':p.returncode,'stderr':p.stderr})
 return json.loads(p.stdout)
def command(c):
 a=[c['mode'],c['band'],c['ms'],str(c['channels']),c['signal'],str(c['frames'])]
 if 'input' in c:a+=['--input',c['input']]
 if 'bitrate' in c:a+=['--bitrate',str(c['bitrate'])]
 return a
with (output/'rows.jsonl').open('w') as f:
 for i,oldrow in enumerate(rows):
  c=oldrow['case']; args=command(c);b=run('baseline',args)
  limits=[x['trace']['accepted_packets'] for x in b['traces']]
  p=run('sparse',args+['--limits',','.join(map(str,limits))])
  has_refusal=any(x['trace']['first_refusal'] is not None for x in b['traces'])
  full=run('sparse',args) if has_refusal else p
  assert len(b['traces'])==len(p['traces'])==len(full['traces'])
  results=[]
  for before,prefix,after in zip(b['traces'],p['traces'],full['traces']):
   bt=before['trace'];pt=prefix['trace'];at=after['trace'];ref=bt['first_refusal'];n=bt['accepted_packets']
   bc={k:v for k,v in bt.items() if k!='first_refusal'};pc={k:v for k,v in pt.items() if k!='first_refusal'}
   exact=bc==pc and pt['first_refusal'] is None
   category='whole_success' if ref is None else 'prefix_before_old_refusal'
   summary['traces']+=1;summary['candidate_full_packets']+=at['accepted_packets'];summary['candidate_refusal_traces']+=at['first_refusal'] is not None
   if ref is None:
    summary['whole_success_traces']+=1;summary['whole_success_packets']+=n;summary['exact_whole_success_traces']+=exact
   else:
    summary['baseline_refusal_traces']+=1;summary['prefix_traces_before_old_refusal']+=1;summary['prefix_packets']+=n;summary['exact_prefix_traces']+=exact
   if not exact:summary['regressions'].append({'case':c,'fec_enabled':before['fec_enabled'],'reset_half':bt['reset_half'],'different_fields':[k for k in bc if bc[k]!=pc[k]],'candidate_refusal':pt['first_refusal']})
   if ref is None and at['first_refusal'] is not None: raise AssertionError('new refusal in baseline successful trace')
   results.append({'category':category,'exact':exact,'baseline':before,'candidate_prefix':prefix,'candidate_full':after})
  f.write(json.dumps({'case':c,'traces':results},separators=(',',':'))+'\n');f.flush()
  if i%20==0 or i+1==len(rows):
   (output/'progress.json').write_text(json.dumps({'completed_cases':i+1,**summary},indent=2)+'\n')
   print(f"{i+1}/{len(rows)} whole={summary['whole_success_traces']} prefixes={summary['prefix_traces_before_old_refusal']} regressions={len(summary['regressions'])}",flush=True)
summary['all_exact']=not summary['regressions']
summary['rows_sha256']=hashlib.sha256((output/'rows.jsonl').read_bytes()).hexdigest()
summary['probe_sha256']=hashlib.sha256((A/'prefix-probe.rs').read_bytes()).hexdigest()
summary['runner_sha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
(output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
