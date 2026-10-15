"""Execute this frozen default-zero gain cost plan only after root quiet authorization."""
import argparse
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--quiet-window-confirmed',action='store_true',required=True)
parser.parse_args()
from pathlib import Path
import datetime,hashlib,json,math,os,statistics,subprocess

root=Path(__file__).resolve().parent
plan_path=root/'plan.json'
source_path=root/'source.json'
plan=json.loads(plan_path.read_text())
source=json.loads(source_path.read_text())
def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
source_hash=sha(source_path)
plan_hash=sha(plan_path)
runner_hash=sha(Path(__file__))
assert source_hash==plan['source_manifest_sha256']
assert runner_hash==plan['runner_sha256']
def check_frozen():
    assert sha(source_path)==source_hash and sha(plan_path)==plan_hash and sha(Path(__file__))==runner_hash
    assert sha(Path(source['bridge_source']))==source['bridge_sha256']
    assert sha(Path(source['reference_real_path']))==source['reference_sha256']
    for name,expected in source['tool_sha256'].items(): assert sha(Path(name))==expected
    for role,row in source['variants'].items():
        for name,expected in row['sources'].items(): assert sha(Path(row['source'])/name)==expected,(role,name)
        assert sha(Path(row['target'])/'release/libsnacka.rlib')==row['library_sha256']
        assert sha(Path(row['binary']))==row['binary_sha256']
    for case in source['cases']: assert sha(Path(case['input_path']))==case['input_sha256']
check_frozen()
out=root/'acceptance'
out.mkdir()
environment=dict(os.environ,SNACKA_BENCH_QOS='user-initiated',MallocNanoZone='0',RUST_LOG='warn')
for key in ['DYLD_LIBRARY_PATH','DYLD_INSERT_LIBRARIES','MallocGuardEdges','MallocScribble','MallocStackLogging','RUST_BACKTRACE','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS']:
    environment.pop(key,None)
manifest={'source_manifest_sha256':source_hash,'plan_sha256':plan_hash,'runner_sha256':runner_hash,
    'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'runtime_environment':{key:environment.get(key) for key in ['SNACKA_BENCH_QOS','MallocNanoZone','RUST_LOG','DYLD_LIBRARY_PATH','DYLD_INSERT_LIBRARIES','MallocGuardEdges','MallocScribble','MallocStackLogging','RUST_BACKTRACE','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS']},
    'quiet_window':'Root explicitly authorized this frozen plan after coordinating all codec workers CPU-idle.',
    'processes':[],'quartet_results':[]}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
def percentile(values,pct): return sorted(values)[math.ceil(len(values)*pct/100)-1]
for quartet in range(3):
    for index,case in enumerate(source['cases']):
        case_id=f"{case['name']}-{case['rate']}-{case['schedule']}"
        roles=['before','after','after','before'] if (quartet+index)%2==0 else ['after','before','before','after']
        results=[]
        for slot,role in enumerate(roles):
            command=[str(root/f'benchmark-{role}'),'matrix',str(case['rate']),str(case['channels']),case['schedule'],'1000','200000',case['input_path']]
            started=datetime.datetime.now(datetime.timezone.utc).isoformat()
            process=subprocess.Popen(command,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=environment)
            stdout,stderr=process.communicate()
            prefix=out/f'{quartet}-{index:02d}-{slot}-{role}'
            output=Path(str(prefix)+'.json')
            error=Path(str(prefix)+'.stderr')
            output.write_text(stdout)
            error.write_text(stderr)
            row={'quartet':quartet,'case_id':case_id,'slot':slot,'role':role,'command':command,'pid':process.pid,'started_utc':started,'finished_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'return_code':process.returncode,'stdout':str(output),'stdout_sha256':sha(output),'stderr':str(error),'stderr_sha256':sha(error)}
            manifest['processes'].append(row)
            (out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
            assert process.returncode==0,(command,stderr)
            result=json.loads(stdout)
            assert result['libopus_version']=='libopus 1.6.1'
            assert result['fec_events']==case['expected_actual_fec_events_per_24_packet_traversal']
            assert result['owned_pcm_hash']==case['owned_pcm_hash']
            assert result['packet_range_hash']==case['packet_range_hash']
            assert result['recovery_intervals']==case['recovery_intervals']
            assert result['all_sample_counts_match'] and result['ordinary_and_plc_final_ranges_match']
            assert result['timed_batches']==1000 and result['all_batches_meet_minimum']
            assert result['minimum_observed_batch_ns']>=200000
            assert len(result['owned_batch_ns'])==len(result['reference_batch_ns'])==1000
            operations=result['operations_per_batch']
            for pct in [50,95,99]:
                owned=percentile(result['owned_batch_ns'],pct)/operations
                native=percentile(result['reference_batch_ns'],pct)/operations
                reported=result[f'p{pct}_ns_per_operation']
                assert math.isclose(owned,reported['snacka'],rel_tol=1e-12)
                assert math.isclose(native,reported['libopus'],rel_tol=1e-12)
                assert math.isclose(owned/native,reported['ratio'],rel_tol=1e-12)
            results.append((role,result))
        comparison={'quartet':quartet,'case_id':case_id,'role_order':roles,'percentiles':{}}
        for pct in [50,95,99]:
            before=[r[f'p{pct}_ns_per_operation'] for role,r in results if role=='before']
            after=[r[f'p{pct}_ns_per_operation'] for role,r in results if role=='after']
            direct=math.sqrt(after[0]['snacka']*after[1]['snacka']/(before[0]['snacka']*before[1]['snacka']))
            normalized=math.sqrt(after[0]['ratio']*after[1]['ratio']/(before[0]['ratio']*before[1]['ratio']))
            comparison['percentiles'][f'p{pct}']={'direct_owned_change_percent':100*(direct-1),'native_normalized_change_percent':100*(normalized-1),'before':before,'after':after}
        manifest['quartet_results'].append(comparison)
        (out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
        print(quartet,case_id,{pct:round(row['native_normalized_change_percent'],3) for pct,row in comparison['percentiles'].items()},flush=True)
assert len(manifest['processes'])==plan['expected_processes']
summary=[]
for case in source['cases']:
    case_id=f"{case['name']}-{case['rate']}-{case['schedule']}"
    rows=[row for row in manifest['quartet_results'] if row['case_id']==case_id]
    summary.append({'case_id':case_id,'percentiles':{pct:{'native_normalized_changes_percent':[row['percentiles'][pct]['native_normalized_change_percent'] for row in rows],'median_native_normalized_change_percent':statistics.median(row['percentiles'][pct]['native_normalized_change_percent'] for row in rows),'direct_owned_changes_percent':[row['percentiles'][pct]['direct_owned_change_percent'] for row in rows],'median_direct_owned_change_percent':statistics.median(row['percentiles'][pct]['direct_owned_change_percent'] for row in rows)} for pct in ['p50','p95','p99']}})
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
check_frozen()
manifest['finished_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat()
manifest['final_integrity_check']='All source, library, binary, bridge, tool, reference, input, plan and runner hashes unchanged.'
manifest['raw_results']={path.name:sha(path) for path in sorted(out.iterdir()) if path.name!='manifest.json'}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('FINISHED',len(manifest['processes']),'processes',flush=True)
