# SPDX-License-Identifier: MIT
"""Run the fixed eight-case CPU experiment; requires an externally arranged quiet host.

Preserves all three repetitions, raw batch arrays and process metadata. Do not
run concurrently with proof/builds or other CPU workers. No retry/discard logic.
"""
import argparse
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument("--quiet-window-confirmed", action="store_true", required=True,
                    help="Confirm all local codec workers are CPU-idle throughout this run.")
parser.parse_args()
from pathlib import Path
import csv, datetime, hashlib, json, os, statistics, subprocess

root=Path(os.environ['SNACKA_BENCH_ARTIFACTS']).resolve()
out=root/'acceptance'
out.mkdir()
source=json.loads((root/'source.json').read_text())
source_digest=hashlib.sha256((root/'source.json').read_bytes()).hexdigest()
base=root/'streams'
cases=[('SILK-WB-20ms-1ch',8000,1,'decode'),
       ('SILK-WB-20ms-1ch',8000,1,'plc-return'),
       ('SILK-WB-20ms-1ch',8000,1,'fec-return'),
       ('SILK-WB-60ms-1ch',8000,1,'decode'),
       ('SILK-WB-10ms-1ch',8000,1,'decode'),
       ('SILK-WB-20ms-2ch',8000,2,'decode'),
       ('SILK-WB-20ms-1ch',48000,1,'decode'),
       ('CELT-FB-2.5ms-1ch',48000,1,'decode')]
def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
streams={name:sha(base/f'{name}.bit') for name,_,_,_ in cases}
for name,digest in streams.items():
    assert digest==source['stream_sha256'][name+'.bit'],('Input changed since frozen generation',name)
def check_frozen():
    assert sha(root/'source.json')==source_digest
    assert sha(Path(source['bridge_source']))==source['bridge_sha256']
    assert sha(Path(source['reference_real_path']))==source['reference_sha256']
    for name,expected in source['tool_sha256'].items():
        assert sha(Path(name))==expected
    for role,row in source['variants'].items():
        for name,expected in row['sources'].items():
            assert sha(Path(row['source'])/name)==expected,(role,name)
        assert sha(Path(row['target'])/'release/libsnacka.rlib')==row['library_sha256']
        assert sha(Path(row['binary']))==row['binary_sha256']
    for name,expected in streams.items():
        assert sha(base/f'{name}.bit')==expected
check_frozen()
environment=dict(os.environ,SNACKA_BENCH_QOS='user-initiated',MallocNanoZone='0',RUST_LOG='warn')
for key in ['DYLD_LIBRARY_PATH','DYLD_INSERT_LIBRARIES','MallocGuardEdges','MallocScribble','MallocStackLogging','RUST_BACKTRACE','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS']:
    environment.pop(key,None)
metadata={'source_sha256':source_digest,'source':source,'candidate_commit':'c9707899a71469343efa4476617a91ab06ba0e2d',
          'runner_sha256':sha(Path(__file__)),'cases':cases,'stream_sha256':streams,
          'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'repetitions':3,'alternating_batches':1000,'minimum_batch_ns':200000,
          'runtime_environment':{key:environment.get(key) for key in [
              'SNACKA_BENCH_QOS','DYLD_LIBRARY_PATH','DYLD_INSERT_LIBRARIES','MallocNanoZone',
              'MallocGuardEdges','MallocScribble','MallocStackLogging','RUST_BACKTRACE','RUST_LOG',
              'RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS']},
          'host':{key:subprocess.check_output(command,text=True).strip() for key,command in {
              'os':['sw_vers'],'hardware':['sysctl','-n','hw.model'],
              'cpu':['sysctl','-n','machdep.cpu.brand_string'],'architecture':['uname','-m']}.items()},
          'scope':'8 selected complete decoder cases; batch-average throughput, not individual-frame deadlines; WB20mono8k FEC-return is fallback (0 actual FEC events in this stream).',
          'quiet_window':'Runner invoked with --quiet-window-confirmed; operator is responsible for keeping local codec workers CPU-idle.'}
