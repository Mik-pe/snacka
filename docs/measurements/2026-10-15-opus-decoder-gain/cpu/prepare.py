from pathlib import Path
import hashlib, io, json, os, subprocess, tarfile

root=Path(__file__).resolve().parent
repo=Path('/tmp/snacka-opus-decoder-gain-20261015')
before='d5d3971b7b75137df1d1150fb5df9c4ad7062d5e'
after='6c4f916f9f2357dae8b388810a5bb50855ca2e9d'
toolchain=Path('/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin')
inputs=Path('/tmp/snacka-parity-20261015/short-final-matrix')
reference=Path('/opt/homebrew/lib/libopus.dylib').resolve()
cases=[
    ('SILK-WB-20ms-1ch',8000,1,'decode'),
    ('SILK-WB-20ms-1ch',48000,1,'decode'),
    ('SILK-WB-20ms-2ch',48000,2,'decode'),
    ('Hybrid-FB-10ms-2ch',48000,2,'fec-return'),
    ('CELT-FB-2.5ms-2ch',48000,2,'decode'),
    ('CELT-FB-5ms-1ch',48000,1,'decode'),
    ('CELT-WB-5ms-1ch',48000,1,'decode'),
]
def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
environment=dict(os.environ)
for key in list(environment):
    if key.startswith('CARGO_PROFILE_RELEASE_') or key.endswith('_RUSTFLAGS') or key in ('RUSTFLAGS','RUSTC_BOOTSTRAP','CARGO_BUILD_TARGET','MACOSX_DEPLOYMENT_TARGET','SDKROOT','CC','CXX','CFLAGS','CXXFLAGS','LDFLAGS','DYLD_LIBRARY_PATH','DYLD_INSERT_LIBRARIES'):
        environment.pop(key)
environment.update({'RUSTC':str(toolchain/'rustc'),'RUSTDOC':str(toolchain/'rustdoc'),
    'RUSTC_WRAPPER':'','RUSTC_WORKSPACE_WRAPPER':'',
    'CARGO_INCREMENTAL':'0','CARGO_BUILD_INCREMENTAL':'false',
    'CARGO_PROFILE_RELEASE_CODEGEN_UNITS':'16','CARGO_PROFILE_RELEASE_LTO':'false',
    'CARGO_PROFILE_RELEASE_OPT_LEVEL':'3','CARGO_PROFILE_RELEASE_DEBUG':'0',
    'CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS':'true','CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS':'false',
    'RUSTFLAGS':'','CARGO_ENCODED_RUSTFLAGS':''})
bridge=root/'benchmark_opus_decode.rs'
bridge.write_bytes(subprocess.check_output(['git','show',f'{before}:tools/reference/benchmark_opus_decode.rs'],cwd=repo))
assert bridge.read_bytes()==subprocess.check_output(['git','show',f'{after}:tools/reference/benchmark_opus_decode.rs'],cwd=repo)
keys=['RUSTC','RUSTDOC','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','RUSTC_BOOTSTRAP',
    'CARGO_INCREMENTAL','CARGO_BUILD_INCREMENTAL','CARGO_BUILD_TARGET',
    'CARGO_PROFILE_RELEASE_CODEGEN_UNITS','CARGO_PROFILE_RELEASE_LTO','CARGO_PROFILE_RELEASE_OPT_LEVEL',
    'CARGO_PROFILE_RELEASE_DEBUG','CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS','CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS',
    'RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CARGO_BUILD_RUSTFLAGS','CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS',
    'MACOSX_DEPLOYMENT_TARGET','SDKROOT','CC','CXX','CFLAGS','CXXFLAGS','LDFLAGS']
manifest={'before_commit':before,'after_commit':after,'gain_source_commit':'a939e81a71679919bd7e2e5ee4d28b831611b2ac',
    'scope':'Ordinary Decoder default gain 0 cost only; no gain setter in the unchanged common bridge. No nonzero-gain performance claim.',
    'build_environment':{key:environment.get(key) for key in keys},
    'effective_release_profile':{'opt_level':3,'debug':0,'codegen_units':16,'lto':False,'overflow_checks':True,'debug_assertions':False,'incremental':False,'target_cpu':'Rust aarch64-apple-darwin default; no target-cpu or extra target-feature flags'},
    'bridge_source':str(bridge),'bridge_sha256':sha(bridge),
    'reference_real_path':str(reference),'reference_sha256':sha(reference),
    'cargo_version':subprocess.check_output([str(toolchain/'cargo'),'-Vv'],text=True,env=environment),
    'rustc_version':subprocess.check_output([str(toolchain/'rustc'),'-Vv'],text=True,env=environment),
    'tool_sha256':{str(toolchain/name):sha(toolchain/name) for name in ['cargo','rustc','rustdoc']},
    'host':{key:subprocess.check_output(command,text=True).strip() for key,command in {'os':['sw_vers'],'hardware':['sysctl','-n','hw.model'],'cpu':['sysctl','-n','machdep.cpu.brand_string'],'architecture':['uname','-m']}.items()},
    'prepare_runner_sha256':sha(Path(__file__)),'variants':{},'cases':[]}
