"""Execute this frozen status-metadata cost plan only after root quiet authorization."""
import argparse
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--quiet-window-confirmed', action='store_true', required=True)
parser.parse_args()
from pathlib import Path
import datetime, hashlib, json, math, os, statistics, subprocess

root = Path(__file__).resolve().parent
source_path, plan_path = root/'source.json', root/'plan.json'
source, plan = json.loads(source_path.read_text()), json.loads(plan_path.read_text())
def sha(path): return hashlib.sha256(Path(path).read_bytes()).hexdigest()
source_hash, plan_hash, runner_hash = sha(source_path), sha(plan_path), sha(__file__)
assert source_hash == plan['source_manifest_sha256']
assert runner_hash == plan['runner_sha256']
def check_frozen():
    assert (sha(source_path),sha(plan_path),sha(__file__)) == (source_hash,plan_hash,runner_hash)
    assert sha(source['bridge_source']) == source['bridge_sha256']
    assert sha(source['reference_real_path']) == source['reference_sha256']
    for name, expected in source['tool_sha256'].items(): assert sha(name) == expected
    for role, row in source['variants'].items():
        for name, expected in row['sources'].items(): assert sha(Path(row['source'])/name) == expected, (role,name)
        assert sha(Path(row['target'])/'release/libsnacka.rlib') == row['library_sha256']
        assert sha(row['binary']) == row['binary_sha256']
    for case in source['cases']: assert sha(case['input_path']) == case['input_sha256']
check_frozen()
out = root/'acceptance'
out.mkdir(exist_ok=False)
environment = os.environ.copy()
for key in list(environment):
    if key.startswith(('DYLD_','Malloc')) or key in ('RUST_BACKTRACE','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS'):
        environment.pop(key,None)
environment.update(SNACKA_BENCH_QOS='user-initiated',MallocNanoZone='0',RUST_LOG='warn')
manifest = {'source_manifest_sha256':source_hash,'plan_sha256':plan_hash,'runner_sha256':runner_hash,
    'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'runtime_environment':{k:environment.get(k) for k in ('SNACKA_BENCH_QOS','MallocNanoZone','RUST_LOG','DYLD_LIBRARY_PATH','DYLD_INSERT_LIBRARIES','MallocGuardEdges','MallocScribble','MallocStackLogging','RUST_BACKTRACE','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS')},
    'runtime_scope':'Other environment inherited after removing all DYLD_/Malloc overrides. QoS setter return0 asserted by common bridge; no effective-class query or affinity.',
    'quiet_window':'Root explicitly authorized these exact pins after coordinating codec workers CPU-idle.',
    'processes':[],'quartet_results':[],'failures':[]}
