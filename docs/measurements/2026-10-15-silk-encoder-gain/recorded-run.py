#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
from pathlib import Path
import argparse, csv, datetime, hashlib, io, json, math, os, statistics, subprocess

parser=argparse.ArgumentParser(description="Repeat the recorded ten-case timing only during a coordinated CPU-quiet window.")
parser.add_argument('--artifacts',default=os.environ.get('SNACKA_BENCH_ARTIFACTS'))
args=parser.parse_args()
if not args.artifacts:
    parser.error('set --artifacts or SNACKA_BENCH_ARTIFACTS to a recorded-build.py result')
root=Path(args.artifacts).resolve()
out=root/'acceptance'
out.mkdir(exist_ok=False)
source=json.loads((root/'source.json').read_text())
cases=[
    ['hybrid','FB','40','1','32000'],
    ['hybrid','FB','20','2','64000'],
    ['hybrid','FB','10','2','64000'],
    ['hybrid','SWB','10','2','64000'],
    ['hybrid','SWB','40','2','64000'],
    ['hybrid','FB','60','2','64000'],
    ['silk','WB','20','1','24000'],
    ['silk','NB','20','1','12000'],
    ['celt','FB','2.5','2','64000'],
    ['celt','WB','5','1','32000'],
]
def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
def check_frozen():
    for role,row in source['variants'].items():
        directory=Path(row['source'])
        for relative,expected in row['sources'].items():
            assert sha(directory/relative)==expected,(role,relative)
        assert sha(Path(row['target'])/'release/libsnacka.rlib')==row['library_sha256']
        assert sha(root/f'benchmark-{role}')==row['benchmark_sha256']
        assert sha(root/f'probe-{role}')==row['probe_sha256']
    for path,expected in source['bridge_sources'].items():
        assert sha(Path(path))==expected,path
    for path,expected in source['tool_sha256'].items():
        assert sha(Path(path))==expected,path
    assert sha(Path(source['reference_path']).resolve())==source['reference_sha256']
    assert sha(Path(source['proof_source']))==source['proof_source_sha256']
check_frozen()
env=dict(os.environ)
env['SNACKA_BENCH_QOS']='user-initiated'
env['SNACKA_BENCH_CPU_LABEL']='Apple M5 macOS, controlled quiet acceptance'
manifest={'source_sha256':sha(root/'source.json'),'source':source,
          'runner_sha256':sha(Path(__file__)),'qos':env['SNACKA_BENCH_QOS'],
          'runtime_environment':{key:env.get(key) for key in [
              'SNACKA_BENCH_QOS','SNACKA_BENCH_CPU_LABEL','DYLD_LIBRARY_PATH',
              'DYLD_INSERT_LIBRARIES','MallocNanoZone','MallocGuardEdges','MallocScribble',
              'MallocStackLogging','RUST_BACKTRACE','RUST_LOG','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS']},
          'cases':cases,'repetitions':3,'batches':1000,'minimum_batch_ns':200000,
          'frames':64,'warmup':128,
          'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'host':{key:subprocess.check_output(cmd,text=True).strip() for key,cmd in {
              'os':['sw_vers'],'hardware':['sysctl','-n','hw.model'],'cpu':['sysctl','-n','machdep.cpu.brand_string'],
              'architecture':['uname','-m']}.items()},
          'scope':'10 selected public encode cases, CBR48k changing voiced input; continuous histories; excludes construction/reset/I/O; paired batch-average throughput percentiles, not individual-frame tails',
          'quiet_window':'Operator must coordinate local CPU-idle workers before invoking this timing runner.'}
(out/'source.json').write_text(json.dumps(manifest,indent=2)+'\n')
rows=[]
for repetition in range(3):
    for index,case in enumerate(cases):
        roles=['before','after'] if (repetition+index)%2==0 else ['after','before']
        case_name='-'.join(case)
        results={}
        for role in roles:
            binary=root/f'benchmark-{role}'
            assert sha(binary)==source['variants'][role]['benchmark_sha256']
            command=[str(binary),'--encoder-case',*case]
            started=datetime.datetime.now(datetime.timezone.utc).isoformat()
            process=subprocess.Popen(command,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env)
            stdout,stderr=process.communicate()
            prefix=out/f'{case_name}-{repetition}-{role}'
            csv_path=Path(str(prefix)+'.csv')
            log_path=Path(str(prefix)+'.log')
            csv_path.write_text(stdout)
            log_path.write_text(stderr)
            with (out/'processes.jsonl').open('a') as sink:
                sink.write(json.dumps({'command':command,'pid':process.pid,'role':role,
                    'repetition':repetition,'case':case_name,'started_utc':started,
                    'finished_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    'exit_code':process.returncode,'stdout_sha256':sha(csv_path),'stderr_sha256':sha(log_path)})+'\n')
            if process.returncode:
                raise RuntimeError(f'{command}: {stderr}')
            parsed=list(csv.DictReader(io.StringIO(stdout)))
            assert len(parsed)==2 and {row['codec'] for row in parsed}=={'snacka','libopus'},parsed
            results[role]={row['codec']:row for row in parsed}
            assert all(int(row['batches'])==1000 for row in parsed)
        for percentile in ['p50','p95','p99']:
            ratio_key=f'paired_encode_ratio_{percentile}'
            time_key=f'{percentile}_batch_ns_per_frame'
            before_ratio=float(results['before']['snacka'][ratio_key])
            after_ratio=float(results['after']['snacka'][ratio_key])
            assert math.isfinite(before_ratio) and math.isfinite(after_ratio)
            rows.append({'case':case_name,'mode':case[0],'bandwidth':case[1],
                         'frame_ms':case[2],'channels':int(case[3]),'bitrate':int(case[4]),
                         'repetition':repetition,'role_order':','.join(roles),'percentile':percentile,
                         'before_ratio':before_ratio,'after_ratio':after_ratio,
                         'normalized_change_percent':100*(after_ratio/before_ratio-1),
                         'before_owned_ns':float(results['before']['snacka'][time_key]),
                         'after_owned_ns':float(results['after']['snacka'][time_key]),
                         'before_reference_ns':float(results['before']['libopus'][time_key]),
                         'after_reference_ns':float(results['after']['libopus'][time_key])})
        with (out/'repetitions.csv').open('w') as sink:
            writer=csv.DictWriter(sink,fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)
        print(repetition,case_name,[(r['percentile'],round(r['normalized_change_percent'],3)) for r in rows[-3:]],flush=True)
summary=[]
for case in cases:
    name='-'.join(case)
    row={'case':name}
    for percentile in ['p50','p95','p99']:
        values=[r for r in rows if r['case']==name and r['percentile']==percentile]
        assert len(values)==3
        row[percentile]={'median_normalized_change_percent':statistics.median(r['normalized_change_percent'] for r in values),
                         'changes_percent':[r['normalized_change_percent'] for r in values],
                         'after_native_ratios':[r['after_ratio'] for r in values],
                         'median_after_native_ratio':statistics.median(r['after_ratio'] for r in values)}
    summary.append(row)
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
check_frozen()
assert sha(Path(__file__))==manifest['runner_sha256']
assert sha(root/'source.json')==manifest['source_sha256']
manifest['finished_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat()
manifest['final_integrity_check']='All recorded source, library, benchmark/probe, bridge, tool, reference and runner hashes match.'
manifest['raw_results']={path.name:sha(path) for path in sorted(out.glob('*')) if path.name!='source.json'}
(out/'source.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('FINISHED',flush=True)
