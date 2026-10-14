import csv,hashlib,io,json,os,pathlib,statistics,subprocess
ROOT=pathlib.Path('/workspace/snacka')
BASE=pathlib.Path('/workspace/snacka-quality-pitch-baseline')
DEST=pathlib.Path('/workspace/snacka-quality-pitch-cpu')
CORPUS=pathlib.Path('/workspace/snacka-quality-corpus-v2')
DEST.mkdir(exist_ok=True)
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def snapshot(root):
 files=[root/'Cargo.toml',root/'Cargo.lock',*sorted((root/'src').rglob('*.rs')),*sorted((root/'benches').rglob('*.rs'))]
 return {str(p.relative_to(root)):sha(p) for p in files}
roots={'baseline':BASE,'current':ROOT}
initial={role:snapshot(root) for role,root in roots.items()}
binaries={role:root/'target/release/deps/opus-238e56d6c6e4d89d' for role,root in roots.items()}
inputs={}
for clip in ['cen1-mdms2-b.wav','cen1-mjgm-b.wav']:
 pcm=DEST/(pathlib.Path(clip).stem+'.f32')
 subprocess.run(['ffmpeg','-v','error','-y','-i',str(CORPUS/clip),'-ar','48000','-ac','1','-f','f32le',str(pcm)],check=True)
 inputs[clip]={'wav_sha256':sha(CORPUS/clip),'pcm_sha256':sha(pcm),'pcm_path':str(pcm)}
env={**os.environ,'LD_LIBRARY_PATH':'/workspace/snacka-reference-build','SNACKA_BENCH_CPU_LABEL':'AMD_EPYC_9V74'}
cases=[('silk','NB','20','1','12000'),('silk','WB','20','1','12000'),('silk','WB','20','1','24000'),('silk','WB','20','1','32000'),('hybrid','FB','20','1','32000'),('celt','WB','20','1','12000'),('celt','WB','20','1','32000')]
groups={}
for repetition in range(3):
 for clip,input_info in inputs.items():
  for case in cases:
   key=(clip,*case)
   for role in (('baseline','current') if repetition!=1 else ('current','baseline')):
    result=subprocess.run([str(binaries[role]),'--encoder-case',*case,'--input',input_info['pcm_path'],'--frames','256'],env=env,text=True,capture_output=True,check=True)
    stem=f'{pathlib.Path(clip).stem}-'+ '-'.join(case)+f'-{role}-{repetition}'
    (DEST/(stem+'.csv')).write_text(result.stdout)
    (DEST/(stem+'.log')).write_text(result.stderr)
    rows=list(csv.DictReader(io.StringIO(result.stdout)))
    assert len(rows)==2 and {r['codec'] for r in rows}=={'snacka','libopus'}
    owned=next(r for r in rows if r['codec']=='snacka')
    assert owned['batches']=='1000' and owned['signal']=='file'
    assert owned['packet_bitrate']==case[4]
    groups.setdefault(key,{})[(role,repetition)]=owned
 print('Completed repetition',repetition,flush=True)
assert all(snapshot(root)==initial[role] for role,root in roots.items())
summary=[]
for key,group in groups.items():
 row=dict(zip(['clip','wire_mode','bandwidth','frame_ms','channels','bitrate'],key))
 changes=[100*(float(group[('current',rep)]['paired_encode_ratio_p50'])/float(group[('baseline',rep)]['paired_encode_ratio_p50'])-1) for rep in range(3)]
 row.update(p50_change_percent=statistics.median(changes),change_min_percent=min(changes),change_max_percent=max(changes))
 for role in roots:
  for p in (50,95,99):row[f'{role}_p{p}_ratio']=statistics.median(float(group[(role,rep)][f'paired_encode_ratio_p{p}']) for rep in range(3))
 summary.append(row)
with (DEST/'summary.csv').open('w',newline='') as stream:
 w=csv.DictWriter(stream,fieldnames=list(summary[0]),lineterminator='\n');w.writeheader();w.writerows(summary)
metadata={'baseline_commit':'fdc18d013266bdcd66def2c77be402b681b84b08','cpu':'AMD EPYC 9V74','affinity':sorted(os.sched_getaffinity(0)),'reference_version':'libopus 1.6.1','reference_sha256':sha(pathlib.Path('/workspace/snacka-reference-build/libopus.so')),'cases':14,'repetitions':3,'batches_per_case':1000,'frames_in_input_cycle':256,'source_unchanged':True,'sources':initial,'inputs':inputs,'binary_sha256':{role:sha(p) for role,p in binaries.items()},'rustc':subprocess.check_output(['rustc','--version'],text=True).strip(),'flags':'release profile opt-level3, codegen16, overflow checks; no LTO/native CPU flags, reference search path only','method':'three before/after repetitions, executable order reversed in middle; benchmark alternates Snacka/libopus pairs, batches >=200us; medians of libopus-normalized current/baseline p50 ratios; construction, IO, validation excluded; not single-frame callback tails','artifacts_sha256':{p.name:sha(p) for p in sorted(DEST.iterdir()) if p.suffix in ['.csv','.log']}}
(DEST/'source.json').write_text(json.dumps(metadata,indent=2)+'\n')
for row in summary:print(row['clip'],row['wire_mode'],row['bandwidth'],row['bitrate'],'p50 change',round(row['p50_change_percent'],2),'vs libopus',row['current_p50_ratio'],flush=True)