for role,commit in [('before',before),('after',after)]:
    source=root/role
    source.mkdir()
    archive=subprocess.check_output(['git','archive',commit],cwd=repo)
    with tarfile.open(fileobj=io.BytesIO(archive)) as files:
        files.extractall(source,filter='data')
    sources={str(path.relative_to(source)):sha(path) for path in [source/'Cargo.toml',source/'Cargo.lock',*sorted((source/'src').rglob('*'))] if path.is_file()}
    target=root/f'target-{role}'
    command=[str(toolchain/'cargo'),'build','--locked','--release','--features','opus','--lib','--target-dir',str(target)]
    with (root/f'build-{role}.log').open('w') as log:
        subprocess.run(command,cwd=source,env=environment,check=True,stdout=log,stderr=subprocess.STDOUT)
    binary=root/f'benchmark-{role}'
    bridge_command=[str(toolchain/'rustc'),'--edition=2024','-O','-C','codegen-units=16','-C','lto=off','-C','debuginfo=0','-C','overflow-checks=yes',str(bridge),'--extern',f'snacka={target}/release/libsnacka.rlib','-L',f'dependency={target}/release/deps','-L','native=/opt/homebrew/lib','-o',str(binary)]
    subprocess.run(bridge_command,cwd=source,env=environment,check=True)
    manifest['variants'][role]={'commit':commit,'source':str(source),'sources':sources,'target':str(target),'library_command':command,'library_sha256':sha(target/'release/libsnacka.rlib'),'bridge_command':bridge_command,'binary':str(binary),'binary_sha256':sha(binary),'linked_libraries':subprocess.check_output(['otool','-L',str(binary)],text=True)}
    print('built',role,manifest['variants'][role]['binary_sha256'],flush=True)
changed=[path for path,expected in manifest['variants']['before']['sources'].items() if manifest['variants']['after']['sources'][path]!=expected]
assert changed==['src/opus/decoder.rs'],changed
manifest['changed_production_files']=changed
proof=root/'proof'
proof.mkdir()
for index,(name,rate,channels,schedule) in enumerate(cases):
    path=inputs/f'{name}.bit'
    pair={}
    for role in ['before','after']:
        command=[str(root/f'benchmark-{role}'),'matrix',str(rate),str(channels),schedule,'0','200000',str(path)]
        process=subprocess.run(command,text=True,capture_output=True,env=environment)
        (proof/f'{index:02d}-{role}.json').write_text(process.stdout)
        (proof/f'{index:02d}-{role}.stderr').write_text(process.stderr)
        assert process.returncode==0,(command,process.stderr)
        pair[role]=json.loads(process.stdout)
    assert pair['before']==pair['after'],name
    result=pair['after']
    assert result['timed_batches']==0
    assert result['all_sample_counts_match'] and result['ordinary_and_plc_final_ranges_match']
    assert result['packets']==24
    expected_fec=int(schedule=='fec-return')
    assert result['fec_events']==expected_fec,(name,result['fec_events'])
    manifest['cases'].append({'name':name,'rate':rate,'channels':channels,'schedule':schedule,'input_path':str(path),'input_sha256':sha(path),'expected_actual_fec_events_per_24_packet_traversal':expected_fec,'exact_untimed_before_after_json':True,'owned_pcm_hash':result['owned_pcm_hash'],'packet_range_hash':result['packet_range_hash'],'recovery_intervals':result['recovery_intervals']})
    print('verified',name,rate,channels,schedule,'actual FEC',result['fec_events'],flush=True)
(root/'source.json').write_text(json.dumps(manifest,indent=2)+'\n')
plan={'status':'PREPARED_NOT_STARTED','timing_authorized':False,'requires':'Explicit root quiet-window authorization; no auto-start.',
    'source_manifest_sha256':sha(root/'source.json'),'cases':manifest['cases'],'quartets_per_case':3,'roles_per_quartet':4,
    'role_order':'ABBA: before/after/after/before when (quartet+case_index) even, reversed after/before/before/after otherwise.',
    'batches_per_process':1000,'minimum_batch_ns':200000,'expected_processes':len(cases)*3*4,
    'scope':'7 selected complete decoder workloads. Existing matrix warmup/calibration/batches are entered only after authorization. FEC-return is a mixed 24-packet schedule with one verified actual FEC event, not an isolated FEC callback benchmark.',
    'analysis':'Retain every native/owned raw batch, stdout/stderr, process order and failure. Recompute p50/p95/p99 per process. Per quartet compare geometric mean of two after and two before owned/native ratios and direct owned times; show all three quartet changes and medians. No adaptive retries, substitutions or selected tails.',
    'runner':str(root/'run.py'),'runner_sha256':sha(root/'run.py')}
(root/'plan.json').write_text(json.dumps(plan,indent=2)+'\n')
print('PREPARED_NOT_STARTED',len(cases),'cases',len(cases)*3*4,'planned processes',flush=True)
