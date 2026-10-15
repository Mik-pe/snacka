"""Replay existing frozen2130 inputs with BATCHES0; compare complete JSON to retained baseline."""
import pathlib,json,hashlib,subprocess,os,datetime,concurrent.futures,math
E=pathlib.Path(__file__).resolve().parent; P=E/'production'; B=pathlib.Path('/tmp/snacka-opus-full-m5-explore-4340e8d-20261015'); OLD=pathlib.Path('/tmp/snacka-opus-decoder-last-samples-evidence-20261015/production')
h=lambda p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
m=json.loads((P/'manifest.json').read_text()); bm=json.loads((OLD/'manifest.json').read_text()); oldproof=json.loads((OLD/'untimed-comparison/summary.json').read_text()); oldrows=OLD/'untimed-comparison/rows.jsonl'
assert m['source_commit']=='567d1b6de5c41593ce1a6437d633a1f01947f60f'
for k in ('library','binary'): assert h(m[k]['path'])==m[k]['sha256']
assert h(oldrows)==oldproof['rows_sha256']
cases=json.loads((B/'cases.json').read_text());inputs=json.loads((B/'inputs.json').read_text()); names={x['name']:x for x in inputs}
assert len(cases)==2130 and len(inputs)==142
for x in inputs:assert h(x['path'])==x['sha256'],x['name']
baseline=[json.loads(s) for s in oldrows.read_text().splitlines()]
assert len(baseline)==len(cases) and all(b['status']=='EXACT' and b['index']==i and b['case']==cases[i] for i,b in enumerate(baseline))
env=dict(os.environ,SNACKA_BENCH_QOS='user-initiated',MallocNanoZone='0',RUST_LOG='warn')
for k in list(env):
 if k.startswith('DYLD_') or k in ('MallocGuardEdges','MallocScribble','MallocStackLogging','RUST_BACKTRACE','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS'):env.pop(k,None)
D=P/'untimed-comparison';D.mkdir(exist_ok=False)
pin={'source_commit':m['source_commit'],'manifest_sha256':h(P/'manifest.json'),'baseline_manifest_sha256':h(OLD/'manifest.json'),'baseline_rows_sha256':h(oldrows),'cases_sha256':h(B/'cases.json'),'inputs_sha256':h(B/'inputs.json'),'script_sha256':h(__file__),'binary':m['binary'],'library':m['library'],'native':m['native'],'workers':4,'batches':0,'timing_warmup_calibration':False,'input_generation':False,'comparison':'Complete parsed result JSON equal to retained baseline for every case; retain all raw output/failures.'}
(D/'binding-before-run.json').write_text(json.dumps(pin,indent=2)+'\n')
def run(pair):
 i,c=pair; row={'index':i,'case':c,'command':[m['binary']['path'],'matrix',str(c['rate']),str(c['channels']),c['schedule'],'0','200000',names[c['stream']]['path']],'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat()}
 try:
  r=subprocess.run(row['command'],env=env,capture_output=True,text=True,timeout=90)
  row.update(return_code=r.returncode,stdout=r.stdout,stderr=r.stderr)
  assert r.returncode==0,r.stderr
  v=json.loads(r.stdout);row['result']=v
  assert v['kind']=='paired_decoder_matrix' and v['timed_batches']==0
  assert v['sample_rate']==c['rate'] and v['channels']==c['channels'] and v['schedule']==c['schedule']
  assert v['all_sample_counts_match'] and v['ordinary_and_plc_final_ranges_match']
  assert not any(k in v for k in ('owned_batch_ns','reference_batch_ns','minimum_observed_batch_ns','p50_ns_per_operation'))
  assert all(math.isfinite(v[k]) for k in ('reference_pcm_error_rms','reference_pcm_rms','reference_pcm_max_error'))
  assert v==baseline[i]['result'],{'changed_keys':[k for k in sorted(set(v)|set(baseline[i]['result'])) if v.get(k)!=baseline[i]['result'].get(k)]}
  row['status']='EXACT'
 except Exception as error:row.update(status='FAILED',error=repr(error))
 row['finished_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat();return row
summary={'kind':'UNTIMED_COMPLETE_JSON_COMPARISON','source_commit':m['source_commit'],'expected':2130,'attempted':0,'exact':0,'failed':0,'failures':[],'timing_warmup_calibration':False,'inputs_generated':False,'workers':4}
with (D/'rows.jsonl').open('x') as sink, concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
 for row in pool.map(run,enumerate(cases)):
  sink.write(json.dumps(row,separators=(',',':'))+'\n');sink.flush();summary['attempted']+=1
  if row['status']=='EXACT':summary['exact']+=1
  else:summary['failed']+=1;summary['failures'].append({'case':row['case'],'error':row['error']})
  if summary['attempted']%100==0 or summary['attempted']==2130:
   (D/'progress.json').write_text(json.dumps(summary,indent=2)+'\n');print(summary['attempted'],'/2130 exact',summary['exact'],'failed',summary['failed'],flush=True)
for k in ('library','binary'):assert h(m[k]['path'])==m[k]['sha256']
for x in inputs:assert h(x['path'])==x['sha256']
assert h(oldrows)==oldproof['rows_sha256']
summary.update(integrity_unchanged=True,rows_sha256=h(D/'rows.jsonl'),binding_sha256=h(D/'binding-before-run.json'))
(D/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary),flush=True)
assert summary['exact']==2130 and summary['failed']==0
