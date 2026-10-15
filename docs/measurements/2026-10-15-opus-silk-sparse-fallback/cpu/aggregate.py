from pathlib import Path
import csv,json,hashlib,math,statistics
R=Path('/tmp/snacka-opus-silk-sparse-cpu-20261015');C=Path('/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015/controls/callback-latency')
h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
m=json.loads((R/'manifest.json').read_text());integrity={'manifest_sha256':h(R/'manifest.json'),'all_frozen_measurement_files_unchanged':all(h(R/name)==sha for name,sha in m['measurement_files'].items()),'native_library_unchanged':h(m['native_reference']['path'])==m['native_reference']['sha256']}
rows=[json.loads(x) for x in (R/'throughput-results/rows.jsonl').read_text().splitlines()];assert len(rows)==120
checks={r['case']:r['input_fnv64'] for r in (json.loads(x) for x in (R/'throughput-final-verify-candidate.jsonl').read_text().splitlines())}
assert all(r['input_fnv64']==checks[r['case']] for r in rows)
assert all(r['timed']['paired_owned_native_ratio'] is None for r in rows if not r['fec_availability_equal'] or r['native_wire_match']!='MATCHED')
t=json.loads((R/'throughput-results/summary.json').read_text())
for case,s in t['by_case'].items():
 for role in ['baseline','candidate']:
  selected=[r for r in rows if r['case']==case and r['role']==role]
  s[role+'_owned_p50_ns_across_processes_median']=statistics.median(r['timed']['owned_ns'][0] for r in selected)
  s[role+'_normalized_p50_geomean']=math.exp(statistics.mean(math.log(r['timed']['paired_normalization_ratio'][0]) for r in selected))
  s[role+'_absolute_native_parity_p50_geomean']=math.exp(statistics.mean(math.log(r['timed']['paired_owned_native_ratio'][0]) for r in selected)) if all(r['timed']['paired_owned_native_ratio'] for r in selected) else None
report={'source_candidate':m['source_candidate'],'source_baseline':m['source_baseline'],'integrity':integrity,'throughput':t,'callbacks':{}}
def q(v,p):
 a=sorted(v);return a[max(0,math.ceil(len(a)*p)-1)] if a else None
def stats(data):
 v=[int(r['elapsed_ns']) for r in data];ratios=[int(r['elapsed_ns'])/int(r['period_ns']) for r in data]
 return {'callbacks':len(data),'p50_ns':q(v,.5),'p95_ns':q(v,.95),'p99_ns':q(v,.99),'max_ns':max(v,default=None),'max_period_fraction':max(ratios,default=None),'overruns':sum(r['overrun']=='true' for r in data)}
for role in ['baseline','candidate']:
 D=C/f'measured-{role}';data=list(csv.DictReader((D/'callbacks.csv').open()));summary=json.loads((D/'summary.json').read_text());ok=[r for r in data if r['accepted']=='true'];bad=[r for r in data if r['accepted']!='true'];assert len(ok)==summary['successful_callbacks'] and len(bad)==summary['refused_callbacks']
 def phase(r):
  return 'refusal' if r['accepted']!='true' else 'after-reset-success' if r['reset_before_callback']=='true' else 'first-success' if r['first_callback']=='true' else 'steady-success'
 result={'success':stats(ok),'refusal':stats(bad),'profiles':len(set(r['profile'] for r in data)),'first_success_count':sum(r['first_callback']=='true' for r in ok),'first_refusal_count':sum(r['first_callback']=='true' for r in bad),'error_classes':{e:sum(r['error']==e for r in bad) for e in sorted(set(r['error'] for r in bad))},'phases':{p:stats([r for r in data if phase(r)==p]) for p in ['first-success','after-reset-success','steady-success','refusal']},'by_profile':{},'qos_and_stack':{k:summary[k] for k in ['qos_requested','qos_set_return','qos_get_return','qos_queried','qos_relative_priority','stack_bytes']},'callbacks_csv_sha256':h(D/'callbacks.csv'),'summary_sha256':h(D/'summary.json')}
 for profile in sorted(set(r['profile'] for r in data)):
  x=[r for r in data if r['profile']==profile];s=[r for r in x if r['accepted']=='true'];f=[r for r in x if r['accepted']!='true'];result['by_profile'][profile]={'period_ns':int(x[0]['period_ns']),'success':stats(s),'refusal':stats(f),'phases':{p:stats([r for r in x if phase(r)==p]) for p in ['first-success','after-reset-success','steady-success','refusal']}}
 result['maximum_duration_row']=max(ok,key=lambda r:int(r['elapsed_ns'])) if ok else None
 result['maximum_period_fraction_row']=max(ok,key=lambda r:int(r['elapsed_ns'])/int(r['period_ns'])) if ok else None
 report['callbacks'][role]=result
assert report['callbacks']['baseline']['success']['callbacks']==10688 and report['callbacks']['baseline']['refusal']['callbacks']==1008
assert report['callbacks']['candidate']['success']['callbacks']==21392 and report['callbacks']['candidate']['refusal']['callbacks']==0
integrity['all_expected_processes_and_intervals']=True
integrity['all_callback_prepared_inputs_identical_to_verify']=all(h(C/f'measured-{role}'/'inputs.f32')==h(C/f'verify-{role}'/'inputs.f32') for role in ['baseline','candidate']) if all((C/f'verify-{role}'/'inputs.f32').exists() for role in ['baseline','candidate']) else 'see frozen callback adapter exactinput/golden checks'
(R/'combined-report.json').write_text(json.dumps(report,indent=2)+'\n')
print('INTEGRITY',json.dumps(integrity))
for role,s in report['callbacks'].items():print('CALLBACK',role,json.dumps({k:v for k,v in s.items() if k not in ['by_profile']}))
for case,s in t['by_case'].items():print('THROUGHPUT',case,','.join(f'{v:+.3f}%' for v in s['replicate_changes_percent']),'median',f"{s['median_change_percent']:+.3f}%")