(out/'source.json').write_text(json.dumps(metadata,indent=2)+'\n')
rows=[]
for repetition in range(3):
    for index,(name,rate,channels,schedule) in enumerate(cases):
        case=f'{name}-{rate}-{schedule}'
        roles=['before','after'] if (repetition+index)%2==0 else ['after','before']
        pair={}
        for role in roles:
            command=[str(root/f'benchmark-{role}'),'matrix',str(rate),str(channels),schedule,
                     '1000','200000',str(base/f'{name}.bit')]
            started=datetime.datetime.now(datetime.timezone.utc).isoformat()
            process=subprocess.Popen(command,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=environment)
            stdout,stderr=process.communicate()
            prefix=out/f'{case}-{repetition}-{role}'
            stdout_path=Path(str(prefix)+'.json')
            stderr_path=Path(str(prefix)+'.log')
            stdout_path.write_text(stdout)
            stderr_path.write_text(stderr)
            with (out/'processes.jsonl').open('a') as sink:
                sink.write(json.dumps({'case':case,'repetition':repetition,'role':role,'command':command,
                    'pid':process.pid,'started_utc':started,
                    'finished_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    'exit_code':process.returncode,'stdout_sha256':sha(stdout_path),'stderr_sha256':sha(stderr_path)})+'\n')
            if process.returncode:
                raise RuntimeError(f'{command}: {stderr}')
            result=json.loads(stdout)
            assert result['all_batches_meet_minimum']
            assert result['minimum_observed_batch_ns']>=200000
            assert result['all_sample_counts_match'] and result['ordinary_and_plc_final_ranges_match']
            assert result['timed_batches']==1000
            assert len(result['owned_batch_ns'])==len(result['reference_batch_ns'])==1000
            pair[role]=result
        assert pair['before']['owned_pcm_hash']==pair['after']['owned_pcm_hash']
        assert pair['before']['packet_range_hash']==pair['after']['packet_range_hash']
        assert pair['before']['recovery_intervals']==pair['after']['recovery_intervals']
        for percentile in ['p50','p95','p99']:
            before=pair['before'][percentile+'_ns_per_operation']
            after=pair['after'][percentile+'_ns_per_operation']
            rows.append({'case':case,'repetition':repetition,'role_order':','.join(roles),
                         'percentile':percentile,'before_ratio':before['ratio'],'after_ratio':after['ratio'],
                         'normalized_change_percent':100*(after['ratio']/before['ratio']-1),
                         'before_owned_ns':before['snacka'],'after_owned_ns':after['snacka'],
                         'before_reference_ns':before['libopus'],'after_reference_ns':after['libopus'],
                         'fec_events':pair['after']['fec_events']})
        with (out/'repetitions.csv').open('w') as sink:
            writer=csv.DictWriter(sink,fieldnames=list(rows[0]),lineterminator='\n')
            writer.writeheader()
            writer.writerows(rows)
        print(repetition,case,[(row['percentile'],round(row['normalized_change_percent'],3)) for row in rows[-3:]],flush=True)
summary=[]
for name,rate,_,schedule in cases:
    case=f'{name}-{rate}-{schedule}'
    row={'case':case}
    for percentile in ['p50','p95','p99']:
        group=[item for item in rows if item['case']==case and item['percentile']==percentile]
        row[percentile]={'median_normalized_change_percent':statistics.median(item['normalized_change_percent'] for item in group),
                         'changes_percent':[item['normalized_change_percent'] for item in group],
                         'after_native_ratios':[item['after_ratio'] for item in group],
                         'median_after_native_ratio':statistics.median(item['after_ratio'] for item in group)}
    summary.append(row)
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
check_frozen()
assert sha(Path(__file__))==metadata['runner_sha256']
metadata['finished_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat()
metadata['final_integrity_check']='All source/library/bridge/binary/tool/reference/input and runner hashes unchanged.'
metadata['raw_results']={path.name:sha(path) for path in sorted(out.iterdir()) if path.name!='source.json'}
(out/'source.json').write_text(json.dumps(metadata,indent=2)+'\n')
print('FINISHED',flush=True)
