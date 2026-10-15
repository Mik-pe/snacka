import hashlib,json,os,subprocess
from pathlib import Path
root=Path('/tmp/snacka-opus-decoder-last-samples-evidence-20261015');repo=Path('/tmp/snacka-opus-decoder-last-packet-samples-20261015');tc=Path('/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin')
env=dict(os.environ)
for key in list(env):
 if key.startswith('CARGO_PROFILE_RELEASE_') or key.endswith('_RUSTFLAGS') or key in ('RUSTFLAGS','RUSTC_BOOTSTRAP','CARGO_BUILD_TARGET','MACOSX_DEPLOYMENT_TARGET','SDKROOT','CC','CXX','CFLAGS','CXXFLAGS','LDFLAGS','DYLD_LIBRARY_PATH','DYLD_INSERT_LIBRARIES'):
  env.pop(key)
env.update({'RUSTC':str(tc/'rustc'),'RUSTDOC':str(tc/'rustdoc'),'RUSTC_WRAPPER':'','RUSTC_WORKSPACE_WRAPPER':'','CARGO_INCREMENTAL':'0','CARGO_BUILD_INCREMENTAL':'false','CARGO_PROFILE_RELEASE_CODEGEN_UNITS':'16','CARGO_PROFILE_RELEASE_LTO':'false','CARGO_PROFILE_RELEASE_OPT_LEVEL':'3','CARGO_PROFILE_RELEASE_DEBUG':'0','CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS':'true','CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS':'false','RUSTFLAGS':'','CARGO_ENCODED_RUSTFLAGS':'-L/opt/homebrew/lib'})
target='/tmp/snacka-opus-decoder-last-samples-check-target';base=[str(tc/'cargo'),'test','--offline','--locked','--release','--features','opus,reference-tests','--target-dir',target]
checks=[('targeted-native-contracts',base+['--test','opus_calls_reference','--','--nocapture']),('targeted-allocation-and-stack',base+['--test','allocations','--test','opus_stack','--','--nocapture']),('release-tests',base),('clippy',[str(tc/'cargo'),'clippy','--offline','--locked','--release','--all-targets','--features','opus,reference-tests','--target-dir',target,'--','-D','warnings']),('fmt',[str(tc/'cargo'),'fmt','--all','--','--check']),('diff-check',['git','diff','--check'])]
receipt={'source_commit':'89e2b209184b43de3e69c37ad33f6b00976254f9','runner_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'environment':{k:env[k] for k in ['RUSTC','RUSTDOC','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_INCREMENTAL','CARGO_BUILD_INCREMENTAL','CARGO_PROFILE_RELEASE_CODEGEN_UNITS','CARGO_PROFILE_RELEASE_LTO','CARGO_PROFILE_RELEASE_OPT_LEVEL','CARGO_PROFILE_RELEASE_DEBUG','CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS','CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS']},'lto_scope':'Cargo profile LTO=false: cross-crate LTO disabled; rustc may perform local ThinLTO across the16 units. No -C lto=off or target-cpu override supplied to Cargo.','checks':[]}
for name,command in checks:
 with (root/f'{name}.log').open('w') as log:r=subprocess.run(command,cwd=repo,env=env,stdout=log,stderr=subprocess.STDOUT)
 receipt['checks'].append({'name':name,'command':command,'exitcode':r.returncode,'log_sha256':hashlib.sha256((root/f'{name}.log').read_bytes()).hexdigest()})
 (root/'gates.json').write_text(json.dumps(receipt,indent=2)+'\n')
 print(name,r.returncode,flush=True)
 if r.returncode:raise SystemExit(r.returncode)
