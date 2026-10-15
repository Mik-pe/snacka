#!/usr/bin/env python3
"""Untimed source/input/output provenance and count checks for callback adapter."""
import collections, csv, datetime, hashlib, json, subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parent
ART = ROOT.parent.parent
CPU = Path('/tmp/snacka-opus-silk-sparse-cpu-20261015')
SOURCE = Path('/tmp/snacka-opus-silk-sparse-fallback-20261015')
PRODUCTION_COMMIT = '8f723ba1f35c053e1cc840f38a4b20446a127dde'
REPEATS = 16

def sha(data):
    return hashlib.sha256(data).hexdigest()

def file_record(path):
    data = path.read_bytes()
    return {'path': str(path), 'bytes': len(data), 'sha256': sha(data)}

def source_record(path):
    files = [*sorted((path/'src').rglob('*.rs')), path/'Cargo.toml', path/'Cargo.lock']
    hashes = {str(p.relative_to(path)): sha(p.read_bytes()) for p in files if p.exists()}
    return {'path': str(path), 'files': hashes,
            'combined_sha256': sha(json.dumps(hashes, sort_keys=True, separators=(',', ':')).encode())}

def packet_count(path, raw):
    data = path.read_bytes()
    offset = count = 0
    while offset < len(data):
        size = int.from_bytes(data[offset:offset+4], 'little')
        offset += (4 if raw else 20) + size
        count += 1
    assert offset == len(data)
    return count

assert not subprocess.check_output(['git','diff',PRODUCTION_COMMIT,'--','src','Cargo.toml','Cargo.lock'], cwd=SOURCE)
source = source_record(SOURCE)
assert source_record(CPU/'candidate')['files'] == source['files']
reports = {}
profiles_by_role = {}
for role in ['baseline','candidate']:
    output = ROOT/f'verify-{role}'
    summary = json.loads((output/'summary.json').read_text())
    rows = list(csv.DictReader((output/'callbacks.csv').open()))
    index = list(csv.DictReader((output/'inputs.csv').open()))
    pcm = (output/'inputs.f32').read_bytes()
    assert not summary['timing_recorded'] and summary['repeats'] == 1
    assert (summary['qos_set_return'], summary['qos_get_return'], summary['qos_queried'], summary['qos_relative_priority']) == (0,0,25,0)
    assert all(not row['elapsed_ns'] and row['overrun'] == 'false' for row in rows)
    assert all(x['timed_callbacks'] == 0 and x['p50_ns'] is None and x['max_ns'] is None for x in summary['samples'])
    by_profile = collections.defaultdict(list)
    for row in index:
        by_profile[row['profile']].append(row)
    call_rows = collections.defaultdict(list)
    for row in rows:
        call_rows[row['profile']].append(row)
    profiles = []
    for name, inputs in by_profile.items():
        first = int(inputs[0]['byte_offset'])
        end = int(inputs[-1]['byte_offset']) + int(inputs[-1]['samples'])*4
        for previous, following in zip(inputs, inputs[1:]):
            assert int(previous['byte_offset']) + int(previous['samples'])*4 == int(following['byte_offset'])
        calls = call_rows[name]
        accepted = sum(c['accepted'] == 'true' for c in calls)
        errors = [c for c in calls if c['accepted'] == 'false']
        assert len(errors) <= 1
        if errors:
            assert int(errors[0]['packet']) == accepted
            assert errors[0]['error'] == ('Encode(Output)' if name.startswith(('mapped-','projection-')) else 'Output')
        else:
            assert accepted == len(inputs)
        raw = False
        if name.startswith('normal-'):
            golden = ROOT/'normal-golden'/f'{name}.packets'
        elif name.startswith('original-'):
            label = name.split('-')[1]
            directory = 'refusal-sparse' if role == 'candidate' else 'refusal-baseline'
            golden = ART/directory/f'{label}.packets'
            raw = True
        elif name.startswith(('mapped-','projection-')):
            golden = ART/'controls'/f'composition-{role}-data'/f'{name}.packets'
        else:
            golden = ART/'controls'/f'fallback-{role}-data'/f'{name}.packets'
        assert packet_count(golden, raw) == accepted
        archive = file_record(golden)
        archive['packets'] = accepted
        archive['format'] = 'u32-length/packet' if raw else '20-byte-metadata/packet'
        range_path = golden.with_suffix('.ranges')
        if raw and range_path.exists():
            archive['ranges'] = file_record(range_path)
        profiles.append({'name': name, 'input_packets': len(inputs), 'input_samples': sum(int(x['samples']) for x in inputs),
                         'input_bytes': end-first, 'input_sha256': sha(pcm[first:end]), 'period_ns': int(inputs[0]['period_ns']),
                         'verified_successful_callbacks': accepted, 'verified_refused_callbacks': len(errors),
                         'planned_contexts': REPEATS, 'planned_successful_callbacks': accepted*REPEATS,
                         'planned_refused_callbacks': len(errors)*REPEATS,
                         'planned_total_callbacks': len(calls)*REPEATS, 'golden': archive})
    assert len(profiles) == 73
    expected = (668,63) if role == 'baseline' else (1337,0)
    assert (sum(p['verified_successful_callbacks'] for p in profiles), sum(p['verified_refused_callbacks'] for p in profiles)) == expected
    reports[role] = {'profiles': profiles, 'verified_summary': summary,
                     'planned_fresh_contexts': len(profiles)*REPEATS,
                     'planned_successful_callbacks': expected[0]*REPEATS,
                     'planned_refused_callbacks': expected[1]*REPEATS,
                     'planned_total_callbacks': sum(expected)*REPEATS,
                     'outputs': [file_record(output/name) for name in ['summary.json','callbacks.csv','inputs.f32','inputs.csv']],
                     'library': file_record(CPU/f'{role}-clean-target/release/libsnacka.rlib'),
                     'binary': file_record(ROOT/f'callback-{role}'),
                     'build_log': file_record(CPU/f'{role}-clean-build.log'),
                     'source_snapshot': source_record(CPU/role)}
    profiles_by_role[role] = profiles
