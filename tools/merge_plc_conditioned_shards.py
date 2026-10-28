#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Verify every paired case before applying the unchanged guard-first selector."""
import argparse,csv,hashlib,itertools,json,subprocess,sys,tarfile,time
from pathlib import Path
from shard_plc_conditioned import tasks
ROOT=Path(__file__).resolve().parents[1]
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('run',type=Path);a=p.parse_args();r=a.run
 if (r/'validation.csv').exists():p.error('fresh merged output required')
 manifest=json.loads((r/'distributed-manifest.json').read_text());selection=json.loads((r/'validation-selection.json').read_text());grid=tasks(selection);binding=json.loads((r/'binding.json').read_text());reference=json.loads((r/'gpu-validation-partial.json').read_text())
 assert sha(r/'validation-selection.json')==manifest['selection_sha256']
 assert sha(ROOT/'tools/evaluate_plc_energy_study.py')==binding['code_sha256']['evaluate_plc_energy_study.py']
 assert sha(ROOT/'tools/select_plc_conditioned.py')==binding['code_sha256']['select_plc_conditioned.py']
 expected_impl=set(reference['binary_sha256'])|{'classical'}
 combos=set(itertools.product(('clean','noise20','noise10'),('periodic1','periodic3','periodic6','independent5','burst40','burst80'),expected_impl))
 blocks={};part_receipts={};fields=None
 for part in ['prefix']+[f'shard-{i}' for i in range(4)]:
  path=r/(part+'.csv');proof=json.loads((r/(part+'.json')).read_text());assert proof['status']=='COMPLETE' and proof['csv_sha256']==sha(path) and proof['manifest_sha256']==sha(r/'distributed-manifest.json')
  assigned=manifest['prefix_tasks'] if part=='prefix' else manifest['shards'][part.split('-')[1]]
  assert proof['task_indices']==assigned
  if part!='prefix':assert proof['binary_sha256']==reference['binary_sha256']
  assert proof['scorer_sha256']==binding['code_sha256']['evaluate_plc_energy_study.py']
  with path.open() as f:
   reader=csv.DictReader(f);current=reader.fieldnames;rows=list(reader)
  if fields is None:fields=current
  assert fields==current and len(rows)==len(assigned)*360
  for offset,index in enumerate(assigned):
   assert index not in blocks
   row,b,d,m=grid[index];wanted=(Path(row['path']).stem,row.get('cell',f"{row['language']}{row['effective_rate']//1000}k"),str(b),str(d),str(m));block=rows[offset*360:(offset+1)*360]
   assert all((v['clip'],v['cell'],v['bitrate'],v['frame_ms'],v['harmonic'])==wanted for v in block)
   assert {(v['condition'],v['pattern'],v['implementation']) for v in block}==combos
   for condition in ('clean','noise20','noise10'):assert len({v['input_sha256'] for v in block if v['condition']==condition})==1
   blocks[index]=block
  part_receipts[part]=dict(csv_sha256=sha(path),rows=len(rows),cases=len(assigned))
 assert sorted(blocks)==list(range(432))
 output=r/'validation.csv'
 with output.open('w') as f:
  writer=csv.DictWriter(f,fieldnames=fields);writer.writeheader()
  for i in range(432):writer.writerows(blocks[i])
 proof=dict(status='COMPLETE',rows=155520,tasks=432,csv_sha256=sha(output),selection_sha256=sha(r/'validation-selection.json'),binary_sha256=reference['binary_sha256'],scorer_sha256=binding['code_sha256']['evaluate_plc_energy_study.py'],parts=part_receipts,manifest_sha256=sha(r/'distributed-manifest.json'),execution='Complete paired cases across four CPU hosts; unchanged scorer and guard rules')
 (r/'validation.json').write_text(json.dumps(proof,indent=2))
 subprocess.run([sys.executable,ROOT/'tools/select_plc_conditioned.py',output,r/'validation-selection.json',r/'decision.json'],check=True)
 archive=r/'result-proof.tar.gz'
 with tarfile.open(archive,'w:gz') as t:
  for name in ('binding.json','models','native-parity.json','binaries.json','validation-selection.json','validation.csv','validation.json','decision.json','distributed-manifest.json','prefix.csv','prefix.json','gpu-validation-partial.csv','gpu-validation-partial.json','setup-status.json','cpu-validation-setup.json'):
   t.add(r/name,arcname=name)
  for i in range(4):
   for suffix in ('.csv','.json','.log'):t.add(r/f'shard-{i}{suffix}',arcname=f'shards/shard-{i}{suffix}')
   env=r/f'shard-{i}-env.json'
   if env.exists():t.add(env,arcname=f'shards/{env.name}')
  for name in ('shard_plc_conditioned.py','merge_plc_conditioned_shards.py'):t.add(ROOT/'tools'/name,arcname=name)
  for out in sorted((r/'native').iterdir()):
   for folder in ('src','examples'):t.add(out/folder,arcname=f'native/{out.name}/{folder}')
   for name in ('Cargo.toml','production-receipt.json'):t.add(out/name,arcname=f'native/{out.name}/{name}')
   t.add(out/'target/release/examples/plc_ablation_compare',arcname=f'native/{out.name}/plc_ablation_compare')
 decision=json.loads((r/'decision.json').read_text());status=dict(status='COMPLETE',selected=decision['selected'],archive_size=archive.stat().st_size,archive_sha256=sha(archive),rows=155520,failed_guards={n:len(v['failed_guards']) for n,v in decision['results'].items()})
 (r/'final-stage.json').write_text(json.dumps(status,indent=2));print(json.dumps(status),flush=True)
if __name__=='__main__':main()
