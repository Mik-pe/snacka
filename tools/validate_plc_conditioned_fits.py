#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Validate sealed fits on a CPU host, without fitting or changing the scorer."""
import argparse,hashlib,json,os,subprocess,sys,tarfile,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('run',type=Path);p.add_argument('--workers',type=int,default=8);a=p.parse_args();r=a.run.resolve();started=time.monotonic()
 if (r/'validation.csv').exists():p.error('fresh full replay required; partial GPU rows are diagnostics only')
 binding=json.loads((r/'binding.json').read_text())
 for name,digest in binding['code_sha256'].items():assert sha(ROOT/'tools'/name)==digest,name
 for name,digest in binding['source_sha256'].items():assert sha(ROOT/name)==digest,name
 for report in json.loads((r/'models/training-summary.json').read_text()):
  assert report['updates']==report['selected_step']==12000
  assert report['binding_sha256']==sha(r/'binding.json')
  assert sha(r/'models'/report['fit']/'plc_conditioned_weights.rs')==report['weights_sha256']
 previous=json.loads((r/'gpu-validation-partial.json').read_text())
 binaries=json.loads((r/'binaries.json').read_text())
 for name,path in binaries.items():assert sha(Path(path))==previous['binary_sha256'][name],name
 execution=dict(schema=1,reason='Move CPU-only audio scoring off A100 after fitting. Fresh complete replay; no partial GPU rows used for selection.',fixed_fit_count=6,applications=18,workers=a.workers,scorer_sha256=sha(ROOT/'tools/evaluate_plc_energy_study.py'),selector_sha256=sha(ROOT/'tools/select_plc_conditioned.py'),wrapper_sha256=sha(Path(__file__)),binding_sha256=sha(r/'binding.json'),test_audio_seen=False)
 (r/'validation-execution.json').write_text(json.dumps(execution,indent=2)+'\n')
 try:
  (r/'stage.json').write_text(json.dumps(dict(status='NATIVE_VALIDATION_CPU',pid=os.getpid())))
  subprocess.run([sys.executable,'-u',ROOT/'tools/evaluate_plc_energy_study.py',r/'validation-selection.json',r/'binaries.json',r/'validation.csv','--workers',str(a.workers)],check=True,timeout=20000)
  subprocess.run([sys.executable,ROOT/'tools/select_plc_conditioned.py',r/'validation.csv',r/'validation-selection.json',r/'decision.json'],check=True)
  archive=r/'result-proof.tar.gz'
  with tarfile.open(archive,'w:gz') as t:
   for name in ('binding.json','models','native-parity.json','binaries.json','validation-selection.json','validation.csv','validation.json','decision.json','validation-execution.json','gpu-validation-partial.csv','gpu-validation-partial.json','setup-status.json','cpu-validation-setup.json'):
    t.add(r/name,arcname=name)
   t.add(Path(__file__),arcname='validate_plc_conditioned_fits.py')
   for out in sorted((r/'native').iterdir()):
    for folder in ('src','examples'):t.add(out/folder,arcname=f'native/{out.name}/{folder}')
    for name in ('Cargo.toml','production-receipt.json'):t.add(out/name,arcname=f'native/{out.name}/{name}')
    t.add(out/'target/release/examples/plc_ablation_compare',arcname=f'native/{out.name}/plc_ablation_compare')
  decision=json.loads((r/'decision.json').read_text());status=dict(status='COMPLETE',seconds=time.monotonic()-started,selected=decision['selected'],archive_size=archive.stat().st_size,archive_sha256=sha(archive),failed_guards={n:len(v['failed_guards']) for n,v in decision['results'].items()})
 except BaseException as e:
  (r/'stage.json').write_text(json.dumps(dict(status='FAILED',error=type(e).__name__)));raise
 (r/'stage.json').write_text(json.dumps(status,indent=2));print(json.dumps(status),flush=True)
if __name__=='__main__':main()
