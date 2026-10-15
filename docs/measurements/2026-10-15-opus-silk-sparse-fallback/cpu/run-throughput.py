from pathlib import Path
import sys,json,subprocess,statistics,math,hashlib,datetime,os
R=Path('/tmp/snacka-opus-silk-sparse-cpu-20261015')
CASES=['nb10m12','nb10s24','mb20m16','wb10m24','wb20m12','wb20s48','wb20m24fec','wb60s48fec','hybrid20m32fec','celt20m32']
assert sys.argv[1:]==['--measure'], 'Explicit authorized measurement mode required'
manifest=json.loads((R/'manifest.json').read_text())
for name,digest in manifest['measurement_files'].items():
 p=R/name;assert hashlib.sha256(p.read_bytes()).hexdigest()==digest,(name,'source/artifact changed')
O=R/'throughput-results';O.mkdir(exist_ok=True)
rows=[];effects=[]
checks={r['case']:r['input_fnv64'] for r in (json.loads(x) for x in (R/'throughput-final-verify-candidate.jsonl').read_text().splitlines())}
env=os.environ.copy();env.pop('SPARSE_CPU_DUMP_INPUTS',None)
with (O/'rows.jsonl').open('x') as f:
 for rep in range(3):
  for case in CASES:
   group=[]
   for position,role in enumerate(['baseline','candidate','candidate','baseline']):
    cmd=[str(R/f'throughput-{role}'),'--measure',case]
    p=subprocess.run(cmd,capture_output=True,text=True,env=env)
    tag=f'{rep}-{case}-{position}-{role}';(O/f'{tag}.stdout').write_text(p.stdout);(O/f'{tag}.stderr').write_text(p.stderr)
    if p.returncode:raise RuntimeError((cmd,p.returncode,p.stderr))
    row=json.loads(p.stdout);assert row['input_fnv64']==checks[case];row.update(repetition=rep,position=position,role=role,command=cmd)
    f.write(json.dumps(row,separators=(',',':'))+'\n');f.flush();rows.append(row);group.append(row)
   norm=lambda role:math.exp(statistics.mean(math.log(r['timed']['paired_normalization_ratio'][0]) for r in group if r['role']==role))
   effect={'case':case,'repetition':rep,'paired_normalized_change_percent':(norm('candidate')/norm('baseline')-1)*100,'all_native_wire_matched':all(r['native_wire_match']=='MATCHED' for r in group),'native_fec_workload_equal':all(r['fec_availability_equal'] for r in group)}
   effects.append(effect);(O/'progress.json').write_text(json.dumps({'completed_quartets':len(effects),'total_quartets':30,'effects':effects},indent=2)+'\n')
   print(json.dumps(effect),flush=True)
summary={'source_candidate':manifest['source_candidate'],'source_baseline':manifest['source_baseline'],'method':'3ABBAquartets/case; pairednative normalization controls CPU drift; batch percentiles are not individual callback tails','effects':effects,'by_case':{}}
for case in CASES:
 data=[r['paired_normalized_change_percent'] for r in effects if r['case']==case]
 summary['by_case'][case]={'replicate_changes_percent':data,'median_change_percent':statistics.median(data),'min_change_percent':min(data),'max_change_percent':max(data),'native_wire_matched':all(r['all_native_wire_matched'] for r in effects if r['case']==case),'native_fec_workload_equal':all(r['native_fec_workload_equal'] for r in effects if r['case']==case)}
summary['rows_sha256']=hashlib.sha256((O/'rows.jsonl').read_bytes()).hexdigest();(O/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary,indent=2))