def persist(): (out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
def percentile(values,pct): return sorted(values)[math.ceil(len(values)*pct/100)-1]
def validate(result,case):
    assert result['libopus_version'] == 'libopus 1.6.1'
    assert result['kind'] == 'paired_decoder_matrix'
    assert (result['sample_rate'],result['channels'],result['schedule']) == (case['rate'],case['channels'],case['schedule'])
    assert result['fec_events'] == case['expected_actual_fec_events_per_24_packet_traversal']
    for k in ('owned_pcm_hash','reference_pcm_hash','packet_range_hash','recovery_intervals','pcm_by_operation_kind'):
        assert result[k] == case[k], k
    assert result['all_sample_counts_match'] and result['ordinary_and_plc_final_ranges_match']
    assert result['timed_batches'] == 1000 and result['all_batches_meet_minimum']
    assert result['minimum_observed_batch_ns'] >= 200000
    assert len(result['owned_batch_ns']) == len(result['reference_batch_ns']) == 1000
    assert min(result['owned_batch_ns']+result['reference_batch_ns']) >= 200000
    operations = result['operations_per_batch']
    assert operations > 0
    for pct in (50,95,99):
        owned = percentile(result['owned_batch_ns'],pct)/operations
        native = percentile(result['reference_batch_ns'],pct)/operations
        reported = result[f'p{pct}_ns_per_operation']
        assert math.isclose(owned,reported['snacka'],rel_tol=1e-12)
        assert math.isclose(native,reported['libopus'],rel_tol=1e-12)
        assert math.isclose(owned/native,reported['ratio'],rel_tol=1e-12)
persist()
try:
    for quartet in range(plan['quartets_per_case']):
        for index,case in enumerate(source['cases']):
            case_id = case['case_id']
            roles = ['before','after','after','before'] if (quartet+index)%2==0 else ['after','before','before','after']
            results = []
            for slot,role in enumerate(roles):
                command = [source['variants'][role]['binary'],'matrix',str(case['rate']),str(case['channels']),case['schedule'],'1000','200000',case['input_path']]
                row = {'quartet':quartet,'case_id':case_id,'slot':slot,'role':role,'command':command,'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat()}
                process = subprocess.Popen(command,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=environment)
                row['pid'] = process.pid
                try:
                    stdout,stderr = process.communicate()
                except BaseException:
                    process.terminate()
                    stdout,stderr = process.communicate()
                    row.update(return_code=process.returncode,status='INTERRUPTED')
                    raise
                finally:
                    prefix = out/f'{quartet}-{index:02d}-{slot}-{role}'
                    output,error = Path(str(prefix)+'.json'),Path(str(prefix)+'.stderr')
                    output.write_text(stdout); error.write_text(stderr)
                    row.update(finished_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),return_code=process.returncode,stdout=str(output),stdout_sha256=sha(output),stderr=str(error),stderr_sha256=sha(error))
                    manifest['processes'].append(row);persist()
                result = None
                try:
                    assert process.returncode == 0, stderr
                    result = json.loads(stdout)
                    validate(result,case)
                    row['status'] = 'VALID'
                except Exception as error:
                    row.update(status='FAILED',error=repr(error))
                    manifest['failures'].append({'quartet':quartet,'case_id':case_id,'slot':slot,'role':role,'error':repr(error)})
                results.append((role,result,row['status']))
                persist()
            comparison = {'quartet':quartet,'case_id':case_id,'role_order':roles,'status':'VALID' if all(s=='VALID' for _,_,s in results) else 'INVALID','percentiles':{}}
            if comparison['status'] == 'VALID':
                for pct in (50,95,99):
                    before = [r[f'p{pct}_ns_per_operation'] for role,r,_ in results if role=='before']
                    after = [r[f'p{pct}_ns_per_operation'] for role,r,_ in results if role=='after']
                    direct = math.sqrt(after[0]['snacka']*after[1]['snacka']/(before[0]['snacka']*before[1]['snacka']))
                    normalized = math.sqrt(after[0]['ratio']*after[1]['ratio']/(before[0]['ratio']*before[1]['ratio']))
                    comparison['percentiles'][f'p{pct}'] = {'direct_owned_change_percent':100*(direct-1),'native_normalized_change_percent':100*(normalized-1),'before':before,'after':after}
            manifest['quartet_results'].append(comparison);persist()
            print(quartet,case_id,comparison['status'],{p:round(r['native_normalized_change_percent'],3) for p,r in comparison['percentiles'].items()},flush=True)
    assert len(manifest['processes']) == plan['expected_processes']
    summary = []
    for case in source['cases']:
        rows = [r for r in manifest['quartet_results'] if r['case_id']==case['case_id']]
        complete = len(rows)==3 and all(r['status']=='VALID' for r in rows)
        record = {'case_id':case['case_id'],'status':'VALID' if complete else 'INVALID','percentiles':{}}
        if complete:
            for pct in ('p50','p95','p99'):
                normalized = [r['percentiles'][pct]['native_normalized_change_percent'] for r in rows]
                direct = [r['percentiles'][pct]['direct_owned_change_percent'] for r in rows]
                record['percentiles'][pct] = {'native_normalized_changes_percent':normalized,'median_native_normalized_change_percent':statistics.median(normalized),'direct_owned_changes_percent':direct,'median_direct_owned_change_percent':statistics.median(direct)}
        summary.append(record)
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    check_frozen()
    manifest.update(finished_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),final_integrity_check='All frozen artifacts unchanged.',status='PASS' if not manifest['failures'] else 'FAILED')
    manifest['raw_results'] = {p.name:sha(p) for p in sorted(out.iterdir()) if p.name!='manifest.json'}
    persist()
    print('FINISHED',len(manifest['processes']),'processes,',len(manifest['failures']),'failures',flush=True)
    assert not manifest['failures'], manifest['failures']
except BaseException as error:
    manifest.update(status='FAILED_OR_INTERRUPTED',error=repr(error));persist();raise
