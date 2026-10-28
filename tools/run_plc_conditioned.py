#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Execute a prebound CUDA study and exhaustive native validation on one VM."""
import argparse,hashlib,json,os,subprocess,sys,time,tarfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('run',type=Path);p.add_argument('--workers',type=int,default=8);a=p.parse_args();r=a.run.resolve();started=time.monotonic()
 assert json.loads((r/'binding.json').read_text())['training_allowed']
 def command(cmd,cwd=ROOT,timeout=3600,env=None):
  subprocess.run(list(map(str,cmd)),cwd=cwd,env=env,check=True,timeout=timeout)
 try:
  (r/'stage.json').write_text(json.dumps(dict(status='SAFETY_PREFLIGHT',pid=os.getpid())))
  command([sys.executable,ROOT/'tools/test_plc_conditioned.py'])
  (r/'stage.json').write_text(json.dumps(dict(status='TRAINING',pid=os.getpid())))
  command([sys.executable,'-u',ROOT/'tools/train_plc_conditioned.py',r/'labels',r/'models','--protocol',ROOT/'docs/neural-plc-conditioned-protocol.json','--binding',r/'binding.json'],timeout=5400)
  (r/'stage.json').write_text(json.dumps(dict(status='NATIVE_BUILD_AND_PARITY',pid=os.getpid())))
  builds=r/'native';builds.mkdir();binaries={}
  baseline=builds/'pr31'
  command([sys.executable,ROOT/'tools/build_plc_conditioned.py',baseline,'--baseline'])
  binaries['pr31']=str(baseline/'target/release/examples/plc_ablation_compare')
  parity=[]
  protocol=json.loads((ROOT/'docs/neural-plc-conditioned-protocol.json').read_text())
  for fit in protocol['fits']:
   for strength in protocol['strengths']:
    suffix={.25:'025',.5:'050',1:'100'}[strength];name=f'{fit}_s{suffix}';out=builds/name
    command([sys.executable,ROOT/'tools/build_plc_conditioned.py',out,'--model',r/'models'/fit,'--strength',str(float(strength))])
    fixture=r/'models'/fit/f'parity-{float(strength)}.bin'
    env=dict(os.environ,SNACKA_ADAPTER_FIXTURE=str(fixture))
    command(['cargo','test','--offline','--release','--target-dir',out/'target','--lib','plc_conditioned::tests::exported_adapter_matches_quantized_pytorch_applied_gains','--','--nocapture'],cwd=out,env=env)
    parity.append(dict(name=name,fixture_sha256=sha(fixture),max_abs_limit=5e-5))
    binaries[name]=str(out/'target/release/examples/plc_ablation_compare')
  (r/'native-parity.json').write_text(json.dumps(parity,indent=2)+'\n')
  (r/'binaries.json').write_text(json.dumps(binaries,indent=2)+'\n')
  selection=json.loads((r/'labels/validation-selection.json').read_text())
  for row in selection:row['corpus_root']=str(r/'corpus/corpus-registered')
  (r/'validation-selection.json').write_text(json.dumps(selection,indent=2)+'\n')
  (r/'stage.json').write_text(json.dumps(dict(status='NATIVE_VALIDATION',pid=os.getpid())))
  command([sys.executable,'-u',ROOT/'tools/evaluate_plc_energy_study.py',r/'validation-selection.json',r/'binaries.json',r/'validation.csv','--workers',str(a.workers)],timeout=10800)
  command([sys.executable,ROOT/'tools/select_plc_conditioned.py',r/'validation.csv',r/'validation-selection.json',r/'decision.json'])
  # Keep exact native inputs, weights, receipts and executables without cargo caches.
  archive=r/'result-proof.tar.gz'
  with tarfile.open(archive,'w:gz') as t:
   for name in ('binding.json','models','native-parity.json','binaries.json','validation-selection.json','validation.csv','validation.json','decision.json'):
    t.add(r/name,arcname=name)
   for out in sorted(builds.iterdir()):
    for folder in ('src','examples'):t.add(out/folder,arcname=f'native/{out.name}/{folder}')
    for name in ('Cargo.toml','production-receipt.json'):t.add(out/name,arcname=f'native/{out.name}/{name}')
    binary=out/'target/release/examples/plc_ablation_compare';t.add(binary,arcname=f'native/{out.name}/plc_ablation_compare')
  decision=json.loads((r/'decision.json').read_text())
  status=dict(status='COMPLETE',seconds=time.monotonic()-started,selected=decision['selected'],archive_size=archive.stat().st_size,archive_sha256=sha(archive),failed_guards={n:len(v['failed_guards']) for n,v in decision['results'].items()})
 except BaseException as e:
  status=dict(status='FAILED',seconds=time.monotonic()-started,error=type(e).__name__);(r/'stage.json').write_text(json.dumps(status));raise
 (r/'stage.json').write_text(json.dumps(status,indent=2)+'\n');print(json.dumps(status),flush=True)
if __name__=='__main__':main()