assert [p['name'] for p in profiles_by_role['baseline']] == [p['name'] for p in profiles_by_role['candidate']]
assert [p['input_sha256'] for p in profiles_by_role['baseline']] == [p['input_sha256'] for p in profiles_by_role['candidate']]
assert (ROOT/'verify-baseline/inputs.f32').read_bytes() == (ROOT/'verify-candidate/inputs.f32').read_bytes()
assert (ROOT/'verify-baseline/inputs.csv').read_bytes() == (ROOT/'verify-candidate/inputs.csv').read_bytes()
compiler = '/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin/rustc'
result = {
    'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'production_commit': PRODUCTION_COMMIT, 'production_base': 'cca87dd',
    'current_documentation_head': subprocess.check_output(['git','rev-parse','HEAD'], cwd=SOURCE, text=True).strip(),
    'production_source_unchanged_from_commit': True, 'production_source': source,
    'adapter_files': [file_record(ROOT/name) for name in ['adapter.rs','README.md','build-and-verify.sh','freeze.py']],
    'normal_control_source': file_record(CPU/'throughput.rs'),
    'compiler_path': compiler, 'compiler_identity': subprocess.check_output([compiler,'-vV'],text=True),
    'adapter_compiler_flags': ['--edition=2024','-Copt-level=3','-Ccodegen-units=16','-Coverflow-checks=on','-Cembed-bitcode=no','-Cstrip=debuginfo'],
    'target_cpu_lto_incremental_extra_flags': [],
    'profile_categories': {'synthetic_clipping':56,'fec_carrier':1,'original_refusals':2,'mapped_projection':4,'normal_controls':10},
    'profiles':73, 'planned_repeats':REPEATS, 'planned_callback_process_count':2,
    'composition_and_normal_integrated_in_each_role_process':True,
    'planned_total_intervals_both_roles':33088,
    'timing_run':False, 'verify_samples_instant':False,
    'preliminary_63_profile_verifications_retained': [str(ROOT/f'verify-63-prefinal-{role}') for role in ['baseline','candidate']],
    'preliminary_outputs': [file_record(ROOT/f'verify-63-prefinal-{role}'/name) for role in ['baseline','candidate'] for name in ['summary.json','callbacks.csv']],
    'preliminary_binary_hashes': [file_record(ROOT/f'callback-63-prefinal-{role}') for role in ['baseline','candidate']],
    'prefinal_adapter_changes': ['Add10 normal controls/shared baseline goldens','Assert Output error class already present in saved63-profile outputs','Export exact prepared PCM and index'],
    'roles':reports,
}
(ROOT/'freeze.json').write_text(json.dumps(result,indent=2,sort_keys=True)+'\n')
print(json.dumps({'profiles':73,'prepared_input_packets':sum(p['input_packets'] for p in profiles_by_role['candidate']),
                  'input_pcm_bytes':(ROOT/'verify-candidate/inputs.f32').stat().st_size,
                  'planned_callback_processes':2,'planned_total_intervals':33088,
                  'baseline':reports['baseline']['planned_total_callbacks'],'candidate':reports['candidate']['planned_total_callbacks'],
                  'timing_run':False}))
