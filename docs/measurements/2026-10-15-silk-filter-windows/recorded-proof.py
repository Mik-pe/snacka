# SPDX-License-Identifier: MIT
"""Run the untimed complete 2,130-case before/after JSON proof; keep both outputs."""
import argparse
import os
argparse.ArgumentParser(description=__doc__).parse_args()
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import hashlib, json, subprocess

root=Path(os.environ['SNACKA_BENCH_ARTIFACTS']).resolve()
out=root/'proof'
out.mkdir()
(out/'raw').mkdir()
manifest=json.loads((root/'source.json').read_text())
def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
source_digest=sha(root/'source.json')
runner_digest=sha(Path(__file__))
streams=sorted((root/'streams').glob('*.bit'))
assert len(streams)==142
cases=[(path,rate,int(path.stem[-3]),schedule) for path in streams
       for rate in [8000,12000,16000,24000,48000]
       for schedule in ['decode','plc-return','fec-return']]
assert len(cases)==2130
def check(index_case):
    index,(path,rate,channels,schedule)=index_case
    pair={}
    for role in ['before','after']:
        command=[str(root/f'benchmark-{role}'),'matrix',str(rate),str(channels),schedule,'0','200000',str(path)]
        done=subprocess.run(command,text=True,capture_output=True)
        (out/'raw'/f'{index:04d}-{role}.stdout').write_text(done.stdout)
        (out/'raw'/f'{index:04d}-{role}.stderr').write_text(done.stderr)
        if done.returncode:
            raise RuntimeError(f'{command}: {done.stderr}')
        pair[role]=json.loads(done.stdout)
    assert pair['before']==pair['after'],(path,rate,channels,schedule,pair)
    assert pair['after']['all_sample_counts_match']
    assert pair['after']['ordinary_and_plc_final_ranges_match']
    return {'case':[path.name,rate,channels,schedule],
            'exact_full_json':True,'result':pair['after']}
rows=[]
with ThreadPoolExecutor(max_workers=4) as workers:
    with (out/'rows.jsonl').open('w') as sink:
        for row in workers.map(check,enumerate(cases)):
            rows.append(row)
            sink.write(json.dumps(row,separators=(',',':'))+'\n')
            if len(rows)%150==0:
                print(len(rows),len(cases),'exact cases',flush=True)
assert source_digest==sha(root/'source.json')
assert runner_digest==sha(Path(__file__))
for role,row in manifest['variants'].items():
    assert sha(root/f'benchmark-{role}')==row['binary_sha256']
    assert sha(Path(row['target'])/'release/libsnacka.rlib')==row['library_sha256']
    for relative,expected in row['sources'].items():
        assert sha(Path(row['source'])/relative)==expected,(role,relative)
assert sha(Path(manifest['bridge_source']))==manifest['bridge_sha256']
assert sha(Path(manifest['reference_real_path']))==manifest['reference_sha256']
for name,expected in manifest['tool_sha256'].items():
    assert sha(Path(name))==expected
for path in streams:
    assert sha(path)==manifest['stream_sha256'][path.name]
summary={'base_commit':manifest['base_commit'],'changed_files':manifest['changed_files'],
         'build_manifest_sha256':source_digest,'runner_sha256':runner_digest,
         'streams':len(streams),'cases':len(rows),'full_json_equivalence':True,
         'actual_fec_cases':sum(row['result']['fec_events']>0 for row in rows),
         'sources_and_binaries_unchanged':True,
         'stream_sha256':{path.name:sha(path) for path in streams},
         'raw_rows_sha256':sha(out/'rows.jsonl'),
         'scope':'All 142 generated geometries, 5 output rates, mono/stereo, decode/PLC-return/FEC-return; zero timed batches. Both complete JSON outputs exact; PCM bit hashes, packet ranges, sample counts, recovery diagnostics unchanged.'}
assert summary['actual_fec_cases']==70
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({key:value for key,value in summary.items() if key!='stream_sha256'},indent=2),flush=True)
