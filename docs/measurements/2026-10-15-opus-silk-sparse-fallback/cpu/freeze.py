from pathlib import Path
import hashlib,json,subprocess,datetime
R=Path('/tmp/snacka-opus-silk-sparse-cpu-20261015')
C=Path('/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015/controls/callback-latency')
h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
frozen=json.loads(Path('/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015/frozen-source.json').read_text())
source={}
for role in ['baseline','candidate']:
 root=R/role;files={str(p.relative_to(root)):h(p) for p in [root/'Cargo.toml',root/'Cargo.lock',*sorted((root/'src').rglob('*.rs'))]}
 source[role]={'files':files,'combined_sha256':hashlib.sha256(json.dumps(files,sort_keys=True,separators=(',',':')).encode()).hexdigest()}
assert source['candidate']['files']==frozen['source_files']
for role in ['baseline','candidate']:
 log=(R/f'{role}-clean-build.log').read_text();assert 'mbx' not in log;assert '-C codegen-units=16' in log and '-C overflow-checks=on' in log and 'target-cpu' not in log
checks=[json.loads(x) for x in (R/'throughput-final-verify-candidate.jsonl').read_text().splitlines()]
assert (R/'throughput-final-verify-baseline.jsonl').read_bytes()==(R/'throughput-final-verify-candidate.jsonl').read_bytes()
for p in (R/'inputs-candidate').glob('*.f32'):assert p.read_bytes()==(R/'inputs-baseline'/p.name).read_bytes()
files=['PLAN.md','throughput.rs','throughput-baseline','throughput-candidate','run-throughput.py','throughput-final-verify-baseline.jsonl','throughput-final-verify-candidate.jsonl','rustc-version.txt','cpu-model.txt','linked-libraries.txt','baseline-clean-build.log','candidate-clean-build.log','baseline-clean-target/release/libsnacka.rlib','candidate-clean-target/release/libsnacka.rlib']
measurement={name:h(R/name) for name in files}
for name in ['adapter.rs','callback-baseline','callback-candidate','freeze.json']:
 p=C/name;measurement[str(p)]=h(p)
inputs={p.stem:{'sha256':h(p),'bytes':p.stat().st_size,'fnv64':next(c['input_fnv64'] for c in checks if c['case']==p.stem)} for p in sorted((R/'inputs-candidate').glob('*.f32'))}
for p in sorted((R/'inputs-candidate').glob('*.f32')):measurement[str(p.relative_to(R))]=h(p)
callback=json.loads((C/'freeze.json').read_text())
manifest={'created_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source_candidate':'8f723ba1f35c053e1cc840f38a4b20446a127dde','source_baseline':'cca87dd757d96623b91bff81d34a021e5a3e2792','source':source,'compiler':(R/'rustc-version.txt').read_text(),'cpu':(R/'cpu-model.txt').read_text().strip(),'settings':{'opt_level':3,'codegen_units':16,'target_cpu':'default','overflow_checks':True,'lto':False,'incremental':False,'compiler_wrapper':None,'rustc_workspace_wrapper':None,'qos_class':25,'qos_relative_priority':0},'native_reference':{'path':'/opt/homebrew/lib/libopus.dylib','sha256':h('/opt/homebrew/lib/libopus.dylib'),'version':'libopus1.6.1','silk_application':2052,'hybrid_application':2048,'celt_application':2051,'complexity':10,'fec_packet_loss_percent':20,'rate_control':'CBR','wire_match':'all10controlsgeometrymatched in1024callbackverification; collectalltimedTOCs; mismatches suppressabsoluteparityratios','fec_availability':'differentforthreecontrols; absoluteparityratiossuppressed, pairednormalizationonly'},'throughput':{'cases':checks,'repetitions':3,'processes_per_case_per_repetition':4,'order':['baseline','candidate','candidate','baseline'],'total_measured_processes':120,'warmup_pairs':128,'batches':1000,'minimum_calibrated_batch_ns':200000,'input_hashes':inputs,'individual_callback_tail_claim':False},'callbacks':{'freeze_path':str(C/'freeze.json'),'freeze_sha256':h(C/'freeze.json'),'ordinary_synthetic_profiles':56,'retained_lbrr_carrier_profiles':1,'original_refusal_profiles':2,'composition_profiles':4,'old_success_profiles':10,'total_profiles':73,'contexts_per_profile':16,'measured_processes':2,'baseline_intervals':11696,'baseline_successes':10688,'baseline_first_refusals':1008,'candidate_intervals':21392,'candidate_successes':21392,'candidate_refusals':0,'total_intervals':33088},'measurement_files':measurement,'timing_status':'NOT RUN; requires explicit root quiet authorization'}
(R/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('manifest_sha256',h(R/'manifest.json'))
print('measurement_files',len(measurement),'total_measured_processes',122)
